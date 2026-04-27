//! Backend renderers for v2 mappings.
//!
//! Two backends, one schema (the v2 conformance contract):
//!
//! - `sql` — emits PostgreSQL DDL (a static text artifact). The harness
//!   applies the DDL to a Postgres instance, populates source tables,
//!   and reads the delta views.
//! - `sparql` — emits a `SparqlPlan` plus an executor. The harness loads
//!   input rows into an Oxigraph in-memory store, runs the plan, and
//!   reads the delta projections.
//!
//! Both backends must produce identical `updates`/`inserts`/`deletes` for
//! every conformance example.

pub mod framing;
pub mod sparql;
mod sql;

pub use sparql::{render_sparql, render_sparql_with_base, SparqlPlan};
pub use sql::render_pg;
