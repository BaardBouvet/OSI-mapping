//! Integration tests: load example mappings, render to SQL, execute against
//! a real PostgreSQL instance via testcontainers, and compare with expected output.

use std::path::PathBuf;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio_postgres::NoTls;

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("examples")
}

/// Discover all example directories that have tests defined.
fn discover_test_examples() -> Vec<(String, PathBuf)> {
    let examples = examples_dir();
    let mut results = Vec::new();

    for entry in std::fs::read_dir(&examples).expect("examples dir exists") {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let mapping_path = entry.path().join("mapping.yaml");
        if !mapping_path.exists() {
            continue;
        }
        // Quick check: does the file contain a tests section?
        let content = std::fs::read_to_string(&mapping_path).unwrap();
        if content.contains("\ntests:") || content.contains("\ntests :") {
            let name = entry.file_name().to_string_lossy().to_string();
            results.push((name, mapping_path));
        }
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

/// Parse all examples without error (basic smoke test - no Postgres needed).
#[test]
fn parse_all_examples() {
    let examples = examples_dir();
    let mut count = 0;
    let mut failures = Vec::new();

    for entry in std::fs::read_dir(&examples).expect("examples dir") {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let mapping = entry.path().join("mapping.yaml");
        if !mapping.exists() {
            continue;
        }
        count += 1;
        let name = entry.file_name().to_string_lossy().to_string();
        match osi_engine::parser::parse_file(&mapping) {
            Ok(doc) => {
                assert_eq!(doc.version, "1.0", "{name}: version mismatch");
            }
            Err(e) => {
                failures.push(format!("{name}: {e:#}"));
            }
        }
    }

    assert!(count > 0, "No examples found");
    if !failures.is_empty() {
        panic!(
            "Failed to parse {}/{} examples:\n{}",
            failures.len(),
            count,
            failures.join("\n")
        );
    }
    eprintln!("Successfully parsed {count} examples");
}

/// Render all examples to SQL without error (no Postgres needed).
#[test]
fn render_all_examples() {
    let examples = examples_dir();
    let mut count = 0;
    let mut failures = Vec::new();

    for entry in std::fs::read_dir(&examples).expect("examples dir") {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let mapping_path = entry.path().join("mapping.yaml");
        if !mapping_path.exists() {
            continue;
        }
        count += 1;
        let name = entry.file_name().to_string_lossy().to_string();
        match osi_engine::parser::parse_file(&mapping_path) {
            Ok(doc) => {
                let dag = osi_engine::dag::build_dag(&doc);
                match osi_engine::render::render_sql(&doc, &dag) {
                    Ok(sql) => {
                        assert!(!sql.is_empty(), "{name}: empty SQL output");
                        assert!(
                            sql.contains("CREATE OR REPLACE VIEW"),
                            "{name}: no views generated"
                        );
                    }
                    Err(e) => {
                        failures.push(format!("{name}: render error: {e:#}"));
                    }
                }
            }
            Err(e) => {
                failures.push(format!("{name}: parse error: {e:#}"));
            }
        }
    }

    assert!(count > 0, "No examples found");
    if !failures.is_empty() {
        panic!(
            "Failed to render {}/{} examples:\n{}",
            failures.len(),
            count,
            failures.join("\n")
        );
    }
    eprintln!("Successfully rendered {count} examples to SQL");
}

/// Full end-to-end test: parse → render → execute against Postgres → compare output.
/// Requires Docker for testcontainers.
#[tokio::test]
async fn execute_hello_world() {
    // Start a Postgres container
    let container = Postgres::default()
        .start()
        .await
        .expect("Failed to start Postgres container");

    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let conn_str = format!(
        "host=127.0.0.1 port={host_port} user=postgres password=postgres dbname=postgres"
    );

    let (client, connection) = tokio_postgres::connect(&conn_str, NoTls)
        .await
        .expect("Failed to connect to Postgres");

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("Connection error: {e}");
        }
    });

    // Parse and render hello-world example
    let mapping_path = examples_dir().join("hello-world/mapping.yaml");
    let doc = osi_engine::parser::parse_file(&mapping_path).expect("parse hello-world");
    let dag = osi_engine::dag::build_dag(&doc);
    let sql = osi_engine::render::render_sql(&doc, &dag).expect("render hello-world");

    for (test_idx, test) in doc.tests.iter().enumerate() {
        let desc = test
            .description
            .as_deref()
            .unwrap_or("(unnamed)");
        eprintln!("\n--- Test {}: {desc} ---", test_idx + 1);

        // Create source tables from test input
        load_test_data(&client, &test.input).await;

        // Ensure cluster_members tables exist (may not be in test input)
        ensure_cluster_members_tables(&client, &doc, &test.input).await;

        // Execute the rendered SQL views
        for stmt in sql.split(';') {
            // Strip leading comment lines and whitespace
            let stmt: String = stmt
                .lines()
                .filter(|line| !line.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            let stmt = stmt.trim();
            if stmt.is_empty() || stmt == "BEGIN" || stmt == "COMMIT" {
                continue;
            }
            client.execute(stmt, &[]).await.unwrap_or_else(|e| {
                panic!("Failed to execute SQL:\n{stmt}\n\nError: {e}");
            });
        }

        // Compare reverse views with expected output
        for (dataset, expected) in &test.expected {
            let mapping = doc
                .mappings
                .iter()
                .find(|m| m.source.dataset == *dataset)
                .unwrap_or_else(|| panic!("No mapping for dataset {dataset}"));

            let rev_view = format!("_rev_{}", mapping.name);

            // Build the list of reverse-mapped source field names
            let reverse_fields: Vec<(String, String)> = mapping
                .fields
                .iter()
                .filter(|fm| fm.is_reverse() && fm.source.is_some())
                .map(|fm| (fm.source.clone().unwrap(), fm.target.clone().unwrap_or_default()))
                .collect();

            // Get all source columns (from test input)
            let source_columns: Vec<String> = {
                let mut cols = Vec::new();
                if let Some(rows) = test.input.get(dataset.as_str()) {
                    for row in rows {
                        if let Some(obj) = row.as_object() {
                            for key in obj.keys() {
                                if !cols.contains(key) {
                                    cols.push(key.clone());
                                }
                            }
                        }
                    }
                }
                cols
            };

            // Query reverse view
            let rev_rows = client
                .query(&format!("SELECT * FROM {rev_view}"), &[])
                .await
                .unwrap_or_else(|e| panic!("Failed to query {rev_view}: {e:?}"));

            // Build actual output by joining reverse view with source table
            let mut actual_updates: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();

            for rev_row in &rev_rows {
                // Skip insert rows (_src_id is NULL); they're verified separately.
                let src_id: Option<String> = rev_row.try_get("_src_id").ok().flatten();
                let src_id = match src_id {
                    Some(id) => id,
                    None => continue,
                };

                // Build PK-based WHERE clause for source row lookup
                let pk_where = if let Some(src_meta) = doc.sources.get(dataset.as_str()) {
                    let cols = src_meta.primary_key.columns();
                    if cols.len() == 1 {
                        format!("{} = $1", cols[0])
                    } else {
                        // Composite: _src_id is a JSONB text, parse it.
                        // For test purposes, fall back to scanning.
                        format!("_row_id::text = $1")
                    }
                } else {
                    "_row_id::text = $1".to_string()
                };

                // Fetch the source row
                let source_rows = client
                    .query(
                        &format!("SELECT * FROM {dataset} WHERE {pk_where}"),
                        &[&src_id],
                    )
                    .await
                    .unwrap();
                let source_row = &source_rows[0];

                let mut output = serde_json::Map::new();

                // Start with unmapped source columns
                for col in &source_columns {
                    let mapped = reverse_fields.iter().any(|(src, _)| src == col);
                    if !mapped {
                        // Use source value
                        let val: Option<String> = source_row.try_get(col.as_str()).ok().flatten();
                        output.insert(
                            col.clone(),
                            val.map(serde_json::Value::String)
                                .unwrap_or(serde_json::Value::Null),
                        );
                    }
                }
                // Override with reverse-mapped values
                for (src_field, _) in &reverse_fields {
                    let val: Option<String> =
                        rev_row.try_get(src_field.as_str()).ok().flatten();
                    output.insert(
                        src_field.clone(),
                        val.map(serde_json::Value::String)
                            .unwrap_or(serde_json::Value::Null),
                    );
                }

                actual_updates.push(output);
            }

            // Compare with expected updates
            let expected_updates: Vec<serde_json::Map<String, serde_json::Value>> = expected
                .updates
                .iter()
                .filter_map(|v| v.as_object().cloned())
                .collect();

            // Sort both for stable comparison
            let mut actual_sorted: Vec<String> = actual_updates
                .iter()
                .map(|m| serde_json::to_string(m).unwrap())
                .collect();
            actual_sorted.sort();

            let mut expected_sorted: Vec<String> = expected_updates
                .iter()
                .map(|m| {
                    // Normalize: convert all values to strings for comparison
                    let normalized: serde_json::Map<String, serde_json::Value> = m
                        .iter()
                        .map(|(k, v)| {
                            let str_val = match v {
                                serde_json::Value::String(s) => {
                                    serde_json::Value::String(s.clone())
                                }
                                serde_json::Value::Number(n) => {
                                    serde_json::Value::String(n.to_string())
                                }
                                serde_json::Value::Bool(b) => {
                                    serde_json::Value::String(b.to_string())
                                }
                                other => other.clone(),
                            };
                            (k.clone(), str_val)
                        })
                        .collect();
                    serde_json::to_string(&normalized).unwrap()
                })
                .collect();
            expected_sorted.sort();

            assert_eq!(
                actual_sorted.len(),
                expected_sorted.len(),
                "{dataset}: update count mismatch.\n  actual: {actual_sorted:?}\n  expected: {expected_sorted:?}"
            );
            for (actual, expected) in actual_sorted.iter().zip(expected_sorted.iter()) {
                assert_eq!(
                    actual, expected,
                    "{dataset}: row mismatch.\n  actual:   {actual}\n  expected: {expected}"
                );
            }
            eprintln!("{dataset}: {count} updates match ✓", count = actual_updates.len());

            // ── Insert verification ────────────────────────────────
            let expected_inserts: Vec<serde_json::Map<String, serde_json::Value>> = expected
                .inserts
                .iter()
                .filter_map(|v| v.as_object().cloned())
                .collect();

            if !expected_inserts.is_empty() {
                let delta_view = format!("_delta_{}", mapping.name);
                let insert_rows = client
                    .query(
                        &format!("SELECT * FROM {delta_view} WHERE _action = 'insert'"),
                        &[],
                    )
                    .await
                    .unwrap_or_else(|e| panic!("query {delta_view} inserts: {e}"));

                // Build actual insert maps: _cluster_id + business fields
                let target_name = mapping.target.name();
                let mut actual_inserts: Vec<serde_json::Map<String, serde_json::Value>> =
                    Vec::new();
                for row in &insert_rows {
                    let mut map = serde_json::Map::new();
                    let cluster_id: Option<String> = row.try_get("_cluster_id").ok().flatten();
                    map.insert(
                        "_cluster_id".into(),
                        cluster_id
                            .map(serde_json::Value::String)
                            .unwrap_or(serde_json::Value::Null),
                    );
                    // Include reverse-mapped business fields
                    for (src_field, _) in &reverse_fields {
                        let val: Option<String> = row.try_get(src_field.as_str()).ok().flatten();
                        map.insert(
                            src_field.clone(),
                            val.map(serde_json::Value::String)
                                .unwrap_or(serde_json::Value::Null),
                        );
                    }
                    actual_inserts.push(map);
                }

                // Resolve expected _cluster_id seeds: "mapping:src_id" → look up
                // _entity_id_resolved from the identity view.
                let mut expected_resolved: Vec<serde_json::Map<String, serde_json::Value>> =
                    Vec::new();
                for exp in &expected_inserts {
                    let mut resolved = serde_json::Map::new();
                    for (k, v) in exp {
                        if k == "_cluster_id" {
                            if let Some(seed) = v.as_str() {
                                let cluster_id =
                                    resolve_cluster_id(&client, seed, target_name).await;
                                resolved.insert(k.clone(), serde_json::Value::String(cluster_id));
                            } else {
                                resolved.insert(k.clone(), v.clone());
                            }
                        } else {
                            // Normalize to string
                            let str_val = match v {
                                serde_json::Value::String(s) => {
                                    serde_json::Value::String(s.clone())
                                }
                                serde_json::Value::Number(n) => {
                                    serde_json::Value::String(n.to_string())
                                }
                                serde_json::Value::Bool(b) => {
                                    serde_json::Value::String(b.to_string())
                                }
                                other => other.clone(),
                            };
                            resolved.insert(k.clone(), str_val);
                        }
                    }
                    expected_resolved.push(resolved);
                }

                let mut actual_sorted: Vec<String> = actual_inserts
                    .iter()
                    .map(|m| serde_json::to_string(m).unwrap())
                    .collect();
                actual_sorted.sort();
                let mut expected_sorted: Vec<String> = expected_resolved
                    .iter()
                    .map(|m| serde_json::to_string(m).unwrap())
                    .collect();
                expected_sorted.sort();

                assert_eq!(
                    actual_sorted.len(),
                    expected_sorted.len(),
                    "{dataset}: insert count mismatch.\n  actual: {actual_sorted:?}\n  expected: {expected_sorted:?}"
                );
                for (actual, expected) in actual_sorted.iter().zip(expected_sorted.iter()) {
                    assert_eq!(
                        actual, expected,
                        "{dataset}: insert mismatch.\n  actual:   {actual}\n  expected: {expected}"
                    );
                }
                eprintln!(
                    "{dataset}: {count} inserts match ✓",
                    count = actual_inserts.len()
                );
            }
        }
    }
}

/// List all examples that have test cases defined.
#[test]
fn list_testable_examples() {
    let examples = discover_test_examples();
    eprintln!("Examples with test cases ({}):", examples.len());
    for (name, _path) in &examples {
        eprintln!("  - {name}");
    }
    assert!(!examples.is_empty(), "No examples with tests found");
}

// ── Shared helpers ──────────────────────────────────────────────────────

/// Resolve a `_cluster_id` seed like `"crm:2"` to the actual
/// `_entity_id_resolved` from the identity view.
///
/// Parses the seed as `"{mapping}:{src_id}"` and queries
/// `_id_{target}` for the resolved entity ID.
async fn resolve_cluster_id(
    client: &tokio_postgres::Client,
    seed: &str,
    target_name: &str,
) -> String {
    let (mapping, src_id) = seed.split_once(':').unwrap_or_else(|| {
        panic!("_cluster_id seed must be 'mapping:src_id', got '{seed}'")
    });
    let id_view = format!("_id_{target_name}");
    let rows = client
        .query(
            &format!(
                "SELECT _entity_id_resolved FROM {id_view} \
                 WHERE _mapping = $1 AND _src_id = $2 LIMIT 1"
            ),
            &[&mapping, &src_id],
        )
        .await
        .unwrap_or_else(|e| {
            panic!("resolve _cluster_id for '{seed}' in {id_view}: {e}")
        });
    assert!(
        !rows.is_empty(),
        "_cluster_id seed '{seed}': no row found in {id_view} for _mapping='{mapping}' _src_id='{src_id}'"
    );
    rows[0].get::<_, String>("_entity_id_resolved")
}

/// Infer Postgres column types from JSON test data values.
///
/// Scans all rows for each column and picks the narrowest compatible type:
/// - All String (or null) → TEXT
/// - All Number (or null) → NUMERIC
/// - All Bool (or null) → BOOLEAN
/// - Array or Object → JSONB
/// - Mixed non-null types → TEXT (safe fallback)
/// - All null → TEXT
fn infer_column_types(
    columns: &[String],
    rows: &[serde_json::Value],
) -> std::collections::HashMap<String, &'static str> {
    let mut types: std::collections::HashMap<String, &'static str> = std::collections::HashMap::new();

    for col in columns {
        let mut seen: Option<&str> = None;
        let mut mixed = false;

        for row in rows {
            let val = row.as_object().and_then(|obj| obj.get(col.as_str()));
            let kind = match val {
                None | Some(serde_json::Value::Null) => continue,
                Some(serde_json::Value::String(_)) => "TEXT",
                Some(serde_json::Value::Number(_)) => "NUMERIC",
                Some(serde_json::Value::Bool(_)) => "BOOLEAN",
                Some(serde_json::Value::Array(_)) | Some(serde_json::Value::Object(_)) => "JSONB",
            };
            match seen {
                None => seen = Some(kind),
                Some(prev) if prev == kind => {}
                Some(_) => {
                    mixed = true;
                    break;
                }
            }
        }

        types.insert(col.clone(), if mixed { "TEXT" } else { seen.unwrap_or("TEXT") });
    }

    types
}

/// Format a JSON value as a SQL literal appropriate for its column type.
fn format_sql_literal(val: Option<&serde_json::Value>, pg_type: &str) -> String {
    match val {
        None | Some(serde_json::Value::Null) => "NULL".to_string(),
        Some(serde_json::Value::String(s)) => {
            format!("'{}'", s.replace('\'', "''"))
        }
        Some(serde_json::Value::Number(n)) => {
            if pg_type == "TEXT" {
                format!("'{n}'")
            } else {
                n.to_string()
            }
        }
        Some(serde_json::Value::Bool(b)) => {
            if pg_type == "TEXT" {
                format!("'{b}'")
            } else {
                b.to_string()
            }
        }
        Some(v @ serde_json::Value::Array(_)) | Some(v @ serde_json::Value::Object(_)) => {
            let json_str = serde_json::to_string(v).unwrap();
            format!("'{}'::jsonb", json_str.replace('\'', "''"))
        }
    }
}

async fn setup_pg() -> (
    tokio_postgres::Client,
    testcontainers::ContainerAsync<Postgres>,
) {
    let container = Postgres::default()
        .start()
        .await
        .expect("Failed to start Postgres container");

    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let conn_str = format!(
        "host=127.0.0.1 port={host_port} user=postgres password=postgres dbname=postgres"
    );

    let (client, connection) = tokio_postgres::connect(&conn_str, NoTls)
        .await
        .expect("Failed to connect to Postgres");

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("Connection error: {e}");
        }
    });

    (client, container)
}

async fn load_test_data(
    client: &tokio_postgres::Client,
    input: &indexmap::IndexMap<String, Vec<serde_json::Value>>,
) {
    for (dataset, rows) in input {
        let mut columns: Vec<String> = Vec::new();
        for row in rows {
            if let Some(obj) = row.as_object() {
                for key in obj.keys() {
                    if !columns.contains(key) {
                        columns.push(key.clone());
                    }
                }
            }
        }

        let col_types = infer_column_types(&columns, rows);

        let col_defs: Vec<String> = std::iter::once("_row_id SERIAL PRIMARY KEY".to_string())
            .chain(columns.iter().map(|c| {
                let pg_type = col_types.get(c).copied().unwrap_or("TEXT");
                format!("{c} {pg_type}")
            }))
            .collect();
        // DROP CASCADE to remove dependent views, then re-create.
        client
            .execute(&format!("DROP TABLE IF EXISTS {dataset} CASCADE"), &[])
            .await
            .unwrap_or_else(|e| panic!("DROP TABLE {dataset}: {e}"));
        let create_sql = format!(
            "CREATE TABLE {dataset} ({cols})",
            cols = col_defs.join(", ")
        );
        client
            .execute(&create_sql, &[])
            .await
            .unwrap_or_else(|e| panic!("CREATE TABLE {dataset}: {e}\nSQL: {create_sql}"));

        for row in rows {
            if let Some(obj) = row.as_object() {
                let vals: Vec<String> = columns
                    .iter()
                    .map(|c| {
                        let pg_type = col_types.get(c).copied().unwrap_or("TEXT");
                        format_sql_literal(obj.get(c), pg_type)
                    })
                    .collect();
                let insert_sql = format!(
                    "INSERT INTO {dataset} ({cols}) VALUES ({vals})",
                    cols = columns.join(", "),
                    vals = vals.join(", ")
                );
                client
                    .execute(&insert_sql, &[])
                    .await
                    .unwrap_or_else(|e| panic!("INSERT {dataset}: {e}\nSQL: {insert_sql}"));
            }
        }
    }
}

/// Ensure cluster_members tables exist for mappings that declare them.
/// If the test input already created the table (with data), skip it.
/// Otherwise create an empty table so the forward view's LEFT JOIN succeeds.
async fn ensure_cluster_members_tables(
    client: &tokio_postgres::Client,
    doc: &osi_engine::model::MappingDocument,
    input: &indexmap::IndexMap<String, Vec<serde_json::Value>>,
) {
    for mapping in &doc.mappings {
        if let Some(ref cm) = mapping.cluster_members {
            let table = cm.table_name(&mapping.name);
            if !input.contains_key(&table) {
                // Not in test input — drop any stale table and create empty
                client
                    .execute(&format!("DROP TABLE IF EXISTS {table} CASCADE"), &[])
                    .await
                    .unwrap();
                client
                    .execute(
                        &format!(
                            "CREATE TABLE {table} ({} TEXT, {} TEXT)",
                            cm.cluster_id, cm.source_key,
                        ),
                        &[],
                    )
                    .await
                    .unwrap();
            }
        }
    }
}

async fn execute_views(client: &tokio_postgres::Client, sql: &str) {
    for stmt in sql.split(';') {
        let stmt: String = stmt
            .lines()
            .filter(|line| !line.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let stmt = stmt.trim();
        if stmt.is_empty() || stmt == "BEGIN" || stmt == "COMMIT" {
            continue;
        }
        client.execute(stmt, &[]).await.unwrap_or_else(|e| {
            panic!("SQL error:\n{stmt}\n\nError: {e}");
        });
    }
}

async fn dump_view(client: &tokio_postgres::Client, view: &str) {
    let rows = client
        .query(&format!("SELECT * FROM {view}"), &[])
        .await
        .unwrap_or_else(|e| panic!("query {view}: {e}"));

    if rows.is_empty() {
        eprintln!("  (empty)");
        return;
    }

    let cols: Vec<String> = rows[0]
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    // header
    eprintln!("  {}", cols.join(" | "));
    eprintln!("  {}", cols.iter().map(|c| "-".repeat(c.len().max(8))).collect::<Vec<_>>().join("-+-"));
    // rows
    for row in &rows {
        let vals: Vec<String> = cols
            .iter()
            .enumerate()
            .map(|(i, _)| {
                // Try text first, fall back to i32/i64 for _row_id/_entity_id
                if let Ok(Some(s)) = row.try_get::<_, Option<String>>(i) {
                    s
                } else if let Ok(v) = row.try_get::<_, i64>(i) {
                    v.to_string()
                } else if let Ok(v) = row.try_get::<_, i32>(i) {
                    v.to_string()
                } else {
                    "NULL".to_string()
                }
            })
            .collect();
        eprintln!("  {}", vals.join(" | "));
    }
}

// ── Intermediate view dump tests ────────────────────────────────────────

/// Dump all intermediate views for hello-world so we can inspect the pipeline.
#[tokio::test]
async fn dump_hello_world_intermediates() {
    let (client, _container) = setup_pg().await;

    let mapping_path = examples_dir().join("hello-world/mapping.yaml");
    let doc = osi_engine::parser::parse_file(&mapping_path).expect("parse");
    let dag = osi_engine::dag::build_dag(&doc);
    let sql = osi_engine::render::render_sql(&doc, &dag).expect("render");

    load_test_data(&client, &doc.tests[0].input).await;
    ensure_cluster_members_tables(&client, &doc, &doc.tests[0].input).await;
    execute_views(&client, &sql).await;

    let views = [
        "_fwd_crm", "_fwd_erp",
        "_id_contact",
        "_resolved_contact",
        "_rev_crm", "_rev_erp",
        "_delta_crm", "_delta_erp",
    ];

    for view in &views {
        eprintln!("\n=== {view} ===");
        dump_view(&client, view).await;
    }
}

/// Dump all intermediate views for inserts-and-deletes.
#[tokio::test]
async fn dump_inserts_and_deletes_intermediates() {
    let (client, _container) = setup_pg().await;

    let mapping_path = examples_dir().join("inserts-and-deletes/mapping.yaml");
    let doc = osi_engine::parser::parse_file(&mapping_path).expect("parse");
    let dag = osi_engine::dag::build_dag(&doc);
    let sql = osi_engine::render::render_sql(&doc, &dag).expect("render");

    eprintln!("\n=== Generated SQL ===\n{sql}");

    load_test_data(&client, &doc.tests[0].input).await;
    execute_views(&client, &sql).await;

    let views = [
        "_fwd_crm_a", "_fwd_crm_b",
        "_id_person",
        "_resolved_person",
        "_rev_crm_a", "_rev_crm_b",
        "_delta_crm_a", "_delta_crm_b",
    ];

    for view in &views {
        eprintln!("\n=== {view} ===");
        dump_view(&client, view).await;
    }
}
