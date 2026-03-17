use regex::Regex;
use std::collections::HashSet;
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
    "SELECT", "FROM", "JOIN", "WHERE", "GROUP", "HAVING", "LIMIT", "ORDER", "DISTINCT", "INSERT",
    "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "TRUNCATE", "BEGIN", "COMMIT", "ROLLBACK",
    "GRANT", "REVOKE", "COPY", "EXECUTE",
];

/// Regex that matches `'...'` including escaped quotes (`''`).
static LITERAL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"'(?:[^']|'')*'").unwrap());

fn strip_string_literals(expr: &str) -> String {
    LITERAL_RE.replace_all(expr, "''").to_string()
}

fn contains_prohibited_keyword(expr: &str, exempt: &[&str]) -> Option<String> {
    let stripped = strip_string_literals(expr);
    for &kw in PROHIBITED_KEYWORDS {
        if exempt.contains(&kw) {
            continue;
        }
        let pattern = format!(r"(?i)\b{kw}\b");
        if Regex::new(&pattern).unwrap().is_match(&stripped) {
            return Some(kw.to_string());
        }
    }
    None
}

// ── Internal view references ────────────────────────────────────────

static INTERNAL_PREFIXES: &[&str] = &[
    "_fwd_",
    "_id_",
    "_resolved_",
    "_ordered_",
    "_rev_",
    "_delta_",
    "_grp_",
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

// ── Identifier extraction (Phase 2) ─────────────────────────────────

/// SQL keywords that should not be treated as column references.
static SQL_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Prohibited keywords (from Phase 1)
        "SELECT", "FROM", "JOIN", "WHERE", "GROUP", "HAVING", "LIMIT", "ORDER", "DISTINCT",
        "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "TRUNCATE", "BEGIN", "COMMIT",
        "ROLLBACK", "GRANT", "REVOKE", "COPY", "EXECUTE",
        // Operators & SQL grammar words
        "AND", "OR", "NOT", "IS", "IN", "AS", "ON", "BY", "BETWEEN", "LIKE", "ILIKE", "CASE",
        "WHEN", "THEN", "ELSE", "END", "NULL", "TRUE", "FALSE", "ASC", "DESC", "NULLS", "FIRST",
        "LAST", "ALL", "ANY", "SOME", "EXISTS", "CAST", "FILTER",
    ]
    .into_iter()
    .collect()
});

/// Common SQL function names that should not be treated as column references.
/// This is not exhaustive — unknown bare words before `(` are also treated
/// as function calls by the extraction logic.
static SQL_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Aggregate functions
        "MIN",
        "MAX",
        "SUM",
        "AVG",
        "COUNT",
        "BOOL_OR",
        "BOOL_AND",
        "STRING_AGG",
        "ARRAY_AGG",
        "JSONB_AGG",
        "JSON_AGG",
        // String functions
        "SPLIT_PART",
        "SUBSTRING",
        "SUBSTR",
        "LENGTH",
        "CHAR_LENGTH",
        "UPPER",
        "LOWER",
        "TRIM",
        "LTRIM",
        "RTRIM",
        "REPLACE",
        "CONCAT",
        "CONCAT_WS",
        "LEFT",
        "RIGHT",
        "LPAD",
        "RPAD",
        "REVERSE",
        "REGEXP_REPLACE",
        "REGEXP_MATCH",
        "REGEXP_MATCHES",
        // Date/time functions
        "TO_DATE",
        "TO_CHAR",
        "TO_TIMESTAMP",
        "TO_NUMBER",
        "DATE_PART",
        "DATE_TRUNC",
        "AGE",
        "NOW",
        "CURRENT_DATE",
        "CURRENT_TIMESTAMP",
        "EXTRACT",
        // Type conversion
        "COALESCE",
        "NULLIF",
        "GREATEST",
        "LEAST",
        // Math
        "ABS",
        "CEIL",
        "CEILING",
        "FLOOR",
        "ROUND",
        "TRUNC",
        "MOD",
        "POWER",
        "SQRT",
        "LOG",
        "LN",
        "EXP",
        "SIGN",
        // Hash / crypto
        "MD5",
        "SHA256",
        "ENCODE",
        "DECODE",
        // JSONB functions
        "JSONB_BUILD_OBJECT",
        "JSONB_BUILD_ARRAY",
        "JSONB_EXTRACT_PATH",
        "JSONB_EXTRACT_PATH_TEXT",
        "JSONB_ARRAY_ELEMENTS",
        "JSONB_ARRAY_ELEMENTS_TEXT",
        "JSON_EXTRACT_PATH_TEXT",
        "JSONB_TYPEOF",
        "JSONB_EACH",
        "JSONB_EACH_TEXT",
        "JSONB_OBJECT_KEYS",
        "JSONB_SET",
        "JSONB_STRIP_NULLS",
        "ROW_NUMBER",
        "RANK",
        "DENSE_RANK",
    ]
    .into_iter()
    .collect()
});

/// Common SQL type names used after `::` casts.
static SQL_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "TEXT",
        "INT",
        "INTEGER",
        "BIGINT",
        "SMALLINT",
        "NUMERIC",
        "DECIMAL",
        "REAL",
        "FLOAT",
        "DOUBLE",
        "BOOLEAN",
        "BOOL",
        "DATE",
        "TIME",
        "TIMESTAMP",
        "TIMESTAMPTZ",
        "INTERVAL",
        "UUID",
        "JSONB",
        "JSON",
        "BYTEA",
        "VARCHAR",
        "CHAR",
        "CHARACTER",
    ]
    .into_iter()
    .collect()
});

/// Regex matching a double-quoted identifier: `"something"`.
static DQUOTE_IDENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]+)""#).unwrap());

/// Regex matching a bare identifier: letters/underscore start, then
/// letters/digits/underscore. Does NOT match numbers-only.
static BARE_IDENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z_][a-z0-9_]*)\b").unwrap());

/// Extract column-like identifiers from an expression.
///
/// Returns bare or double-quoted names that:
/// - Are not SQL keywords, function names, or type names
/// - Are not immediately followed by `(` (function call)
/// - Are not immediately preceded by `::` (type cast)
///
/// This is heuristic — it may miss some references or include false
/// positives, but is good enough for a warning pass.
pub fn extract_identifiers(expr: &str) -> Vec<String> {
    let stripped = strip_string_literals(expr);
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    // 1. Double-quoted identifiers — always column references
    for cap in DQUOTE_IDENT_RE.captures_iter(&stripped) {
        let name = cap[1].to_string();
        if seen.insert(name.clone()) {
            result.push(name);
        }
    }

    // 2. Bare identifiers — filter out keywords, functions, types
    // Remove double-quoted spans so we don't re-extract their contents
    let no_dquotes = DQUOTE_IDENT_RE.replace_all(&stripped, "\"\"");

    for m in BARE_IDENT_RE.find_iter(&no_dquotes) {
        let word = m.as_str();
        let upper = word.to_uppercase();

        // Skip keywords, functions, types
        if SQL_KEYWORDS.contains(upper.as_str()) {
            continue;
        }
        if SQL_FUNCTIONS.contains(upper.as_str()) {
            continue;
        }
        if SQL_TYPES.contains(upper.as_str()) {
            continue;
        }

        // Skip if followed by `(` — it's a function call
        let after = &no_dquotes[m.end()..];
        let after_trimmed = after.trim_start();
        if after_trimmed.starts_with('(') {
            continue;
        }

        // Skip if preceded by `::` — it's a type cast
        let before = &no_dquotes[..m.start()];
        if before.ends_with("::") {
            continue;
        }

        // Skip the `r` prefix in `r."field"` (table alias, not a column)
        if word == "r" && after_trimmed.starts_with('.') {
            continue;
        }

        if seen.insert(word.to_string()) {
            result.push(word.to_string());
        }
    }

    result
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_function_call() {
        assert!(
            validate_expression("SPLIT_PART(name, ' ', 1)", ExprContext::ForwardExpression).is_ok()
        );
    }

    #[test]
    fn accepts_cast() {
        assert!(
            validate_expression("TO_DATE(dob, 'DD/MM/YY')", ExprContext::ForwardExpression).is_ok()
        );
    }

    #[test]
    fn accepts_concatenation() {
        assert!(validate_expression(
            "first_name || ' ' || last_name",
            ExprContext::ReverseExpression
        )
        .is_ok());
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
        assert!(validate_expression(
            "COALESCE(phone, '__CLEARED__')",
            ExprContext::ForwardExpression
        )
        .is_ok());
    }

    #[test]
    fn accepts_is_not_null() {
        assert!(
            validate_expression("deleted_at IS NOT NULL", ExprContext::ForwardExpression).is_ok()
        );
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
        )
        .is_ok());
    }

    #[test]
    fn accepts_max_aggregate() {
        assert!(validate_expression("max(price)", ExprContext::TargetExpression).is_ok());
    }

    #[test]
    fn accepts_nullif() {
        assert!(validate_expression(
            "NULLIF(phone, '__CLEARED__')",
            ExprContext::ReverseExpression
        )
        .is_ok());
    }

    #[test]
    fn accepts_default_expression() {
        assert!(validate_expression(
            "SPLIT_PART(full_name, ' ', 1)",
            ExprContext::DefaultExpression
        )
        .is_ok());
    }

    #[test]
    fn accepts_regex_replace() {
        assert!(validate_expression(
            "REGEXP_REPLACE(phone_number, '[^0-9]', '', 'g')",
            ExprContext::ForwardExpression
        )
        .is_ok());
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
        let err = validate_expression("name; DROP TABLE users", ExprContext::ForwardExpression)
            .unwrap_err();
        assert!(err.contains("semicolon"), "{err}");
    }

    #[test]
    fn rejects_select() {
        let err = validate_expression(
            "(SELECT min(phone) FROM phones)",
            ExprContext::ReverseExpression,
        )
        .unwrap_err();
        assert!(err.contains("SELECT"), "{err}");
    }

    #[test]
    fn rejects_drop() {
        let err =
            validate_expression("DROP TABLE users", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("DROP"), "{err}");
    }

    #[test]
    fn rejects_insert() {
        let err = validate_expression("INSERT INTO t VALUES (1)", ExprContext::ForwardExpression)
            .unwrap_err();
        assert!(err.contains("INSERT"), "{err}");
    }

    #[test]
    fn rejects_internal_view_fwd() {
        let err =
            validate_expression("_fwd_contact.name", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("_fwd_"), "{err}");
    }

    #[test]
    fn rejects_internal_view_resolved() {
        let err = validate_expression(
            r#"(SELECT min("phone") FROM "_resolved_phone_entry")"#,
            ExprContext::ReverseExpression,
        )
        .unwrap_err();
        // Could match SELECT first or _resolved_, both are violations
        assert!(
            err.contains("SELECT") || err.contains("_resolved_"),
            "{err}"
        );
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
        let err =
            validate_expression("name ORDER BY id", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("ORDER"), "{err}");
    }

    #[test]
    fn rejects_delta_prefix() {
        let err =
            validate_expression("_delta_contact.name", ExprContext::ForwardExpression).unwrap_err();
        assert!(err.contains("_delta_"), "{err}");
    }

    #[test]
    fn accepts_escaped_quotes() {
        assert!(validate_expression("name = 'it''s'", ExprContext::Filter).is_ok());
    }

    #[test]
    fn rejects_grant() {
        let err = validate_expression("GRANT ALL ON t TO public", ExprContext::ForwardExpression)
            .unwrap_err();
        assert!(err.contains("GRANT"), "{err}");
    }

    // ── Identifier extraction tests ──────────────────────────────────

    #[test]
    fn extract_simple_column() {
        let ids = extract_identifiers("name");
        assert_eq!(ids, vec!["name"]);
    }

    #[test]
    fn extract_function_args_not_function_name() {
        let ids = extract_identifiers("SPLIT_PART(name, ' ', 1)");
        assert_eq!(ids, vec!["name"]);
    }

    #[test]
    fn extract_multiple_columns() {
        let ids = extract_identifiers("first_name || ' ' || last_name");
        assert!(ids.contains(&"first_name".to_string()));
        assert!(ids.contains(&"last_name".to_string()));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn extract_skips_keywords() {
        let ids = extract_identifiers("deleted_at IS NOT NULL");
        assert_eq!(ids, vec!["deleted_at"]);
    }

    #[test]
    fn extract_skips_type_casts() {
        let ids = extract_identifiers("amount::numeric");
        assert_eq!(ids, vec!["amount"]);
    }

    #[test]
    fn extract_quoted_identifier() {
        let ids = extract_identifiers(r#""first_name" || ' ' || "last_name""#);
        assert!(ids.contains(&"first_name".to_string()));
        assert!(ids.contains(&"last_name".to_string()));
    }

    #[test]
    fn extract_skips_string_literal_contents() {
        let ids = extract_identifiers("contact_type = 'person'");
        assert_eq!(ids, vec!["contact_type"]);
    }

    #[test]
    fn extract_aggregate_gets_inner_column() {
        let ids = extract_identifiers("string_agg(distinct type, ',' order by type)");
        // "type" appears twice but should be deduplicated
        assert_eq!(ids, vec!["type"]);
    }

    #[test]
    fn extract_coalesce_args() {
        let ids = extract_identifiers("COALESCE(phone, '__CLEARED__')");
        assert_eq!(ids, vec!["phone"]);
    }

    #[test]
    fn extract_comparison_filter() {
        let ids = extract_identifiers("is_primary = 'true'");
        assert_eq!(ids, vec!["is_primary"]);
    }

    #[test]
    fn extract_nothing_from_literal() {
        let ids = extract_identifiers("'employee'");
        assert!(ids.is_empty());
    }

    #[test]
    fn extract_nothing_from_boolean() {
        let ids = extract_identifiers("true");
        assert!(ids.is_empty());
    }

    #[test]
    fn extract_complex_expression() {
        let ids = extract_identifiers(
            "'+' || SUBSTRING(phone, 1, 1) || ' ' || SUBSTRING(phone, 2, 3) || '-' || SUBSTRING(phone, 5)"
        );
        assert_eq!(ids, vec!["phone"]);
    }

    #[test]
    fn extract_r_dot_alias_skipped() {
        // In reverse expressions, r."field" is a table-qualified reference.
        // The `r` should be skipped; the quoted field name is extracted.
        let ids = extract_identifiers(r#"r."email" || '@' || r."domain""#);
        assert!(ids.contains(&"email".to_string()));
        assert!(ids.contains(&"domain".to_string()));
        assert!(!ids.contains(&"r".to_string()));
    }

    #[test]
    fn extract_deduplicates() {
        let ids = extract_identifiers("name = name");
        assert_eq!(ids, vec!["name"]);
    }
}
