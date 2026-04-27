//! v2 conformance test runner.
//!
//! Each example is run against every available backend (PG views via
//! testcontainers, SPARQL via Oxigraph in-process). Both backends must
//! produce identical `updates`/`inserts`/`deletes` for every test case —
//! that is the v2 conformance contract.

use indexmap::IndexMap;
use serde_yaml::Value as Yaml;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio_postgres::{types::ToSql, NoTls};

use osi_engine::model::{Doc, Mapping, Source, Test};

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("examples")
}

// ---------------------------------------------------------------------------
// Normalised row form: BTreeMap<column, Option<text>>, used for unordered eq.
// ---------------------------------------------------------------------------

type NormRow = BTreeMap<String, Option<String>>;

#[derive(Debug, Default, Clone)]
struct Deltas {
    /// source name → kind ("updates"/"inserts"/"deletes") → rows.
    by_source: BTreeMap<String, BTreeMap<String, Vec<NormRow>>>,
}

fn yaml_to_text(v: &Yaml) -> Option<String> {
    match v {
        Yaml::Null => None,
        Yaml::Bool(b) => Some(b.to_string()),
        Yaml::Number(n) => Some(n.to_string()),
        Yaml::String(s) => Some(s.clone()),
        Yaml::Sequence(_) | Yaml::Mapping(_) => {
            // Canonicalise compound values to JSON so they can be
            // compared against jsonb / canonical-JSON strings emitted
            // by the backends.
            let json: serde_json::Value =
                serde_yaml::from_value(v.clone()).unwrap_or(serde_json::Value::Null);
            Some(osi_engine::render::sparql::canonical_json_string(&json))
        }
        _ => Some(serde_yaml::to_string(v).unwrap_or_default()),
    }
}

fn discover_columns(rows: &[Yaml]) -> Vec<String> {
    let mut seen = Vec::new();
    for row in rows {
        if let Yaml::Mapping(m) = row {
            for (k, _v) in m {
                if let Yaml::String(s) = k {
                    if !seen.iter().any(|c| c == s) {
                        seen.push(s.clone());
                    }
                }
            }
        }
    }
    seen
}

fn yaml_row_to_norm(row: &Yaml) -> NormRow {
    let mut out = BTreeMap::new();
    if let Yaml::Mapping(m) = row {
        for (k, v) in m {
            if let Yaml::String(name) = k {
                out.insert(name.clone(), yaml_to_text(v));
            }
        }
    }
    out
}

/// Drop synthetic columns that some backends emit as instrumentation but
/// the conformance contract doesn't assert on.
fn strip_synthetic(row: &mut NormRow) {
    row.remove("canonical_id");
    row.remove("_src_pk");
    row.remove("_canonical_id");
}

/// Convert expected outcomes to NormRows. Only inserts carry `_canonical_id`;
/// updates and deletes don't, so we strip it from those.
fn expected_to_norm(test: &Test) -> Deltas {
    let mut by_source: BTreeMap<String, BTreeMap<String, Vec<NormRow>>> = BTreeMap::new();
    for (src, exp) in &test.expected {
        let mut kinds: BTreeMap<String, Vec<NormRow>> = BTreeMap::new();
        for r in &exp.updates {
            let mut n = yaml_row_to_norm(r);
            n.remove("_canonical_id");
            kinds.entry("updates".to_string()).or_default().push(n);
        }
        for r in &exp.inserts {
            // Keep _canonical_id presence as a marker but its actual value
            // is backend-dependent — strip the value for comparison and
            // re-insert with sentinel.
            let mut n = yaml_row_to_norm(r);
            n.remove("_canonical_id");
            kinds.entry("inserts".to_string()).or_default().push(n);
        }
        for r in &exp.deletes {
            let mut n = yaml_row_to_norm(r);
            n.remove("_canonical_id");
            kinds.entry("deletes".to_string()).or_default().push(n);
        }
        by_source.insert(src.clone(), kinds);
    }
    Deltas { by_source }
}

/// If a column value parses as JSON, normalise it to canonical
/// (key-sorted, whitespace-free) form so PG and SPARQL backends agree
/// regardless of internal jsonb formatting.
fn canonicalise_jsonish(row: &mut NormRow) {
    for (_k, v) in row.iter_mut() {
        let Some(text) = v.as_ref() else { continue };
        let trimmed = text.trim_start();
        if trimmed.starts_with('[') || trimmed.starts_with('{') {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
                *v = Some(osi_engine::render::sparql::canonical_json_string(&parsed));
            }
        }
    }
}

fn assert_deltas_match(label: &str, expected: &Deltas, actual: &Deltas) {
    for (src, kinds) in &expected.by_source {
        let actual_kinds = actual.by_source.get(src).cloned().unwrap_or_default();
        for (kind, exp_rows) in kinds {
            let mut act_rows: Vec<NormRow> = actual_kinds.get(kind).cloned().unwrap_or_default();
            for r in act_rows.iter_mut() {
                strip_synthetic(r);
                r.remove("_canonical_id");
            }
            let mut e = exp_rows.clone();
            e.sort_by_key(|m| format!("{m:?}"));
            act_rows.sort_by_key(|m| format!("{m:?}"));
            assert_eq!(
                e, act_rows,
                "\n[{label}] {src}.{kind} differ\nexpected: {e:#?}\nactual:   {act_rows:#?}",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// PG backend runner
// ---------------------------------------------------------------------------

async fn run_pg_backend(doc: &Doc, test: &Test) -> anyhow::Result<Deltas> {
    let sql_ddl = osi_engine::render::render_pg(doc)?;
    let container = Postgres::default().start().await?;
    let host = container.get_host().await?;
    let port = container.get_host_port_ipv4(5432).await?;
    let conn_str =
        format!("host={host} port={port} user=postgres password=postgres dbname=postgres");
    let (client, connection) = tokio_postgres::connect(&conn_str, NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("conn error: {e}");
        }
    });

    // 1. Source tables.
    for (src_name, src) in &doc.sources {
        let rows = test.input.get(src_name).cloned().unwrap_or_default();
        let columns = discover_columns(&rows);
        // Per-column type inference: if any cell in the input data is a
        // YAML sequence or mapping, the column gets `jsonb`; otherwise
        // `text`. This lets nested-array mappings declare their array
        // column natively without forcing every example to spell it out.
        let col_types: Vec<&'static str> = columns
            .iter()
            .map(|c| {
                if rows.iter().any(|row| {
                    if let Yaml::Mapping(m) = row {
                        matches!(
                            m.get(Yaml::String(c.clone())),
                            Some(Yaml::Sequence(_)) | Some(Yaml::Mapping(_))
                        )
                    } else {
                        false
                    }
                }) {
                    "jsonb"
                } else {
                    "text"
                }
            })
            .collect();
        if columns.is_empty() {
            let pk = &src.primary_key;
            let stmt = format!("CREATE TABLE \"{src_name}\" (\"{pk}\" text);");
            client.batch_execute(&stmt).await?;
            continue;
        }
        let cols_sql: Vec<String> = columns
            .iter()
            .zip(col_types.iter())
            .map(|(c, t)| format!("\"{c}\" {t}"))
            .collect();
        let stmt = format!("CREATE TABLE \"{src_name}\" ({});", cols_sql.join(", "));
        client.batch_execute(&stmt).await?;
        for row in &rows {
            let Yaml::Mapping(m) = row else { continue };
            let mut col_names = Vec::new();
            let mut placeholders = Vec::new();
            let mut values: Vec<Option<String>> = Vec::new();
            let mut idx = 1;
            for (c, t) in columns.iter().zip(col_types.iter()) {
                let v = m.get(Yaml::String(c.clone()));
                col_names.push(format!("\"{c}\""));
                // Always send the parameter as `text` and let the server
                // cast (jsonb columns get a second `::jsonb`). This avoids
                // tokio-postgres's strict client-side type matching for
                // jsonb-typed parameters.
                let cast = if *t == "jsonb" {
                    "::text::jsonb"
                } else {
                    "::text"
                };
                placeholders.push(format!("${idx}{cast}"));
                let text = match v {
                    None | Some(Yaml::Null) => None,
                    Some(Yaml::Sequence(_)) | Some(Yaml::Mapping(_)) => {
                        let json: serde_json::Value = serde_yaml::from_value(v.unwrap().clone())?;
                        Some(serde_json::to_string(&json)?)
                    }
                    Some(other) => yaml_to_text(other),
                };
                values.push(text);
                idx += 1;
            }
            let stmt = format!(
                "INSERT INTO \"{src_name}\" ({}) VALUES ({});",
                col_names.join(", "),
                placeholders.join(", ")
            );
            let params: Vec<&(dyn ToSql + Sync)> =
                values.iter().map(|v| v as &(dyn ToSql + Sync)).collect();
            client.execute(&stmt, &params).await?;
        }
    }

    // 2. DDL.
    client.batch_execute(&sql_ddl).await?;

    // 3. Read delta views.
    //    Expected-deltas keys may be either a mapping name or a source name.
    //    Mapping name is canonical — child mappings share a source with their
    //    parent so source-based lookup is ambiguous. Try mapping name first,
    //    then fall back to source.
    let mut deltas = Deltas::default();
    for key in test.expected.keys() {
        let mapping = doc
            .mappings
            .iter()
            .find(|m| m.name == *key)
            .or_else(|| doc.mappings.iter().find(|m| m.source == *key))
            .ok_or_else(|| anyhow::anyhow!("no mapping matching expected key `{key}`"))?;
        let mut kinds: BTreeMap<String, Vec<NormRow>> = BTreeMap::new();
        for kind in ["updates", "inserts", "deletes"] {
            let view = format!("{}_{kind}", mapping.name);
            let rows = client
                .query(&format!("SELECT * FROM \"{view}\""), &[])
                .await?;
            let mut out = Vec::new();
            for row in rows {
                let mut m = NormRow::new();
                for (i, col) in row.columns().iter().enumerate() {
                    let type_name = col.type_().name();
                    let val: Option<String> = if type_name == "jsonb" || type_name == "json" {
                        let v: Option<serde_json::Value> = row.try_get(i).ok().flatten();
                        v.map(|jv| osi_engine::render::sparql::canonical_json_string(&jv))
                    } else {
                        row.try_get::<_, Option<String>>(i).ok().flatten()
                    };
                    m.insert(col.name().to_string(), val);
                }
                canonicalise_jsonish(&mut m);
                out.push(m);
            }
            kinds.insert(kind.to_string(), out);
        }
        deltas.by_source.insert(key.clone(), kinds);
    }
    Ok(deltas)
}

// ---------------------------------------------------------------------------
// SPARQL backend runner
// ---------------------------------------------------------------------------

fn run_sparql_backend(doc: &Doc, test: &Test) -> anyhow::Result<Deltas> {
    let plan = osi_engine::render::render_sparql(doc)?;

    // Convert test.input → HashMap<String, Vec<Row>> where Row = IndexMap<...>.
    let mut inputs: HashMap<String, Vec<osi_engine::render::sparql::Row>> = HashMap::new();
    for (src, rows) in &test.input {
        let mut converted = Vec::new();
        for row in rows {
            let mut r: osi_engine::render::sparql::Row = IndexMap::new();
            if let Yaml::Mapping(m) = row {
                for (k, v) in m {
                    if let Yaml::String(name) = k {
                        r.insert(name.clone(), v.clone());
                    }
                }
            }
            converted.push(r);
        }
        inputs.insert(src.clone(), converted);
    }

    let raw = plan.execute(&inputs)?;

    let mut deltas = Deltas::default();
    for key in test.expected.keys() {
        // Expected keys are mapping names (preferred) or source names
        // (legacy / single-mapping examples). The plan keys deltas by
        // mapping name; resolve the source-name case by finding the
        // unique mapping with that source.
        let mapping_name = if doc.mappings.iter().any(|m| m.name == *key) {
            key.clone()
        } else {
            doc.mappings
                .iter()
                .find(|m| m.source == *key)
                .map(|m| m.name.clone())
                .ok_or_else(|| anyhow::anyhow!("no mapping matching expected key `{key}`"))?
        };
        let mut kinds: BTreeMap<String, Vec<NormRow>> = BTreeMap::new();
        let row_to_norm = |r: &osi_engine::render::sparql::Row| -> NormRow {
            let mut out = NormRow::new();
            for (k, v) in r {
                out.insert(k.clone(), yaml_to_text(v));
            }
            out
        };
        let updates = raw.updates.get(&mapping_name).cloned().unwrap_or_default();
        let inserts = raw.inserts.get(&mapping_name).cloned().unwrap_or_default();
        let deletes = raw.deletes.get(&mapping_name).cloned().unwrap_or_default();
        kinds.insert(
            "updates".to_string(),
            updates.iter().map(row_to_norm).collect(),
        );
        kinds.insert(
            "inserts".to_string(),
            inserts.iter().map(row_to_norm).collect(),
        );
        kinds.insert(
            "deletes".to_string(),
            deletes.iter().map(row_to_norm).collect(),
        );
        deltas.by_source.insert(key.clone(), kinds);
    }
    Ok(deltas)
}

// ---------------------------------------------------------------------------
// Test entry point
// ---------------------------------------------------------------------------

async fn run_example(name: &str) -> anyhow::Result<()> {
    let mapping_path = examples_dir().join(name).join("mapping.yaml");
    let doc = osi_engine::parser::parse_file(&mapping_path)?;
    for (i, test) in doc.tests.iter().enumerate() {
        eprintln!("--- {name} test {i}: {} ---", test.description);
        let expected = expected_to_norm(test);

        // PG backend.
        let pg_actual = run_pg_backend(&doc, test).await?;
        assert_deltas_match("pg", &expected, &pg_actual);
        eprintln!("    pg: PASS");

        // SPARQL backend.
        let sparql_actual = run_sparql_backend(&doc, test)?;
        assert_deltas_match("sparql", &expected, &sparql_actual);
        eprintln!("    sparql: PASS");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_world() -> anyhow::Result<()> {
    run_example("hello-world").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composite_identity() -> anyhow::Result<()> {
    run_example("composite-identity").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_modified() -> anyhow::Result<()> {
    run_example("last-modified").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_arrays_shallow() -> anyhow::Result<()> {
    run_example("nested-arrays-shallow").await
}

// Quiet warnings about unused imports when individual paths aren't directly
// referenced by helper functions; keeps cargo clippy happy.
#[allow(dead_code)]
fn _silence_unused(_s: Source, _m: Mapping) {}
