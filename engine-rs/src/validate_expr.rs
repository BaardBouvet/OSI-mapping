use regex::Regex;
use std::sync::LazyLock;

/// The context in which an expression appears, controlling which
/// keywords are permitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprContext {
    /// `expression:` on a field mapping — source columns available.
    ForwardExpression,
    /// `reverse_expression:` on a field mapping — target fields + r.alias.
    ReverseExpression,
    /// `filter:` on a mapping — source columns available.
    Filter,
    /// `reverse_filter:` on a mapping — target fields available.
    ReverseFilter,
    /// `default_expression:` on a target field — target fields available.
    DefaultExpression,
    /// `expression:` on a target field (strategy: expression) — aggregation context.
    TargetExpression,
    /// `last_modified.expression:` — timestamp derivation.
    LastModifiedExpression,
}

/// Validate that `expr` is a safe column-level SQL snippet.
/// Returns `Ok(())` if the expression passes all checks, or
/// `Err(message)` describing the first violation found.
pub fn validate_expression(expr: &str, context: ExprContext) -> Result<(), String> {
    // 1. Reject semicolons
    if expr.contains(';') {
        return Err("contains semicolon — expressions must be single SQL snippets".into());
    }

    // 2. Reject prohibited keywords (after stripping string literals)
    let exempt = match context {
        ExprContext::TargetExpression => &["ORDER", "DISTINCT"][..],
        _ => &[],
    };
    if let Some(kw) = contains_prohibited_keyword(expr, exempt) {
        return Err(format!(
            "contains prohibited keyword '{kw}' — expressions must be column-level SQL snippets"
        ));
    }

    // 3. Reject internal view name patterns
    if let Some(prefix) = contains_internal_view_ref(expr) {
        return Err(format!(
            "references internal view prefix '{prefix}' — expressions must not depend on generated view names"
        ));
    }

    // 4. Balanced parentheses
    check_balanced_parens(expr)?;

    // 5. Balanced single quotes
    check_balanced_quotes(expr)?;

    Ok(())
}

// ── Keyword detection ────────────────────────────────────────────────

static PROHIBITED_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "JOIN", "WHERE", "GROUP", "HAVING", "LIMIT", "ORDER",
    "DISTINCT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER",
    "TRUNCATE", "BEGIN", "COMMIT", "ROLLBACK", "GRANT", "REVOKE", "COPY",
    "EXECUTE",
];

/// Regex that matches `'...'` including escaped quotes (`''`).
static LITERAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"'(?:[^']|'')*'").unwrap());

fn strip_string_literals(expr: &str) -> String {
    LITERAL_RE.replace_all(expr, "''").to_string()
}

fn contains_prohibited_keyword(expr: &str, exempt: &[&str]) -> Option<String> {
    let stripped = strip_string_literals(expr);
    for &kw in PROHIBITED_KEYWORDS {
        if exempt.contains(&kw) {
            continue;
        }
        let pattern = format!(r"(?i)\b{}\b", kw);
        if Regex::new(&pattern).unwrap().is_match(&stripped) {
            return Some(kw.to_string());
        }
    }
    None
}

// ── Internal view references ────────────────────────────────────────

static INTERNAL_PREFIXES: &[&str] = &[
    "_fwd_", "_id_", "_resolved_", "_rev_", "_delta_", "_grp_",
];

fn contains_internal_view_ref(expr: &str) -> Option<String> {
    let stripped = strip_string_literals(expr);
    let lower = stripped.to_lowercase();
    for &prefix in INTERNAL_PREFIXES {
        if lower.contains(prefix) {
            return Some(prefix.to_string());
        }
    }
    None
}

// ── Structural checks ───────────────────────────────────────────────

fn check_balanced_parens(expr: &str) -> Result<(), String> {
    let mut depth: i32 = 0;
    let mut in_quote = false;
    let chars: Vec<char> = expr.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        let ch = chars[i];
        if in_quote {
            if ch == '\'' {
                if i + 1 < len && chars[i + 1] == '\'' {
                    i += 2;
                    continue;
                }
                in_quote = false;
            }
        } else {
            match ch {
                '\'' => in_quote = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return Err("unmatched closing parenthesis".into());
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    if depth != 0 {
        return Err(format!("unbalanced parentheses (depth {depth})"));
    }
    Ok(())
}

fn check_balanced_quotes(expr: &str) -> Result<(), String> {
    let mut in_quote = false;
    let chars: Vec<char> = expr.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        let ch = chars[i];
        if in_quote {
            if ch == '\'' {
                if i + 1 < len && chars[i + 1] == '\'' {
                    i += 2;
                    continue;
                }
                in_quote = false;
            }
        } else if ch == '\'' {
            in_quote = true;
        }
        i += 1;
    }
    if in_quote {
        return Err("unterminated string literal".into());
    }
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_function_call() {
        assert!(validate_expression("SPLIT_PART(name, ' ', 1)", ExprContext::ForwardExpression).is_ok());
    }

    #[test]
    fn accepts_cast() {
        assert!(validate_expression("TO_DATE(dob, 'DD/MM/YY')", ExprContext::ForwardExpression).is_ok());
    }

    #[test]
    fn accepts_concatenation() {
        assert!(validate_expression("first_name || ' ' || last_name", ExprContext::ReverseExpression).is_ok());
    }

    #[test]
    fn accepts_literal() {
        assert!(validate_expression("'employee'", ExprContext::ForwardExpression).is_ok());
    }

    #[test]
    fn accepts_boolean_literal() {
        assert!(validate_expression("true", ExprContext::ForwardExpression).is_ok());
    }

    #[test]
    fn accepts_coalesce() {
        assert!(validate_expression("COALESCE(phone, '__CLEARED__')", ExprContext::ForwardExpression).is_ok());
    }

    #[test]
    fn accepts_is_not_null() {
        assert!(validate_expression("deleted_at IS NOT NULL", ExprContext::ForwardExpression).is_ok());
    }

    #[test]
    fn accepts_comparison_filter() {
        assert!(validate_expression("contact_type = 'person'", ExprContext::Filter).is_ok());
    }

    #[test]
    fn accepts_like_filter() {
        assert!(validate_expression("type LIKE '%employee%'", ExprContext::ReverseFilter).is_ok());
    }

    #[test]
    fn accepts_is_not_true_filter() {
        assert!(validate_expression("is_deleted IS NOT TRUE", ExprContext::ReverseFilter).is_ok());
    }

    #[test]
    fn accepts_aggregate_with_order_distinct() {
        assert!(validate_expression(
            "string_agg(distinct type, ',' order by type)",
            ExprContext::TargetExpression,
        ).is_ok());
    }

    #[test]
    fn accepts_max_aggregate() {
        assert!(validate_expression("max(price)", ExprContext::TargetExpression).is_ok());
    }

    #[test]
    fn accepts_nullif() {
        assert!(validate_expression("NULLIF(phone, '__CLEARED__')", ExprContext::ReverseExpression).is_ok());
    }

    #[test]
    fn accepts_default_expression() {
        assert!(validate_expression("SPLIT_PART(full_name, ' ', 1)", ExprContext::DefaultExpression).is_ok());
    }

    #[test]
    fn accepts_regex_replace() {
        assert!(validate_expression("REGEXP_REPLACE(phone_number, '[^0-9]', '', 'g')", ExprContext::ForwardExpression).is_ok());
    }

    #[test]
    fn accepts_complex_reverse() {
        assert!(validate_expression(
            "'+' || SUBSTRING(phone, 1, 1) || ' ' || SUBSTRING(phone, 2, 3) || '-' || SUBSTRING(phone, 5)",
            ExprContext::ReverseExpression,
        ).is_ok());
    }

    // ── Rejection tests ──────────────────────────────────────────────

    #[test]
    fn rejects_semicolon() {
        let err = validate_expression("name; DROP TABLE users", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("semicolon"), "{err}");
    }

    #[test]
    fn rejects_select() {
        let err = validate_expression("(SELECT min(phone) FROM phones)", ExprContext::ReverseExpression).unwrap_err();
        assert!(err.contains("SELECT"), "{err}");
    }

    #[test]
    fn rejects_drop() {
        let err = validate_expression("DROP TABLE users", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("DROP"), "{err}");
    }

    #[test]
    fn rejects_insert() {
        let err = validate_expression("INSERT INTO t VALUES (1)", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("INSERT"), "{err}");
    }

    #[test]
    fn rejects_internal_view_fwd() {
        let err = validate_expression("_fwd_contact.name", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("_fwd_"), "{err}");
    }

    #[test]
    fn rejects_internal_view_resolved() {
        let err = validate_expression(
            r#"(SELECT min("phone") FROM "_resolved_phone_entry")"#,
            ExprContext::ReverseExpression,
        ).unwrap_err();
        // Could match SELECT first or _resolved_, both are violations
        assert!(err.contains("SELECT") || err.contains("_resolved_"), "{err}");
    }

    #[test]
    fn rejects_unbalanced_open_paren() {
        let err = validate_expression("COALESCE(a, b", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("parenthes"), "{err}");
    }

    #[test]
    fn rejects_unbalanced_close_paren() {
        let err = validate_expression("a, b)", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("parenthes"), "{err}");
    }

    #[test]
    fn rejects_unterminated_string() {
        let err = validate_expression("name = 'foo", ExprContext::Filter).unwrap_err();
        assert!(err.contains("string literal"), "{err}");
    }

    #[test]
    fn keyword_inside_string_literal_is_ok() {
        assert!(validate_expression("name = 'SELECT FROM WHERE'", ExprContext::Filter).is_ok());
    }

    #[test]
    fn order_rejected_outside_aggregate_context() {
        let err = validate_expression("name ORDER BY id", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("ORDER"), "{err}");
    }

    #[test]
    fn rejects_delta_prefix() {
        let err = validate_expression("_delta_contact.name", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("_delta_"), "{err}");
    }

    #[test]
    fn accepts_escaped_quotes() {
        assert!(validate_expression("name = 'it''s'", ExprContext::Filter).is_ok());
    }

    #[test]
    fn rejects_grant() {
        let err = validate_expression("GRANT ALL ON t TO public", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("GRANT"), "{err}");
    }
}
