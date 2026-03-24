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

/// Split SQL into statements, respecting dollar-quoted blocks (`$$...$$`).
fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut stmts = Vec::new();
    let mut current = String::new();
    let mut in_dollar = false;
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'$') {
            current.push(ch);
            current.push(chars.next().unwrap());
            in_dollar = !in_dollar;
        } else if ch == ';' && !in_dollar {
            stmts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    if !current.trim().is_empty() {
        stmts.push(current);
    }
    stmts
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
                match osi_engine::render::render_sql(&doc, &dag, false, false, false) {
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
    let (client, _container) = setup_pg().await;
    execute_example(&client, "hello-world").await;
}

/// End-to-end test for cross-entity reference resolution.
#[tokio::test]
async fn execute_references() {
    let (client, _container) = setup_pg().await;
    execute_example(&client, "references").await;
}

#[tokio::test]
async fn execute_route() {
    let (client, _container) = setup_pg().await;
    execute_example(&client, "route").await;
}

/// Run all testable examples and report pass/fail summary.
/// Failures are collected (not panic) so every example gets a chance.
///
/// Filter with env var: `OSI_EXAMPLES=route,hello-world cargo test execute_all_examples`
#[tokio::test]
async fn execute_all_examples() {
    let (client, _container) = setup_pg().await;
    let all_examples = discover_test_examples();

    // Optional filter via OSI_EXAMPLES env var (comma-separated).
    let filter: Option<Vec<String>> = std::env::var("OSI_EXAMPLES").ok().map(|v| {
        v.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
    let examples: Vec<_> = all_examples
        .iter()
        .filter(|(name, _)| {
            filter
                .as_ref()
                .is_none_or(|f| f.iter().any(|p| name.contains(p.as_str())))
        })
        .collect();

    if let Some(ref f) = filter {
        eprintln!(
            "Filtering examples: {:?} → {}/{} matched",
            f,
            examples.len(),
            all_examples.len()
        );
    }

    let mut passed = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut skipped = Vec::new();

    for (name, _path) in &examples {
        eprintln!("\n{}", "=".repeat(60));
        eprintln!("  Example: {name}");
        eprintln!("{}", "=".repeat(60));

        // Parse + render (non-async, can catch)
        let mapping_path = examples_dir().join(format!("{name}/mapping.yaml"));
        let doc = match osi_engine::parser::parse_file(&mapping_path) {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("parse: {e}");
                eprintln!("  ✗ {name} FAILED: {msg}");
                failed.push((name.clone(), msg));
                continue;
            }
        };
        let dag = osi_engine::dag::build_dag(&doc);
        let sql = match osi_engine::render::render_sql(&doc, &dag, false, false, false) {
            Ok(s) => s,
            Err(e) => {
                let msg = format!("render: {e}");
                eprintln!("  ✗ {name} FAILED: {msg}");
                failed.push((name.clone(), msg));
                continue;
            }
        };

        // Check if the mapping has any sync views (needs delta)
        if !doc.mappings.iter().any(|m| m.needs_sync()) {
            eprintln!("  ⊘ {name} SKIPPED (no sync views)");
            skipped.push(name.clone());
            continue;
        }

        // Run each test case
        let mut example_ok = true;
        for (test_idx, test) in doc.tests.iter().enumerate() {
            let desc = test.description.as_deref().unwrap_or("(unnamed)");
            eprintln!("  --- Test {}: {desc} ---", test_idx + 1);

            load_test_data(&client, &test.input).await;
            ensure_cluster_members_tables(&client, &doc, &test.input).await;
            ensure_written_state_tables(&client, &doc, &test.input).await;
            ensure_source_columns(&client, &doc, &test.input).await;

            // Drop stale views in reverse order to avoid dependency errors.
            for node in dag.order.iter().rev() {
                let vn = osi_engine::qi(&node.view_name());
                if !matches!(node, osi_engine::dag::ViewNode::Source(_)) {
                    let _ = client
                        .execute(&format!("DROP VIEW IF EXISTS {vn} CASCADE"), &[])
                        .await;
                }
            }

            // Execute views — may fail for unsupported features
            let mut exec_err = None;
            for stmt in split_sql_statements(&sql) {
                let stmt: String = stmt
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("--"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let stmt = stmt.trim();
                if stmt.is_empty() || stmt == "BEGIN" || stmt == "COMMIT" {
                    continue;
                }
                if let Err(e) = client.execute(stmt, &[]).await {
                    exec_err = Some(format!(
                        "SQL exec: {e:?}\n  SQL: {}",
                        stmt.lines().next().unwrap_or("")
                    ));
                    break;
                }
            }
            if let Some(err) = exec_err {
                eprintln!("  ✗ {name} test {}: {err}", test_idx + 1);
                failed.push((name.clone(), err));
                example_ok = false;
                break;
            }

            // Populate written-state tables now that identity views exist.
            populate_written_state_tables(&client, &doc, &test.input).await;

            // Compare expected with actual — skip if expected is empty
            if test.expected.is_empty() {
                eprintln!("  ✓ (empty expected)");
                continue;
            }

            // Run comparison via execute_example's inner logic
            // For simplicity, delegate to the full verifier
            match verify_test_expected(&client, &doc, test).await {
                Ok(()) => {
                    eprintln!("  ✓ test {}", test_idx + 1);
                }
                Err(e) => {
                    eprintln!("  ✗ {name} test {}: {e}", test_idx + 1);
                    failed.push((name.clone(), e));
                    example_ok = false;
                    break;
                }
            }
        }

        if example_ok {
            eprintln!("  ✓ {name} PASSED");
            passed.push(name.clone());
        }
    }

    eprintln!("\n\n===== SUMMARY =====");
    eprintln!("Passed:  {}/{}", passed.len(), examples.len());
    for name in &passed {
        eprintln!("  ✓ {name}");
    }
    if !skipped.is_empty() {
        eprintln!("Skipped: {}", skipped.len());
        for name in &skipped {
            eprintln!("  ⊘ {name}");
        }
    }
    if !failed.is_empty() {
        eprintln!("Failed:  {}", failed.len());
        for (name, err) in &failed {
            eprintln!("  ✗ {name}: {err}");
        }
    }
    assert!(failed.is_empty(), "{} example(s) failed", failed.len());
}

async fn execute_example(client: &tokio_postgres::Client, example_name: &str) {
    // Parse and render example
    let mapping_path = examples_dir().join(format!("{example_name}/mapping.yaml"));
    let doc = osi_engine::parser::parse_file(&mapping_path)
        .unwrap_or_else(|e| panic!("parse {example_name}: {e}"));

    // Validate expected test values: non-string scalars in nested JSONB objects must
    // have a matching `type:` on the target field, otherwise the engine returns text
    // and the comparison silently requires stringified values.
    validate_expected_types(&doc, example_name);

    let dag = osi_engine::dag::build_dag(&doc);
    let sql = osi_engine::render::render_sql(&doc, &dag, false, false, false)
        .unwrap_or_else(|e| panic!("render {example_name}: {e}"));

    for (test_idx, test) in doc.tests.iter().enumerate() {
        let desc = test.description.as_deref().unwrap_or("(unnamed)");
        eprintln!("\n--- Test {}: {desc} ---", test_idx + 1);

        // Create source tables from test input
        load_test_data(client, &test.input).await;

        // Ensure cluster_members tables exist (may not be in test input)
        ensure_cluster_members_tables(client, &doc, &test.input).await;

        // Ensure written_state tables exist with proper JSONB types
        ensure_written_state_tables(client, &doc, &test.input).await;

        // Ensure source tables have all required columns (even if source data was empty)
        ensure_source_columns(client, &doc, &test.input).await;

        // Execute the rendered SQL views
        for stmt in split_sql_statements(&sql) {
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

        // Populate written-state tables now that identity views exist.
        populate_written_state_tables(client, &doc, &test.input).await;

        // Compare reverse views with expected output
        for (expected_key, expected) in &test.expected {
            // expected_key is the source dataset name.
            // Find all mappings for this source.
            let source_mappings: Vec<&_> = doc
                .mappings
                .iter()
                .filter(|m| m.source.dataset == *expected_key || m.name == *expected_key)
                .collect();
            assert!(
                !source_mappings.is_empty(),
                "No mapping for key {expected_key}"
            );
            let dataset = &source_mappings[0].source.dataset;

            // Build reverse field mapping (needed for noop verification)
            let mut reverse_fields: Vec<(String, String)> = Vec::new();
            // Fields whose noop delta value may differ from source (normalize / written_noop).
            let mut noop_exempt_fields: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for mapping in &source_mappings {
                if mapping.source.path.is_some() {
                    continue;
                }
                let all_exempt = mapping.derive_noop;
                for fm in &mapping.fields {
                    if fm.is_reverse() && fm.source.is_some() {
                        let pair = (
                            fm.source.clone().unwrap(),
                            fm.target.clone().unwrap_or_default(),
                        );
                        if !reverse_fields.iter().any(|(s, _)| s == &pair.0) {
                            reverse_fields.push(pair);
                        }
                        if all_exempt || fm.normalize.is_some() {
                            noop_exempt_fields.insert(fm.source.clone().unwrap());
                        }
                    }
                }
            }

            // Query delta view for update rows
            let delta_view = osi_engine::qi(&format!("_delta_{dataset}"));
            let rev_rows = client
                .query(&format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'update'"), &[])
                .await
                .unwrap_or_else(|e| panic!("Failed to query {delta_view}: {e:?}"));

            // Build actual output directly from delta view columns
            let expects_base = expected
                .updates
                .iter()
                .any(|v| v.as_object().is_some_and(|obj| obj.contains_key("_base")));
            let actual_updates: Vec<serde_json::Map<String, serde_json::Value>> = rev_rows
                .iter()
                .map(|row| delta_row_to_map(row, expects_base, false))
                .collect();

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
                    // Normalize: recursively convert all values to strings
                    let normalized: serde_json::Map<String, serde_json::Value> = m
                        .iter()
                        .map(|(k, v)| (k.clone(), normalize_json_to_text(v)))
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
            eprintln!(
                "{dataset}: {count} updates match ✓",
                count = actual_updates.len()
            );

            // ── Insert verification ────────────────────────────────
            let expected_inserts: Vec<serde_json::Map<String, serde_json::Value>> = expected
                .inserts
                .iter()
                .filter_map(|v| v.as_object().cloned())
                .collect();

            if !expected_inserts.is_empty() {
                let insert_rows = client
                    .query(
                        &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'insert'"),
                        &[],
                    )
                    .await
                    .unwrap_or_else(|e| panic!("query {delta_view} inserts: {e}"));

                // Build actual insert maps directly from delta view columns.
                let expects_base = expected_inserts.iter().any(|obj| obj.contains_key("_base"));
                // Require _cluster_id on every expected insert.
                for exp in &expected_inserts {
                    assert!(
                        exp.contains_key("_cluster_id"),
                        "{dataset}: every expected insert must include _cluster_id.\n  missing in: {}",
                        serde_json::to_string(exp).unwrap(),
                    );
                }
                let actual_inserts: Vec<serde_json::Map<String, serde_json::Value>> = insert_rows
                    .iter()
                    .map(|row| delta_row_to_map(row, expects_base, true))
                    .collect();

                // Resolve expected _cluster_id seeds: "mapping:src_id" → look up
                // _entity_id_resolved from the identity view.
                let mut expected_resolved: Vec<serde_json::Map<String, serde_json::Value>> =
                    Vec::new();
                for exp in &expected_inserts {
                    let mut resolved = serde_json::Map::new();
                    for (k, v) in exp {
                        if k == "_cluster_id" {
                            if let Some(seed) = v.as_str() {
                                // Extract mapping name from seed "mapping:src_id" to find target
                                let seed_mapping =
                                    seed.split_once(':').map(|(m, _)| m).unwrap_or("");
                                let target_name = doc
                                    .mappings
                                    .iter()
                                    .find(|m| m.name == seed_mapping)
                                    .map(|m| m.target.name())
                                    .unwrap_or_else(|| source_mappings[0].target.name());
                                let cluster_id =
                                    resolve_cluster_id(client, seed, target_name).await;
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
            } else {
                let insert_rows = client
                    .query(
                        &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'insert'"),
                        &[],
                    )
                    .await
                    .unwrap_or_else(|e| panic!("query {delta_view} inserts: {e}"));
                assert_eq!(
                    insert_rows.len(),
                    0,
                    "{dataset}: expected 0 inserts but got {}",
                    insert_rows.len()
                );
            }

            // ── Delete verification ────────────────────────────────
            let expected_deletes: Vec<serde_json::Map<String, serde_json::Value>> = expected
                .deletes
                .iter()
                .filter_map(|v| v.as_object().cloned())
                .collect();

            if !expected_deletes.is_empty() {
                let delete_rows = client
                    .query(
                        &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'delete'"),
                        &[],
                    )
                    .await
                    .unwrap_or_else(|e| panic!("query {delta_view} deletes: {e}"));

                let expects_base = expected_deletes.iter().any(|obj| obj.contains_key("_base"));
                let actual_deletes: Vec<serde_json::Map<String, serde_json::Value>> = delete_rows
                    .iter()
                    .map(|row| delta_row_to_map(row, expects_base, false))
                    .collect();

                let mut actual_sorted: Vec<String> = actual_deletes
                    .iter()
                    .map(|m| serde_json::to_string(m).unwrap())
                    .collect();
                actual_sorted.sort();

                let mut expected_sorted: Vec<String> = expected_deletes
                    .iter()
                    .map(|m| {
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
                    "{dataset}: delete count mismatch.\n  actual: {actual_sorted:?}\n  expected: {expected_sorted:?}"
                );
                for (actual, expected) in actual_sorted.iter().zip(expected_sorted.iter()) {
                    assert_eq!(
                        actual, expected,
                        "{dataset}: delete mismatch.\n  actual:   {actual}\n  expected: {expected}"
                    );
                }
                eprintln!(
                    "{dataset}: {count} deletes match ✓",
                    count = actual_deletes.len()
                );
            } else {
                let delete_rows = client
                    .query(
                        &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'delete'"),
                        &[],
                    )
                    .await
                    .unwrap_or_else(|e| panic!("query {delta_view} deletes: {e}"));
                assert_eq!(
                    delete_rows.len(),
                    0,
                    "{dataset}: expected 0 deletes but got {}",
                    delete_rows.len()
                );
            }

            // ── Implicit noop verification ─────────────────────────
            // Verify noop content: reverse-mapped values must match source row.
            let noop_rows = client
                .query(
                    &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'noop'"),
                    &[],
                )
                .await
                .unwrap_or_else(|e| panic!("query {delta_view} noops: {e}"));

            let noop_count = noop_rows.len() as i64;

            for noop_row in &noop_rows {
                let noop_json: serde_json::Value = noop_row.get("_json");
                let noop_obj = noop_json
                    .as_object()
                    .expect("row_to_json should produce object");

                let (pk_where, pk_params): (String, Vec<String>) =
                    if let Some(src_meta) = doc.sources.get(dataset.as_str()) {
                        let cols = src_meta.primary_key.columns();
                        let clauses: Vec<String> = cols
                            .iter()
                            .enumerate()
                            .map(|(i, c)| format!("{}::text = ${}", osi_engine::qi(c), i + 1))
                            .collect();
                        let vals: Vec<String> = cols
                            .iter()
                            .map(|c| {
                                noop_obj
                                    .get(*c)
                                    .and_then(|v| match v {
                                        serde_json::Value::String(s) => Some(s.clone()),
                                        serde_json::Value::Number(n) => Some(n.to_string()),
                                        _ => None,
                                    })
                                    .unwrap_or_default()
                            })
                            .collect();
                        (clauses.join(" AND "), vals)
                    } else {
                        let val = noop_obj
                            .get("_row_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        ("_row_id::text = $1".to_string(), vec![val])
                    };

                let query = format!("SELECT * FROM {} WHERE {pk_where}", osi_engine::qi(dataset));
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = pk_params
                    .iter()
                    .map(|s| s as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let source_rows = client.query(&query, &params).await.unwrap();
                assert!(
                    !source_rows.is_empty(),
                    "{dataset}: noop row has no matching source (pk_where: {pk_where}, params: {pk_params:?})"
                );
                let source_row = &source_rows[0];

                for (src_field, _) in &reverse_fields {
                    // Skip fields that may legitimately differ (normalize / written_noop).
                    if noop_exempt_fields.contains(src_field.as_str()) {
                        continue;
                    }
                    let noop_val: Option<String> =
                        noop_obj.get(src_field.as_str()).and_then(|v| match v {
                            serde_json::Value::String(s) => Some(s.clone()),
                            serde_json::Value::Number(n) => Some(n.to_string()),
                            serde_json::Value::Bool(b) => Some(b.to_string()),
                            serde_json::Value::Null => None,
                            other => Some(other.to_string()),
                        });
                    let source_val: Option<String> = get_text(source_row, src_field.as_str());
                    // Skip comparison if delta field is NULL — this mapping doesn't project it
                    // (happens with embedded mappings where UNION ALL produces multiple partial rows)
                    if noop_val.is_none() && source_val.is_some() {
                        continue;
                    }
                    assert_eq!(
                        noop_val, source_val,
                        "{dataset}: noop row field '{src_field}' mismatch.\n  \
                         delta={noop_val:?} source={source_val:?}"
                    );
                }
            }

            eprintln!("{dataset}: {noop_count} implicit noops verified ✓");
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

// ── Verification helper for generic runner ──────────────────────────────

/// Result-based verification of test expected data against delta views.
/// Returns Ok(()) if all comparisons pass, Err(message) on first mismatch.
async fn verify_test_expected(
    client: &tokio_postgres::Client,
    doc: &osi_engine::model::MappingDocument,
    test: &osi_engine::model::TestCase,
) -> Result<(), String> {
    for (expected_key, expected) in &test.expected {
        // expected_key is the source dataset name.
        let source_mappings: Vec<&_> = doc
            .mappings
            .iter()
            .filter(|m| m.source.dataset == *expected_key || m.name == *expected_key)
            .collect();
        if source_mappings.is_empty() {
            return Err(format!("No mapping for key {expected_key}"));
        }
        let dataset = &source_mappings[0].source.dataset;

        let delta_view = osi_engine::qi(&format!("_delta_{dataset}"));

        // Build reverse field mapping (needed for noop verification)
        let mut reverse_fields: Vec<(String, String)> = Vec::new();
        let mut noop_exempt_fields: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for mapping in &source_mappings {
            if mapping.source.path.is_some() {
                continue;
            }
            let all_exempt = mapping.derive_noop;
            for fm in &mapping.fields {
                if fm.is_reverse() && fm.source.is_some() {
                    let pair = (
                        fm.source.clone().unwrap(),
                        fm.target.clone().unwrap_or_default(),
                    );
                    if !reverse_fields.iter().any(|(s, _)| s == &pair.0) {
                        reverse_fields.push(pair);
                    }
                    if all_exempt || fm.normalize.is_some() {
                        noop_exempt_fields.insert(fm.source.clone().unwrap());
                    }
                }
            }
        }

        // ── Verify updates ─────────────────────────
        let expected_updates: Vec<serde_json::Map<String, serde_json::Value>> = expected
            .updates
            .iter()
            .filter_map(|v| v.as_object().cloned())
            .collect();

        let update_rows = client
            .query(
                &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'update'"),
                &[],
            )
            .await
            .map_err(|e| format!("{dataset} query updates: {e}"))?;

        if update_rows.len() != expected_updates.len() {
            return Err(format!(
                "{dataset}: update count mismatch: got {} expected {}",
                update_rows.len(),
                expected_updates.len()
            ));
        }

        let expects_base = expected_updates.iter().any(|obj| obj.contains_key("_base"));
        let actual_updates: Vec<serde_json::Map<String, serde_json::Value>> = update_rows
            .iter()
            .map(|row| delta_row_to_map(row, expects_base, false))
            .collect();

        // Compare actual vs expected updates (full row comparison)
        let mut actual_sorted: Vec<String> = actual_updates
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect();
        actual_sorted.sort();

        let mut expected_sorted: Vec<String> = expected_updates
            .iter()
            .map(|m| {
                let normalized: serde_json::Map<String, serde_json::Value> = m
                    .iter()
                    .map(|(k, v)| (k.clone(), normalize_json_to_text(v)))
                    .collect();
                serde_json::to_string(&normalized).unwrap()
            })
            .collect();
        expected_sorted.sort();

        for (actual, exp) in actual_sorted.iter().zip(expected_sorted.iter()) {
            if actual != exp {
                return Err(format!(
                    "{dataset}: update row mismatch.\n  actual:   {actual}\n  expected: {exp}"
                ));
            }
        }

        // ── Verify inserts ─────────────────────────
        let expected_inserts: Vec<serde_json::Map<String, serde_json::Value>> = expected
            .inserts
            .iter()
            .filter_map(|v| v.as_object().cloned())
            .collect();

        if !expected_inserts.is_empty() {
            let insert_rows = client
                .query(
                    &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'insert'"),
                    &[],
                )
                .await
                .map_err(|e| format!("{dataset} query inserts: {e}"))?;

            if insert_rows.len() != expected_inserts.len() {
                return Err(format!(
                    "{dataset}: insert count mismatch: got {} expected {}",
                    insert_rows.len(),
                    expected_inserts.len()
                ));
            }

            let expects_base = expected_inserts.iter().any(|obj| obj.contains_key("_base"));
            // Require _cluster_id on every expected insert.
            for exp in &expected_inserts {
                if !exp.contains_key("_cluster_id") {
                    return Err(format!(
                        "{dataset}: every expected insert must include _cluster_id.\n  missing in: {}",
                        serde_json::to_string(exp).unwrap(),
                    ));
                }
            }
            let actual_inserts: Vec<serde_json::Map<String, serde_json::Value>> = insert_rows
                .iter()
                .map(|row| delta_row_to_map(row, expects_base, true))
                .collect();

            // Resolve expected _cluster_id seeds and normalize
            let mut expected_resolved: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();
            for exp in &expected_inserts {
                let mut resolved = serde_json::Map::new();
                for (k, v) in exp {
                    if k == "_cluster_id" {
                        if let Some(seed) = v.as_str() {
                            let seed_mapping = seed.split_once(':').map(|(m, _)| m).unwrap_or("");
                            let target_name = doc
                                .mappings
                                .iter()
                                .find(|m| m.name == seed_mapping)
                                .map(|m| m.target.name())
                                .unwrap_or_else(|| source_mappings[0].target.name());
                            let cluster_id = resolve_cluster_id(client, seed, target_name).await;
                            resolved.insert(k.clone(), serde_json::Value::String(cluster_id));
                        } else {
                            resolved.insert(k.clone(), v.clone());
                        }
                    } else {
                        resolved.insert(k.clone(), normalize_json_to_text(v));
                    }
                }
                expected_resolved.push(resolved);
            }

            // Full-column matching: expected inserts must specify every column
            // the delta produces (except _action, _base unless opted-in, and
            // source PK columns which are always null).  This catches missing
            // nested arrays and unexpected NULL fields.
            //
            // Normalize actual values to text to match expected normalization,
            // then sort and compare row-by-row.
            let actual_normalized: Vec<serde_json::Map<String, serde_json::Value>> = actual_inserts
                .iter()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), normalize_json_to_text(v)))
                        .collect()
                })
                .collect();
            let mut actual_sorted: Vec<String> = actual_normalized
                .iter()
                .map(|m| serde_json::to_string(m).unwrap())
                .collect();
            actual_sorted.sort();
            let mut expected_sorted: Vec<String> = expected_resolved
                .iter()
                .map(|m| serde_json::to_string(m).unwrap())
                .collect();
            expected_sorted.sort();

            if actual_sorted.len() != expected_sorted.len() {
                return Err(format!(
                    "{dataset}: insert count mismatch: got {} expected {}\n  actual: {actual_sorted:?}\n  expected: {expected_sorted:?}",
                    actual_sorted.len(),
                    expected_sorted.len(),
                ));
            }
            for (actual, exp) in actual_sorted.iter().zip(expected_sorted.iter()) {
                if actual != exp {
                    return Err(format!(
                        "{dataset}: insert row mismatch.\n  actual:   {actual}\n  expected: {exp}"
                    ));
                }
            }
        } else {
            let insert_rows = client
                .query(
                    &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'insert'"),
                    &[],
                )
                .await
                .map_err(|e| format!("{dataset} query inserts: {e}"))?;
            if !insert_rows.is_empty() {
                return Err(format!(
                    "{dataset}: expected 0 inserts but got {}",
                    insert_rows.len()
                ));
            }
        }

        // ── Verify deletes ─────────────────────────
        let expected_deletes: Vec<serde_json::Map<String, serde_json::Value>> = expected
            .deletes
            .iter()
            .filter_map(|v| v.as_object().cloned())
            .collect();

        if !expected_deletes.is_empty() {
            let delete_rows = client
                .query(
                    &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'delete'"),
                    &[],
                )
                .await
                .map_err(|e| format!("{dataset} query deletes: {e}"))?;

            if delete_rows.len() != expected_deletes.len() {
                return Err(format!(
                    "{dataset}: delete count mismatch: got {} expected {}",
                    delete_rows.len(),
                    expected_deletes.len()
                ));
            }
        } else {
            let delete_rows = client
                .query(
                    &format!("SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'delete'"),
                    &[],
                )
                .await
                .map_err(|e| format!("{dataset} query deletes: {e}"))?;
            if !delete_rows.is_empty() {
                return Err(format!(
                    "{dataset}: expected 0 deletes but got {}",
                    delete_rows.len()
                ));
            }
        }

        // ── Verify implicit noops ──────────────────
        let noop_rows = client
            .query(
                &format!(
                    "SELECT row_to_json(d.*) AS _json FROM {delta_view} d WHERE d._action = 'noop'"
                ),
                &[],
            )
            .await
            .map_err(|e| format!("{dataset} query noops: {e}"))?;

        // Verify noop content: reverse-mapped values must match source row.
        for noop_row in &noop_rows {
            let noop_json: serde_json::Value = noop_row.get("_json");
            let noop_obj = noop_json
                .as_object()
                .expect("row_to_json should produce object");

            let json_to_string = |v: &serde_json::Value| -> Option<String> {
                match v {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    serde_json::Value::Bool(b) => Some(b.to_string()),
                    serde_json::Value::Null => None,
                    other => Some(other.to_string()),
                }
            };

            let (pk_vals, pk_where_str): (Vec<String>, String) = if let Some(src_meta) =
                doc.sources.get(dataset.as_str())
            {
                let cols = src_meta.primary_key.columns();
                if cols.len() == 1 {
                    let val = noop_obj
                        .get(cols[0])
                        .and_then(json_to_string)
                        .ok_or_else(|| {
                            format!("{dataset}: noop row missing PK column {}", cols[0])
                        })?;
                    (vec![val], format!("{}::text = $1", cols[0]))
                } else {
                    let mut vals = Vec::new();
                    let mut clauses = Vec::new();
                    for (i, col) in cols.iter().enumerate() {
                        let val = noop_obj.get(*col).and_then(json_to_string).ok_or_else(|| {
                            format!("{dataset}: noop row missing PK column {col}")
                        })?;
                        vals.push(val);
                        clauses.push(format!("{col}::text = ${}", i + 1));
                    }
                    (vals, clauses.join(" AND "))
                }
            } else {
                let val = noop_obj
                    .get("_row_id")
                    .and_then(json_to_string)
                    .ok_or_else(|| format!("{dataset}: noop row missing _row_id"))?;
                (vec![val], "_row_id::text = $1".to_string())
            };

            let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = pk_vals
                .iter()
                .map(|s| s as &(dyn tokio_postgres::types::ToSql + Sync))
                .collect();
            let source_rows = client
                .query(
                    &format!(
                        "SELECT * FROM {} WHERE {pk_where_str}",
                        osi_engine::qi(dataset)
                    ),
                    &params,
                )
                .await
                .map_err(|e| format!("{dataset} noop source lookup: {e}"))?;
            if source_rows.is_empty() {
                return Err(format!(
                    "{dataset}: noop row has no matching source (pk={pk_vals:?})"
                ));
            }
            let source_row = &source_rows[0];

            for (src_field, _) in &reverse_fields {
                if noop_exempt_fields.contains(src_field.as_str()) {
                    continue;
                }
                let noop_val: Option<String> =
                    noop_obj.get(src_field.as_str()).and_then(json_to_string);
                let source_val: Option<String> = get_text(source_row, src_field.as_str());
                if noop_val.is_none() && source_val.is_some() {
                    continue;
                }
                if noop_val != source_val {
                    return Err(format!(
                        "{dataset}: noop row field '{src_field}' mismatch: delta={noop_val:?} source={source_val:?}"
                    ));
                }
            }
        }
    }

    // ── Verify unlisted sources are empty ──────────
    // Any source with a delta view that wasn't listed in `test.expected`
    // must produce only noop / NULL rows (no inserts, updates, or deletes).
    let listed: std::collections::HashSet<&str> =
        test.expected.keys().map(|k| k.as_str()).collect();
    let mut checked_datasets: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut unlisted_errors: Vec<String> = Vec::new();
    for m in &doc.mappings {
        if !m.needs_sync() || m.source.path.is_some() {
            continue;
        }
        let ds = &m.source.dataset;
        if listed.contains(ds.as_str()) || !checked_datasets.insert(ds.clone()) {
            continue;
        }
        let delta_view = osi_engine::qi(&format!("_delta_{ds}"));
        let action_rows = client
            .query(
                &format!(
                    "SELECT d._action::text, count(*) AS cnt \
                     FROM {delta_view} d \
                     WHERE d._action IS NOT NULL AND d._action <> 'noop' \
                     GROUP BY d._action"
                ),
                &[],
            )
            .await
            .map_err(|e| format!("{ds} (unlisted) query actions: {e}"))?;
        if !action_rows.is_empty() {
            let summary: Vec<String> = action_rows
                .iter()
                .map(|r| {
                    let action: String = r.get(0);
                    let cnt: i64 = r.get(1);
                    format!("{cnt} {action}(s)")
                })
                .collect();
            unlisted_errors.push(format!(
                "{ds}: not in expected but has non-noop rows: {}",
                summary.join(", ")
            ));
        }
    }

    if !unlisted_errors.is_empty() {
        return Err(unlisted_errors.join("\n"));
    }

    Ok(())
}

// ── Shared helpers ──────────────────────────────────────────────────────

/// Resolve a `_cluster_id` seed like `"crm:2"` to the actual
/// `_entity_id_resolved` from the identity view.
///
/// Parses the seed as `"{mapping}:{src_id}"` and queries
/// `_id_{target}` for the resolved entity ID.
///
/// For nested-array elements that share `_src_id`, append query-param
/// style filters to disambiguate: `"shop_lines:ORD-001?line_number=1"`.
async fn resolve_cluster_id(
    client: &tokio_postgres::Client,
    seed: &str,
    target_name: &str,
) -> String {
    // Split off optional ?field=value&field2=value2 filters.
    let (base, query) = seed.split_once('?').unwrap_or((seed, ""));
    let (mapping, src_id) = base
        .split_once(':')
        .unwrap_or_else(|| panic!("_cluster_id seed must be 'mapping:src_id', got '{seed}'"));
    let id_view = osi_engine::qi(&format!("_id_{target_name}"));

    let mut sql = format!(
        "SELECT _entity_id_resolved FROM {id_view} \
         WHERE _mapping = $1 AND _src_id = $2"
    );
    let mut params: Vec<String> = vec![mapping.to_string(), src_id.to_string()];

    if !query.is_empty() {
        for pair in query.split('&') {
            let (field, value) = pair.split_once('=').unwrap_or_else(|| {
                panic!("_cluster_id seed filter must be 'field=value', got '{pair}' in '{seed}'")
            });
            params.push(value.to_string());
            sql.push_str(&format!(
                " AND {}::text = ${}",
                osi_engine::qi(field),
                params.len()
            ));
        }
    }
    sql.push_str(" LIMIT 1");

    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        params.iter().map(|s| s as _).collect();
    let rows = client
        .query(&sql, &param_refs)
        .await
        .unwrap_or_else(|e| panic!("resolve _cluster_id for '{seed}' in {id_view}: {e}"));
    assert!(
        !rows.is_empty(),
        "_cluster_id seed '{seed}': no row found in {id_view} for _mapping='{mapping}' _src_id='{src_id}'"
    );
    rows[0].get::<_, String>("_entity_id_resolved")
}

/// Recursively normalize JSON values: convert Number/Bool to String at all depths.
/// Matches the engine pipeline which normalizes all values to text.
fn normalize_json_to_text(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(s.clone()),
        serde_json::Value::Number(n) => serde_json::Value::Number(n.clone()),
        serde_json::Value::Bool(b) => serde_json::Value::Bool(*b),
        serde_json::Value::Null => serde_json::Value::Null,
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(normalize_json_to_text).collect())
        }
        serde_json::Value::Object(obj) => serde_json::Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), normalize_json_to_text(v)))
                .collect(),
        ),
    }
}

/// Validate that expected test values don't contain non-string scalars (Number, Bool)
/// for fields that lack a `type:` declaration on the target. Without `type:`, the
/// engine pipeline returns text, so writing `budget: 50000` in expected would silently
/// fail (actual is `"50000"`). Either add `type: numeric` to the target field or use
/// a string value in expected.
fn validate_expected_types(doc: &osi_engine::model::MappingDocument, example_name: &str) {
    // Build a set of target field names that have an explicit type declaration.
    // Key: field name (used in reverse views / nested JSONB objects).
    let mut typed_fields: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (_target_name, target) in &doc.targets {
        for (field_name, field_def) in &target.fields {
            if field_def.field_type().is_some() {
                typed_fields.insert(field_name.clone());
            }
        }
    }

    // Build a map from reverse-view source column names to target field names.
    // In nested JSONB, the key is the source column name, not the target field name.
    let mut source_to_target: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for mapping in &doc.mappings {
        for fm in &mapping.fields {
            if let (Some(ref src), Some(ref tgt)) = (&fm.source, &fm.target) {
                source_to_target.insert(src.clone(), tgt.clone());
            }
        }
    }

    let mut errors: Vec<String> = Vec::new();

    for (test_idx, test) in doc.tests.iter().enumerate() {
        for (dataset, expected) in &test.expected {
            let all_rows: Vec<&serde_json::Value> = expected
                .updates
                .iter()
                .chain(expected.inserts.iter())
                .collect();
            for row in all_rows {
                if let Some(obj) = row.as_object() {
                    check_nested_types(
                        obj,
                        &typed_fields,
                        &source_to_target,
                        &mut errors,
                        example_name,
                        test_idx + 1,
                        dataset,
                    );
                }
            }
        }
    }

    if !errors.is_empty() {
        let msg = errors.join("\n  ");
        panic!(
            "{example_name}: expected test data contains non-string values for fields without \
             `type:` on the target. The engine returns text for untyped fields, so these values \
             would never match.\n  {msg}\n\
             Fix: add `type: numeric` (or appropriate type) to the target field definition, \
             or use string values in expected."
        );
    }
}

/// Recursively check nested objects/arrays in expected test data for non-string scalars
/// that don't correspond to typed target fields.
fn check_nested_types(
    obj: &serde_json::Map<String, serde_json::Value>,
    typed_fields: &std::collections::HashSet<String>,
    source_to_target: &std::collections::HashMap<String, String>,
    errors: &mut Vec<String>,
    _example: &str,
    test_num: usize,
    dataset: &str,
) {
    for (key, value) in obj {
        if let serde_json::Value::Array(arr) = value {
            // Nested arrays contain objects that flow through JSONB — check their fields.
            for item in arr {
                if let Some(inner_obj) = item.as_object() {
                    check_nested_object_types(
                        inner_obj,
                        typed_fields,
                        source_to_target,
                        errors,
                        test_num,
                        dataset,
                        key,
                    );
                }
            }
        }
    }
}

/// Check leaf values inside a nested JSONB object for non-string types without target `type:`.
fn check_nested_object_types(
    obj: &serde_json::Map<String, serde_json::Value>,
    typed_fields: &std::collections::HashSet<String>,
    source_to_target: &std::collections::HashMap<String, String>,
    errors: &mut Vec<String>,
    test_num: usize,
    dataset: &str,
    array_name: &str,
) {
    for (key, value) in obj {
        match value {
            serde_json::Value::Number(_) | serde_json::Value::Bool(_) => {
                // Resolve the target field name: the key might be a source column name.
                let target_field = source_to_target
                    .get(key)
                    .map(|s| s.as_str())
                    .unwrap_or(key.as_str());
                if !typed_fields.contains(target_field) {
                    let type_hint = match value {
                        serde_json::Value::Number(_) => "numeric",
                        serde_json::Value::Bool(_) => "boolean",
                        _ => "unknown",
                    };
                    errors.push(format!(
                        "test {test_num}, {dataset}.{array_name}[].{key}: \
                         value {value} is {type_hint} but target field '{target_field}' \
                         has no `type:` declaration"
                    ));
                }
            }
            serde_json::Value::Array(arr) => {
                // Recurse into deeper nested arrays.
                for item in arr {
                    if let Some(inner_obj) = item.as_object() {
                        check_nested_object_types(
                            inner_obj,
                            typed_fields,
                            source_to_target,
                            errors,
                            test_num,
                            dataset,
                            key,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Infer Postgres column types from JSON test data values.
///
/// Scans all rows for each column and picks the narrowest compatible type:
/// - All String (or null) → TEXT
/// - All Number (or null) → NUMERIC
///
/// Read a column from a PostgreSQL row as Option<String>, handling multiple types.
fn get_text(row: &tokio_postgres::Row, col: &str) -> Option<String> {
    row.try_get::<_, Option<String>>(col)
        .ok()
        .flatten()
        .or_else(|| {
            row.try_get::<_, Option<i64>>(col)
                .ok()
                .flatten()
                .map(|n| n.to_string())
        })
        .or_else(|| {
            row.try_get::<_, Option<f64>>(col)
                .ok()
                .flatten()
                .map(|n| n.to_string())
        })
        .or_else(|| {
            row.try_get::<_, Option<bool>>(col)
                .ok()
                .flatten()
                .map(|b| b.to_string())
        })
        .or_else(|| {
            // Fallback for NUMERIC and other types: read as JSON value
            row.try_get::<_, Option<serde_json::Value>>(col)
                .ok()
                .flatten()
                .and_then(|v| match v {
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    serde_json::Value::String(s) => Some(s),
                    serde_json::Value::Bool(b) => Some(b.to_string()),
                    serde_json::Value::Null => None,
                    other => Some(other.to_string()),
                })
        })
}

/// Infer PostgreSQL column types from JSON test data.
/// - All null → TEXT
fn infer_column_types(
    columns: &[String],
    rows: &[serde_json::Value],
) -> std::collections::HashMap<String, &'static str> {
    let mut types: std::collections::HashMap<String, &'static str> =
        std::collections::HashMap::new();

    for col in columns {
        let mut seen: Option<&str> = None;
        let mut mixed = false;

        for row in rows {
            let val = row.as_object().and_then(|obj| obj.get(col.as_str()));
            let kind = match val {
                None | Some(serde_json::Value::Null) => continue,
                Some(serde_json::Value::String(_)) => "TEXT",
                Some(serde_json::Value::Number(n)) => {
                    if n.is_f64() && !n.is_i64() && !n.is_u64() {
                        "DOUBLE PRECISION"
                    } else {
                        "BIGINT"
                    }
                }
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

        types.insert(
            col.clone(),
            if mixed {
                "TEXT"
            } else {
                seen.unwrap_or("TEXT")
            },
        );
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

    let host = container
        .get_host()
        .await
        .expect("Failed to determine container host")
        .to_string();
    let host_port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Failed to map Postgres host port");

    let mut config = tokio_postgres::Config::new();
    config
        .host(&host)
        .port(host_port)
        .user("postgres")
        .password("postgres")
        .dbname("postgres");

    let (client, connection) = config
        .connect(NoTls)
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
    let qi = osi_engine::qi;
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
                format!("{} {pg_type}", qi(c))
            }))
            .collect();
        // DROP CASCADE to remove dependent views, then re-create.
        // Drop any stale view first — a previous example may have created a
        // VIEW with the same name (e.g. target identity view "person"), and
        // DROP TABLE IF EXISTS fails when the name is a view, not a table.
        let _ = client
            .execute(&format!("DROP VIEW IF EXISTS {} CASCADE", qi(dataset)), &[])
            .await;
        client
            .execute(
                &format!("DROP TABLE IF EXISTS {} CASCADE", qi(dataset)),
                &[],
            )
            .await
            .unwrap_or_else(|e| panic!("DROP TABLE {dataset}: {e}"));
        let create_sql = format!(
            "CREATE TABLE {} ({cols})",
            qi(dataset),
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
                    "INSERT INTO {} ({cols}) VALUES ({vals})",
                    qi(dataset),
                    cols = columns.iter().map(|c| qi(c)).collect::<Vec<_>>().join(", "),
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

/// Read all columns from a delta-view row into a JSON map.
/// Skips `_action` always. Skips `_base` unless `include_base` is true.
/// Skips `_cluster_id` unless `include_cluster_id` is true.
fn delta_row_to_map(
    row: &tokio_postgres::Row,
    include_base: bool,
    include_cluster_id: bool,
) -> serde_json::Map<String, serde_json::Value> {
    // Row is from: SELECT row_to_json(d.*) AS _json FROM delta_view d WHERE ...
    // So we get a single JSONB column containing all fields with native types.
    let json_val: serde_json::Value = row.get("_json");
    let obj = json_val
        .as_object()
        .expect("row_to_json should produce object");
    let mut map = serde_json::Map::new();
    for (k, v) in obj {
        if k == "_action" {
            continue;
        }
        if k == "_base" && !include_base {
            continue;
        }
        if k == "_cluster_id" && !include_cluster_id {
            continue;
        }
        map.insert(k.clone(), v.clone());
    }
    map
}

/// Ensure source tables have all columns referenced by mappings (PK, field sources,
/// timestamps). Adds missing columns to tables that were created from empty test data.
async fn ensure_source_columns(
    client: &tokio_postgres::Client,
    doc: &osi_engine::model::MappingDocument,
    input: &indexmap::IndexMap<String, Vec<serde_json::Value>>,
) {
    for (dataset, _) in input {
        // Collect all columns the mapping references for this source.
        let mut needed: Vec<String> = Vec::new();
        if let Some(src) = doc.sources.get(dataset.as_str()) {
            for col in src.primary_key.columns() {
                if !needed.contains(&col.to_string()) {
                    needed.push(col.to_string());
                }
            }
        }
        for mapping in &doc.mappings {
            if mapping.source.dataset != *dataset {
                continue;
            }
            // Skip nested-path mappings — their field sources are JSONB item
            // fields or parent_field aliases, not real table columns.
            if mapping.source.path.is_some() {
                continue;
            }
            for fm in &mapping.fields {
                if let Some(ref src) = fm.source {
                    if !needed.contains(src) {
                        needed.push(src.clone());
                    }
                }
                if let Some(ref lm) = fm.last_modified {
                    if let Some(field_name) = lm.field_name() {
                        let s = field_name.to_string();
                        if !needed.contains(&s) {
                            needed.push(s);
                        }
                    }
                }
            }
            // Mapping-level last_modified
            if let Some(ref lm) = mapping.last_modified {
                if let Some(field_name) = lm.field_name() {
                    let s = field_name.to_string();
                    if !needed.contains(&s) {
                        needed.push(s);
                    }
                }
            }
            // Passthrough columns
            for col in &mapping.passthrough {
                if !needed.contains(col) {
                    needed.push(col.clone());
                }
            }
        }
        // Add any missing columns to the table.
        for col in &needed {
            let sql = format!(
                "ALTER TABLE {} ADD COLUMN IF NOT EXISTS {} TEXT",
                osi_engine::qi(dataset),
                osi_engine::qi(col)
            );
            let _ = client.execute(&sql, &[]).await;
        }
    }
}

/// Ensure cluster_members tables exist for mappings that declare them.
/// If the test input already created the table (with data rows), skip it.
/// If the input has the table with an empty array, the generic loader
/// creates it with only `_row_id` — re-create with the proper schema.
/// Otherwise create an empty table so the forward view's LEFT JOIN succeeds.
async fn ensure_cluster_members_tables(
    client: &tokio_postgres::Client,
    doc: &osi_engine::model::MappingDocument,
    input: &indexmap::IndexMap<String, Vec<serde_json::Value>>,
) {
    for mapping in &doc.mappings {
        if let Some(ref cm) = mapping.cluster_members {
            let table = cm.table_name(&mapping.name);
            let needs_create = match input.get(&table) {
                None => true,                          // not in input at all
                Some(rows) if rows.is_empty() => true, // empty array — generic loader has wrong schema
                _ => false,                            // has data — generic loader inferred columns
            };
            if needs_create {
                let qi = osi_engine::qi;
                client
                    .execute(&format!("DROP TABLE IF EXISTS {} CASCADE", qi(&table)), &[])
                    .await
                    .unwrap();
                client
                    .execute(
                        &format!(
                            "CREATE TABLE {} ({} TEXT, {} TEXT)",
                            qi(&table),
                            qi(&cm.cluster_id),
                            qi(&cm.source_key),
                        ),
                        &[],
                    )
                    .await
                    .unwrap();
            }
        }
    }
}

async fn ensure_written_state_tables(
    client: &tokio_postgres::Client,
    doc: &osi_engine::model::MappingDocument,
    input: &indexmap::IndexMap<String, Vec<serde_json::Value>>,
) {
    for mapping in &doc.mappings {
        if let Some(ref ws) = mapping.written_state {
            let table = ws.table_name(&mapping.name);
            let qi = osi_engine::qi;
            // Always (re)create with proper types — JSONB not TEXT.
            client
                .execute(&format!("DROP TABLE IF EXISTS {} CASCADE", qi(&table)), &[])
                .await
                .unwrap();
            client
                .execute(
                    &format!(
                        "CREATE TABLE {} ({} TEXT PRIMARY KEY, {} JSONB NOT NULL, {} TIMESTAMPTZ NOT NULL DEFAULT now(), {} JSONB NOT NULL DEFAULT '{{}}'::jsonb)",
                        qi(&table),
                        qi(&ws.cluster_id),
                        qi(&ws.written),
                        qi(&ws.written_at),
                        qi(&ws.written_ts),
                    ),
                    &[],
                )
                .await
                .unwrap();
            // Skip populating here if input has data — that happens in
            // populate_written_state_tables after views exist (so that
            // _cluster_id seeds like "crm:1" can be resolved).
            if input.get(&table).is_none() {
                // No data for this table — nothing to do.
            }
        }
    }
}

/// Populate written-state tables from test input AFTER views have been created.
/// Resolves `_cluster_id` seeds (e.g. "crm:1") via the identity view.
async fn populate_written_state_tables(
    client: &tokio_postgres::Client,
    doc: &osi_engine::model::MappingDocument,
    input: &indexmap::IndexMap<String, Vec<serde_json::Value>>,
) {
    let qi = osi_engine::qi;
    for mapping in &doc.mappings {
        if let Some(ref ws) = mapping.written_state {
            let table = ws.table_name(&mapping.name);
            if let Some(rows) = input.get(&table) {
                for row in rows {
                    if let Some(obj) = row.as_object() {
                        let raw_cluster_id = obj
                            .get(&ws.cluster_id)
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        // When derive_timestamps is active, the written table is
                        // joined in the forward view on the cluster_field value —
                        // use the raw seed directly.  Otherwise resolve the seed
                        // via the identity view (e.g. "crm:1" → entity hash).
                        let cluster_id = if mapping.derive_timestamps
                            && mapping.cluster_field.is_some()
                        {
                            raw_cluster_id.to_string()
                        } else {
                            resolve_cluster_id(client, raw_cluster_id, mapping.target.name()).await
                        };
                        let written_str = obj
                            .get(&ws.written)
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");
                        let written_json: serde_json::Value =
                            serde_json::from_str(written_str).unwrap_or(serde_json::json!({}));

                        // Optional _written_at timestamp.
                        let written_at_str = obj.get(&ws.written_at).and_then(|v| v.as_str());
                        // Optional _written_ts per-field timestamps.
                        let written_ts_str = obj.get(&ws.written_ts).and_then(|v| v.as_str());
                        let written_ts_json: serde_json::Value = written_ts_str
                            .map(|s| serde_json::from_str(s).unwrap_or(serde_json::json!({})))
                            .unwrap_or(serde_json::json!({}));

                        if let Some(at_str) = written_at_str {
                            client
                                .execute(
                                    &format!(
                                        "INSERT INTO {} ({}, {}, {}, {}) VALUES ($1, $2, '{}'::timestamptz, $3)",
                                        qi(&table),
                                        qi(&ws.cluster_id),
                                        qi(&ws.written),
                                        qi(&ws.written_at),
                                        qi(&ws.written_ts),
                                        at_str.replace('\'', "''"),
                                    ),
                                    &[&cluster_id, &written_json, &written_ts_json],
                                )
                                .await
                                .unwrap();
                        } else {
                            client
                                .execute(
                                    &format!(
                                        "INSERT INTO {} ({}, {}, {}) VALUES ($1, $2, $3)",
                                        qi(&table),
                                        qi(&ws.cluster_id),
                                        qi(&ws.written),
                                        qi(&ws.written_ts),
                                    ),
                                    &[&cluster_id, &written_json, &written_ts_json],
                                )
                                .await
                                .unwrap();
                        }
                    }
                }
            }
        }
    }
}

async fn execute_views(client: &tokio_postgres::Client, sql: &str) {
    for stmt in split_sql_statements(sql) {
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
    eprintln!(
        "  {}",
        cols.iter()
            .map(|c| "-".repeat(c.len().max(8)))
            .collect::<Vec<_>>()
            .join("-+-")
    );
    // rows
    for row in &rows {
        let vals: Vec<String> = cols
            .iter()
            .enumerate()
            .map(|(i, _)| {
                // Try text first, fall back to i32/i64 for _row_id/_entity_id
                if let Ok(Some(s)) = row.try_get::<_, Option<String>>(i) {
                    s
                } else if let Ok(Some(v)) = row.try_get::<_, Option<serde_json::Value>>(i) {
                    serde_json::to_string(&v).unwrap()
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
    let sql = osi_engine::render::render_sql(&doc, &dag, false, false, false).expect("render");

    load_test_data(&client, &doc.tests[0].input).await;
    ensure_cluster_members_tables(&client, &doc, &doc.tests[0].input).await;
    ensure_written_state_tables(&client, &doc, &doc.tests[0].input).await;
    execute_views(&client, &sql).await;

    let views = [
        "_fwd_crm",
        "_fwd_erp",
        "_id_contact",
        "_resolved_contact",
        "contact",
        "_rev_crm",
        "_delta_crm",
        "_rev_erp",
        "_delta_erp",
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
    let sql = osi_engine::render::render_sql(&doc, &dag, false, false, false).expect("render");

    eprintln!("\n=== Generated SQL ===\n{sql}");

    load_test_data(&client, &doc.tests[0].input).await;
    execute_views(&client, &sql).await;

    let views = [
        "_fwd_crm_a",
        "_fwd_crm_b",
        "_id_person",
        "_resolved_person",
        "person",
        "_rev_crm_a",
        "_delta_crm_a",
        "_rev_crm_b",
        "_delta_crm_b",
    ];

    for view in &views {
        eprintln!("\n=== {view} ===");
        dump_view(&client, view).await;
    }
}

/// Dump all intermediate views for composite-keys.
#[tokio::test]
async fn dump_composite_keys_intermediates() {
    let (client, _container) = setup_pg().await;

    let mapping_path = examples_dir().join("composite-keys/mapping.yaml");
    let doc = osi_engine::parser::parse_file(&mapping_path).expect("parse");
    let dag = osi_engine::dag::build_dag(&doc);
    let sql = osi_engine::render::render_sql(&doc, &dag, false, false, false).expect("render");

    eprintln!("\n=== Generated SQL ===\n{sql}");

    load_test_data(&client, &doc.tests[0].input).await;
    ensure_cluster_members_tables(&client, &doc, &doc.tests[0].input).await;
    ensure_written_state_tables(&client, &doc, &doc.tests[0].input).await;
    execute_views(&client, &sql).await;

    let views = [
        "_fwd_erp_orders",
        "_fwd_erp_order_lines",
        "_fwd_crm_orders",
        "_fwd_crm_line_items",
        "_id_purchase_order",
        "_id_order_line",
        "_resolved_purchase_order",
        "_resolved_order_line",
        "purchase_order",
        "order_line",
        "_rev_erp_orders",
        "_delta_erp_orders",
        "_rev_erp_order_lines",
        "_delta_erp_order_lines",
        "_rev_crm_orders",
        "_delta_crm_orders",
        "_rev_crm_line_items",
        "_delta_crm_line_items",
    ];

    for view in &views {
        eprintln!("\n=== {view} ===");
        dump_view(&client, view).await;
    }
}

/// Dump intermediate views for relationship-mapping.
#[tokio::test]
async fn dump_relationship_mapping_intermediates() {
    let (client, _container) = setup_pg().await;

    let mapping_path = examples_dir().join("relationship-mapping/mapping.yaml");
    let doc = osi_engine::parser::parse_file(&mapping_path).expect("parse");
    let dag = osi_engine::dag::build_dag(&doc);
    let sql = osi_engine::render::render_sql(&doc, &dag, false, false, false).expect("render");

    load_test_data(&client, &doc.tests[0].input).await;
    ensure_cluster_members_tables(&client, &doc, &doc.tests[0].input).await;
    ensure_written_state_tables(&client, &doc, &doc.tests[0].input).await;
    ensure_source_columns(&client, &doc, &doc.tests[0].input).await;
    execute_views(&client, &sql).await;

    let views = ["_rev_crm_associations", "_delta_crm_associations"];

    for view in &views {
        eprintln!("\n=== {view} ===");
        dump_view(&client, view).await;
    }
}
#[tokio::test]
async fn dump_references_intermediates() {
    let (client, _container) = setup_pg().await;

    let mapping_path = examples_dir().join("references/mapping.yaml");
    let doc = osi_engine::parser::parse_file(&mapping_path).expect("parse");
    let dag = osi_engine::dag::build_dag(&doc);
    let sql = osi_engine::render::render_sql(&doc, &dag, false, false, false).expect("render");

    eprintln!("\n=== Generated SQL ===\n{sql}");

    load_test_data(&client, &doc.tests[0].input).await;
    ensure_cluster_members_tables(&client, &doc, &doc.tests[0].input).await;
    ensure_written_state_tables(&client, &doc, &doc.tests[0].input).await;
    execute_views(&client, &sql).await;

    let views = [
        "_fwd_crm_company",
        "_fwd_crm_contact",
        "_fwd_erp_customer",
        "_fwd_erp_contact",
        "_id_company",
        "_id_person",
        "_resolved_company",
        "_resolved_person",
        "company",
        "person",
        "_rev_crm_company",
        "_delta_crm_company",
        "_rev_crm_contact",
        "_delta_crm_contact",
        "_rev_erp_customer",
        "_delta_erp_customer",
        "_rev_erp_contact",
        "_delta_erp_contact",
    ];

    for view in &views {
        eprintln!("\n=== {view} ===");
        dump_view(&client, view).await;
    }
}
