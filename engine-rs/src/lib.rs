pub mod model;
pub mod parser;
pub mod validate;
pub mod dag;
pub mod render;
pub mod error;

/// Quote a SQL identifier with double quotes (PostgreSQL standard).
/// Escapes embedded double-quotes by doubling them.
pub fn qi(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
