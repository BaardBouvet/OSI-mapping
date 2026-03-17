//! Property-based structural fuzzing for the render pipeline.
//!
//! Generates random but structurally valid mapping documents and verifies
//! that the full pipeline (parse → validate → DAG → render) does not panic
//! and produces well-formed SQL.

use proptest::prelude::*;

// ── Name generators ────────────────────────────────────────────────

fn ident_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,12}"
        .prop_filter("must not be empty", |s| !s.is_empty())
        .prop_map(|s| {
            // Avoid SQL reserved words that could confuse quoting
            match s.as_str() {
                "select" | "from" | "where" | "order" | "group" | "table" | "index" | "null"
                | "true" | "false" | "and" | "or" | "not" | "as" | "in" | "is" | "by" | "on"
                | "join" | "left" | "right" | "inner" | "outer" | "cross" | "case" | "when"
                | "then" | "else" | "end" | "create" | "drop" | "alter" | "insert" | "update"
                | "delete" | "set" | "into" | "values" | "having" | "limit" | "offset"
                | "union" | "all" | "distinct" | "exists" | "between" | "like" | "with"
                | "recursive" => format!("{s}_x"),
                _ => s,
            }
        })
}

// ── Strategy enum ──────────────────────────────────────────────────

fn strategy_name() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("identity"),
        Just("coalesce"),
        Just("last_modified"),
        Just("expression"),
        Just("bool_or"),
        Just("collect"),
    ]
}

// ── Field type ─────────────────────────────────────────────────────

fn optional_field_type() -> impl Strategy<Value = Option<&'static str>> {
    prop_oneof![
        Just(None),
        Just(Some("text")),
        Just(Some("integer")),
        Just(Some("numeric")),
        Just(Some("boolean")),
        Just(Some("date")),
    ]
}

// ── Target generation ──────────────────────────────────────────────

/// A generated target with name, field names, and strategies.
#[derive(Debug, Clone)]
struct GenTarget {
    name: String,
    fields: Vec<(String, String, Option<&'static str>)>, // (name, strategy, type)
}

fn gen_target() -> impl Strategy<Value = GenTarget> {
    (
        ident_name(),
        prop::collection::vec(
            (ident_name(), strategy_name(), optional_field_type()),
            1..=6,
        ),
    )
        .prop_map(|(name, raw_fields)| {
            // Deduplicate field names
            let mut seen = std::collections::HashSet::new();
            let mut fields: Vec<(String, String, Option<&str>)> = raw_fields
                .into_iter()
                .filter(|(n, _, _)| seen.insert(n.clone()))
                .map(|(n, s, t)| (n, s.to_string(), t))
                .collect();

            // Ensure at least one identity field
            if !fields.iter().any(|(_, s, _)| s == "identity") {
                if let Some(f) = fields.first_mut() {
                    f.1 = "identity".to_string();
                }
            }

            GenTarget { name, fields }
        })
}

// ── Mapping generation ─────────────────────────────────────────────

/// A generated mapping.
#[derive(Debug, Clone)]
struct GenMapping {
    name: String,
    source: String,
    target_idx: usize,
    field_indices: Vec<usize>, // indices into target's fields
    priority: Option<i64>,
    has_filter: bool,
}

fn gen_mappings(num_targets: usize) -> impl Strategy<Value = Vec<GenMapping>> {
    let target_range = 0..num_targets;
    prop::collection::vec(
        (
            ident_name(),
            ident_name(),
            target_range,
            prop::collection::vec(0..6usize, 1..=6),
            prop::option::of(1..100i64),
            any::<bool>(),
        ),
        1..=8,
    )
    .prop_map(|raw| {
        let mut seen = std::collections::HashSet::new();
        raw.into_iter()
            .filter(|(n, _, _, _, _, _)| seen.insert(n.clone()))
            .map(
                |(name, source, target_idx, field_indices, priority, has_filter)| GenMapping {
                    name,
                    source,
                    target_idx,
                    field_indices,
                    priority,
                    has_filter,
                },
            )
            .collect()
    })
}

// ── Document generation ────────────────────────────────────────────

/// A complete generated mapping document, ready to be rendered to YAML.
#[derive(Debug, Clone)]
struct GenDoc {
    targets: Vec<GenTarget>,
    mappings: Vec<GenMapping>,
}

fn gen_doc() -> impl Strategy<Value = GenDoc> {
    prop::collection::vec(gen_target(), 1..=4)
        .prop_filter("unique target names", |targets| {
            let mut seen = std::collections::HashSet::new();
            targets.iter().all(|t| seen.insert(&t.name))
        })
        .prop_flat_map(|targets| {
            let n = targets.len();
            (Just(targets), gen_mappings(n))
        })
        .prop_filter("at least one mapping", |(_, mappings)| !mappings.is_empty())
        .prop_map(|(targets, mappings)| GenDoc { targets, mappings })
}

/// Render a GenDoc to YAML string.
fn to_yaml(doc: &GenDoc) -> String {
    let mut yaml = String::from("version: \"1.0\"\n");

    // Sources — collect unique source names from mappings
    let sources: std::collections::BTreeSet<&str> =
        doc.mappings.iter().map(|m| m.source.as_str()).collect();
    yaml.push_str("sources:\n");
    for src in &sources {
        yaml.push_str(&format!("  {src}: {{ primary_key: id }}\n"));
    }

    // Targets
    yaml.push_str("targets:\n");
    for t in &doc.targets {
        yaml.push_str(&format!("  {}:\n    fields:\n", t.name));
        for (fname, strategy, ftype) in &t.fields {
            let mut parts = format!("strategy: {strategy}");
            if strategy == "expression" {
                parts.push_str(&format!(", expression: \"max({fname})\""));
            }
            if let Some(ty) = ftype {
                // Only apply type to identity fields
                if strategy == "identity" {
                    parts.push_str(&format!(", type: {ty}"));
                }
            }
            yaml.push_str(&format!("      {fname}: {{ {parts} }}\n"));
        }
    }

    // Mappings
    yaml.push_str("mappings:\n");
    for m in &doc.mappings {
        let target = &doc.targets[m.target_idx];
        yaml.push_str(&format!(
            "  - name: {}\n    source: {}\n    target: {}\n",
            m.name, m.source, target.name
        ));
        if let Some(p) = m.priority {
            yaml.push_str(&format!("    priority: {p}\n"));
        }
        if m.has_filter {
            yaml.push_str("    filter: \"id IS NOT NULL\"\n");
        }

        // Field mappings — map indices to actual target fields
        yaml.push_str("    fields:\n");
        let mut mapped = std::collections::HashSet::new();
        for &idx in &m.field_indices {
            let field_idx = idx % target.fields.len();
            if mapped.insert(field_idx) {
                let (fname, _, _) = &target.fields[field_idx];
                yaml.push_str(&format!("      - {{ source: {fname}, target: {fname} }}\n"));
            }
        }
        // If nothing was mapped (unlikely but possible with dedup), map first field
        if mapped.is_empty() {
            let (fname, _, _) = &target.fields[0];
            yaml.push_str(&format!("      - {{ source: {fname}, target: {fname} }}\n"));
        }
    }

    yaml
}

// ── SQL validation helpers ─────────────────────────────────────────

/// Check that parentheses are balanced (respecting single-quoted strings).
fn check_balanced_parens(sql: &str) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    for ch in sql.chars() {
        match ch {
            '\'' if !in_string => in_string = true,
            '\'' if in_string => in_string = false,
            '(' if !in_string => depth += 1,
            ')' if !in_string => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

/// Check there is no empty SELECT (SELECT immediately followed by FROM).
fn check_no_empty_select(sql: &str) -> bool {
    !sql.contains("SELECT\nFROM") && !sql.contains("SELECT FROM")
}

/// Check that every CREATE OR REPLACE VIEW name is unique.
fn check_unique_view_names(sql: &str) -> bool {
    let mut names = std::collections::HashSet::new();
    for line in sql.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("CREATE OR REPLACE VIEW ") {
            if let Some(name) = rest.split_whitespace().next() {
                if !names.insert(name.to_string()) {
                    return false;
                }
            }
        }
    }
    true
}

/// Check that the SQL contains BEGIN and COMMIT wrapping.
fn check_transaction_wrapping(sql: &str) -> bool {
    sql.contains("BEGIN;") && sql.contains("COMMIT;")
}

// ── Proptest ───────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn no_panics_on_random_docs(doc in gen_doc()) {
        let yaml = to_yaml(&doc);

        // Parse — may fail on edge cases (duplicate names etc.), that's fine
        let Ok(parsed) = osi_engine::parser::parse_str(&yaml) else {
            return Ok(());
        };

        // Validate — may produce warnings/errors, that's expected for random input
        let _ = osi_engine::validate::validate(&parsed);

        // Build DAG + render — must not panic
        let dag = osi_engine::dag::build_dag(&parsed);
        let result = osi_engine::render::render_sql(&parsed, &dag, false, false);

        if let Ok(sql) = &result {
            prop_assert!(
                check_balanced_parens(sql),
                "Unbalanced parentheses in generated SQL:\n{yaml}"
            );
            prop_assert!(
                check_no_empty_select(sql),
                "Empty SELECT in generated SQL:\n{yaml}"
            );
            prop_assert!(
                check_unique_view_names(sql),
                "Duplicate view names in generated SQL:\n{yaml}"
            );
            prop_assert!(
                check_transaction_wrapping(sql),
                "Missing BEGIN/COMMIT in generated SQL:\n{yaml}"
            );
        }
    }

    #[test]
    fn create_tables_flag_no_panics(doc in gen_doc()) {
        let yaml = to_yaml(&doc);
        let Ok(parsed) = osi_engine::parser::parse_str(&yaml) else {
            return Ok(());
        };

        let dag = osi_engine::dag::build_dag(&parsed);
        // Exercise the create_tables + annotate paths
        let result = osi_engine::render::render_sql(&parsed, &dag, true, true);

        if let Ok(sql) = &result {
            prop_assert!(check_balanced_parens(sql));
            prop_assert!(check_transaction_wrapping(sql));
            // create_tables should emit CREATE TABLE for each source
            for src in parsed.sources.keys() {
                prop_assert!(
                    sql.contains(&format!("CREATE TABLE IF NOT EXISTS \"{src}\"")),
                    "Missing CREATE TABLE for source {src}"
                );
            }
        }
    }

    #[test]
    fn dag_covers_all_targets(doc in gen_doc()) {
        let yaml = to_yaml(&doc);
        let Ok(parsed) = osi_engine::parser::parse_str(&yaml) else {
            return Ok(());
        };

        let dag = osi_engine::dag::build_dag(&parsed);
        let result = osi_engine::render::render_sql(&parsed, &dag, false, false);

        if let Ok(sql) = &result {
            // Collect which targets have at least one mapping
            let mapped_targets: std::collections::HashSet<&str> = parsed
                .mappings
                .iter()
                .map(|m| m.target.name())
                .collect();

            // Targets with mappings should have identity + resolution views
            for target_name in parsed.targets.keys() {
                if !mapped_targets.contains(target_name.as_str()) {
                    continue;
                }
                prop_assert!(
                    sql.contains(&format!("_id_{target_name}")),
                    "Missing identity view for target {target_name}"
                );
                prop_assert!(
                    sql.contains(&format!("_resolved_{target_name}")),
                    "Missing resolution view for target {target_name}"
                );
            }

            // Every mapping with fields should have a forward view
            for mapping in &parsed.mappings {
                if mapping.has_fields() {
                    prop_assert!(
                        sql.contains(&format!("_fwd_{}", mapping.name)),
                        "Missing forward view for mapping {}", mapping.name
                    );
                }
            }
        }
    }
}
