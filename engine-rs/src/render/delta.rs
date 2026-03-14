use anyhow::Result;

use crate::model::{Mapping, Source};

/// Render a delta view that classifies rows as insert/update/delete/noop.
///
/// Produces: `CREATE OR REPLACE VIEW _delta_{mapping_name} AS ...`
///
/// Single SELECT from the reverse view with a CASE expression:
/// - `_src_id IS NULL` → insert
/// - `reverse_required` field is NULL → delete
/// - `reverse_filter` fails → delete
/// - All fields match `_base` → noop
/// - Otherwise → update
pub fn render_delta_view(mapping: &Mapping, source_meta: Option<&Source>) -> Result<String> {
    let view_name = format!("_delta_{}", mapping.name);
    let rev_view = format!("_rev_{}", mapping.name);

    let pk_columns: std::collections::HashSet<&str> = source_meta
        .map(|src| src.primary_key.columns().into_iter().collect())
        .unwrap_or_default();

    let reverse_fields: Vec<String> = mapping
        .fields
        .iter()
        .filter(|fm| fm.is_reverse() && fm.source.is_some())
        .filter_map(|fm| fm.source.clone())
        .filter(|src| !pk_columns.contains(src.as_str()))
        .collect();

    // Delete conditions from reverse_required + reverse_filter.
    let mut delete_conditions: Vec<String> = Vec::new();
    for fm in &mapping.fields {
        if fm.reverse_required {
            if let Some(ref tgt) = fm.target {
                delete_conditions.push(format!("{tgt} IS NULL"));
            }
        }
    }
    if let Some(ref rf) = mapping.reverse_filter {
        delete_conditions.push(format!("NOT ({rf})"));
    }

    // Noop detection: compare each reverse-mapped field against _base.
    let noop_parts: Vec<String> = mapping
        .fields
        .iter()
        .filter(|fm| fm.is_reverse() && fm.source.is_some())
        .filter_map(|fm| {
            let src = fm.source.as_ref()?;
            if pk_columns.contains(src.as_str()) {
                return None;
            }
            Some(format!("_base->>'{src}' IS NOT DISTINCT FROM {src}::text"))
        })
        .collect();

    let mut branches = vec!["WHEN _src_id IS NULL THEN 'insert'".to_string()];
    if !delete_conditions.is_empty() {
        branches.push(format!("WHEN {} THEN 'delete'", delete_conditions.join(" OR ")));
    }
    if !noop_parts.is_empty() {
        branches.push(format!("WHEN {} THEN 'noop'", noop_parts.join(" AND ")));
    }
    branches.push("ELSE 'update'".to_string());

    let action_expr = format!(
        "CASE\n    {}\n  END",
        branches.join("\n    ")
    );

    let mut cols: Vec<String> = vec![
        format!("{action_expr} AS _action"),
        "_src_id".to_string(),
        "_cluster_id".to_string(),
    ];

    if let Some(src) = source_meta {
        for col in src.primary_key.columns() {
            cols.push(col.to_string());
        }
    }

    cols.extend(reverse_fields.iter().cloned());
    cols.push("_base".to_string());

    let sql = format!(
        "-- Delta: {name} (change detection)\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         SELECT\n  {columns}\n\
         FROM {rev_view};\n",
        name = mapping.name,
        columns = cols.join(",\n  "),
    );

    Ok(sql)
}
