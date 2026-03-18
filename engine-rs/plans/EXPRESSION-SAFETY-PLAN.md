# Expression safety

**Status:** Done

Validate that user-provided expressions are safe column-level SQL snippets.
Prevent leaking internal view names and coupling mappings to engine internals.

## Problem

Every expression type in the mapping YAML is interpolated verbatim into
generated SQL with zero validation:

| Expression type | SQL position | Example |
|-----------------|-------------|---------|
| `expression:` (field) | SELECT column | `SPLIT_PART(name, ' ', 1)::text AS "first_name"` |
| `reverse_expression:` | SELECT column | `first_name \|\| ' ' \|\| last_name AS "name"` |
| `filter:` | WHERE clause | `WHERE contact_type = 'person'` |
| `reverse_filter:` | CASE WHEN | `WHEN (is_deleted IS NOT TRUE) IS NOT TRUE` |
| `default_expression:` | COALESCE arg | `COALESCE("first_name", SPLIT_PART(full_name, ' ', 1))` |
| `expression:` (target) | Aggregation | `string_agg(distinct type, ',' order by type)` |
| `last_modified.expression:` | SELECT column | `(updated_at) AS _last_modified` |

This creates three problems:

**1. Internal coupling.** The multi-value pattern requires `reverse_expression`
subqueries against internal view names like `"_resolved_phone_entry"` and the
`r."email"` alias. If the naming convention changes, mappings break silently.

**2. Correctness risk.** Unbalanced parentheses, typos in column names, or
accidentally referencing columns from the wrong scope are only caught at
PostgreSQL runtime — after all views are deployed.

**3. No guardrails.** Nothing prevents DDL (`DROP TABLE`), multi-statement
injection (`;`), or subqueries against arbitrary tables. While the mapping
author is trusted, expressions should be limited to their intended purpose:
column-level SQL transforms.

## Design principles

1. Expressions are **column-level SQL snippets** — scalar transforms of
   available columns using functions, operators, casts, and literals.
2. Cross-target access (subqueries against resolved views) is **not** an
   expression concern — it needs a dedicated engine feature.
3. Validation should catch mistakes early (compile time) rather than late
   (PostgreSQL deploy time).
4. Existing valid expressions must not break — the validation must accept
   everything in the current examples.

## What a "safe snippet" is

A column-level SQL snippet may contain:

- **Column references**: unquoted or double-quoted identifiers (`name`,
  `"first_name"`, `_base->>'phone'`)
- **Literals**: strings (`'person'`), numbers (`42`, `3.14`), booleans
  (`true`, `false`), NULL
- **Operators**: arithmetic (`+`, `-`, `*`, `/`), comparison (`=`, `!=`,
  `<`, `>`, `<=`, `>=`, `<>`, `IS`, `IS NOT`, `LIKE`, `ILIKE`),
  logical (`AND`, `OR`, `NOT`), string (`||`), JSONB (`->`, `->>`),
  type cast (`::`)
- **Function calls**: `SPLIT_PART(...)`, `TO_DATE(...)`, `COALESCE(...)`,
  `REGEXP_REPLACE(...)`, `SUBSTRING(...)`, `round(...)`, etc.
- **Aggregate calls** (target expressions only): `min(...)`, `max(...)`,
  `string_agg(...)`, `bool_or(...)`, `avg(...)`, `array_agg(...)`
- **CASE expressions**: `CASE WHEN ... THEN ... ELSE ... END`
- **ORDER BY / DISTINCT inside aggregates**: `string_agg(distinct x, ',' order by x)`

A column-level SQL snippet may **not** contain:

- **Subqueries**: `(SELECT ... FROM ...)`
- **FROM / JOIN / WHERE / GROUP BY / HAVING / LIMIT** as standalone clauses
- **Semicolons**: `;`
- **DDL keywords**: `CREATE`, `DROP`, `ALTER`, `TRUNCATE`
- **DML keywords**: `INSERT`, `UPDATE`, `DELETE`
- **Transaction control**: `BEGIN`, `COMMIT`, `ROLLBACK`
- **System access**: `pg_catalog`, `information_schema`, `pg_read_file`
- **Internal view references**: `_fwd_*`, `_id_*`, `_resolved_*`, `_rev_*`, `_delta_*`

## Phase 1 — Static snippet validation (compile-time)

### Implementation

Add a `validate_expression(expr: &str, context: ExprContext) -> Result<()>`
function in a new `engine-rs/src/validate_expr.rs` module. Called during the
existing validation pass on the parsed mapping document.

```rust
enum ExprContext {
    /// expression: on field mapping — source columns available
    ForwardExpression,
    /// reverse_expression: on field mapping — target fields + r.alias
    ReverseExpression,
    /// filter: on mapping — source columns available
    Filter,
    /// reverse_filter: on mapping — target fields available
    ReverseFilter,
    /// default_expression: on target field — target fields available
    DefaultExpression,
    /// expression: on target field (strategy: expression) — aggregation context
    TargetExpression,
}
```

### Validation rules

**All contexts:**
1. Reject if contains `;`
2. Reject if contains prohibited keywords (case-insensitive word-boundary
   match): `SELECT`, `FROM`, `JOIN`, `WHERE`, `GROUP`, `HAVING`, `LIMIT`,
   `INSERT`, `UPDATE`, `DELETE`, `CREATE`, `DROP`, `ALTER`, `TRUNCATE`,
   `BEGIN`, `COMMIT`, `ROLLBACK`, `GRANT`, `REVOKE`, `COPY`, `EXECUTE`
3. Reject if contains internal view name pattern: `_fwd_`, `_id_`, `_resolved_`,
   `_rev_`, `_delta_`, `_grp_`
4. Check balanced parentheses
5. Check balanced single quotes (basic — not a full SQL parser)

**Aggregate context** (`TargetExpression`):
- Exempt keywords `ORDER` and `DISTINCT` (needed inside `string_agg(distinct ..., ',' order by ...)`)

**`_base` references** (delta context):
- `_base->>'field'` is only valid inside `normalize` expressions (Phase 2 of
  PRECISION-LOSS-PLAN). Disallow in all other contexts.

### Keyword detection

Use word-boundary matching to avoid false positives:

```rust
fn contains_prohibited_keyword(expr: &str, exempt: &[&str]) -> Option<String> {
    let prohibited = [
        "SELECT", "FROM", "JOIN", "WHERE", "GROUP", "HAVING", "LIMIT",
        "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "BEGIN", "COMMIT", "ROLLBACK", "GRANT", "REVOKE", "COPY", "EXECUTE",
    ];
    // Strip string literals first to avoid matching keywords inside 'strings'
    let stripped = strip_string_literals(expr);
    for kw in &prohibited {
        if exempt.contains(kw) { continue; }
        // Word boundary: preceded by start/space/paren, followed by end/space/paren
        let pattern = format!(r"(?i)\b{}\b", kw);
        if Regex::new(&pattern).unwrap().is_match(&stripped) {
            return Some(kw.to_string());
        }
    }
    None
}
```

### False positive mitigation

Keywords inside string literals must not trigger rejection. Strip `'...'`
content before keyword scanning:

```rust
fn strip_string_literals(expr: &str) -> String {
    // Replace 'anything' with '' (empty string literal)
    // Handle escaped quotes ('it''s') by matching pairs
    LITERAL_RE.replace_all(expr, "''").to_string()
}
```

### Error messages

```
error: expression contains prohibited keyword 'SELECT'
  --> mapping.yaml:47
   |
47 |         reverse_expression: >
   |           (SELECT min("phone") FROM "_resolved_phone_entry" ...)
   |
   = help: expressions must be column-level SQL snippets
   = help: for cross-target access, use 'lookup:' instead (see MULTI-VALUE-PLAN)
```

## Phase 2 — Column reference validation

After Phase 1 (syntactic safety), validate that referenced columns actually
exist in the expression's scope.

### Available columns per context

| Context | Available columns |
|---------|------------------|
| `ForwardExpression` | Source table columns from `source.dataset` |
| `ReverseExpression` | Target field names (unquoted or `r."field"`) |
| `Filter` | Source table columns |
| `ReverseFilter` | Target field names |
| `DefaultExpression` | Other target field names |
| `TargetExpression` | Target field names contributed by mappings |

### Approach

Extract identifiers from the expression (double-quoted or bare words that
aren't SQL keywords / function names / types) and check them against the
available column set. Warn on unrecognized identifiers rather than error —
the column set may be incomplete (source columns aren't declared in the YAML
unless `source.fields:` is provided).

Phase 2 is optional — it's a warning pass, not a hard gate.

## Migration path

Phase 1 shipped with validation on — existing examples already pass
(all current expressions are safe snippets). No migration needed.

Cross-target access (`lookup:`) was originally planned as Phase 3 but is
superseded by the `from:` / `_enriched_` approach in
[COMPUTED-FIELDS-PLAN](COMPUTED-FIELDS-PLAN.md), which is more declarative
and integrates cleanly with the pipeline.

## Scope of changes

### Phase 1 (validation) — Done
- New file: `engine-rs/src/validate_expr.rs`
- Modified: `engine-rs/src/validate.rs` — calls `validate_expression()` for each expression field
- Modified: `engine-rs/Cargo.toml` — `regex` dependency added

## Interaction with other plans

- **MULTI-VALUE-PLAN**: Now uses `primary_phone` coalesce field instead of
  cross-target subqueries. `lookup:` remains available for edge cases that
  genuinely need cross-target access.
- **COMPUTED-FIELDS-PLAN**: Cross-target access (`lookup:`) is superseded by
  the `from:` / `_enriched_` layer defined there.
- **PRECISION-LOSS-PLAN**: `normalize` expressions are column-level snippets
  with a `%s` placeholder — Phase 1 validation applies (validate after
  placeholder substitution).
- **PROPAGATED-DELETE-PLAN**: Uses `expression: "deleted_at IS NOT NULL"` and
  `reverse_filter: "is_deleted IS NOT TRUE"` — both are valid snippets.
