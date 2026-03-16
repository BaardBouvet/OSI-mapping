pub mod model;
pub mod parser;
pub mod validate;
pub mod validate_expr;
pub mod dag;
pub mod render;
pub mod error;

/// Quote a SQL identifier with double quotes (PostgreSQL standard).
/// Escapes embedded double-quotes by doubling them.
pub fn qi(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Escape a string for use inside a SQL single-quoted literal.
/// Doubles any embedded single quotes: `it's` → `it''s`.
pub fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}
