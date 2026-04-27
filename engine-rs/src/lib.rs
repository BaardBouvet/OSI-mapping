//! OSI mapping reference engine — v2 rewrite.
//!
//! Renders v2 mappings to a DAG of PostgreSQL views. The SPARQL backend
//! is validated separately (see `sparql-spike/`) and will follow once the
//! SQL renderer is at conformance parity.

pub mod model;
pub mod parser;
pub mod render;

/// Quote a SQL identifier with double quotes (PostgreSQL standard).
pub fn qi(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Escape a string for inclusion in a SQL single-quoted literal.
pub fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}
