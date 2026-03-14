use anyhow::Result;

use crate::model::{Mapping, Source};

/// Render a delta view that classifies rows as updated, inserted, or deleted.
///
/// Produces: `CREATE OR REPLACE VIEW _delta_{mapping_name} AS ...`
///
/// The delta is a single SELECT from the reverse view (no second data source).
/// The reverse view emits ALL rows (no filtering), and the delta classifies:
/// - `_src_id IS NULL` → `insert` (entity exists but not in this source)
/// - `_src_id IS NOT NULL` and reverse filters pass → `update`
/// - `_src_id IS NOT NULL` and reverse filters fail → `delete`
///
/// This avoids a diamond dependency: delta depends only on reverse.
pub fn render_delta_view(mapping: &Mapping, source_meta: Option<&Source>) -> Result<String> {
    let view_name = format!("_delta_{}", mapping.name);
    let rev_view = format!("_rev_{}", mapping.name);

    // Collect PK column names for dedup
    let pk_columns: std::collections::HashSet<&str> = source_meta
        .map(|src| src.primary_key.columns().into_iter().collect())
        .unwrap_or_default();

    // Collect reverse-mapped source field names (excluding PK columns,
    // which are emitted separately via reverse_select_exprs)
    let reverse_fields: Vec<String> = mapping
        .fields
        .iter()
        .filter(|fm| fm.is_reverse() && fm.source.is_some())
        .filter_map(|fm| fm.source.clone())
        .filter(|src| !pk_columns.contains(src.as_str()))
        .collect();

    // Build the delete predicate from reverse_required + reverse_filter.
    // When the source row exists (_src_id IS NOT NULL) but these conditions
    // fail, the row is classified as a delete.
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

    // Build the CASE expression for _action.
    // Noop detection compares each reverse-mapped field against _base.
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
        let delete_pred = delete_conditions.join(" OR ");
        branches.push(format!("WHEN {delete_pred} THEN 'delete'"));
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

    // Include PK columns (human-readable aliases from reverse view)
    if let Some(src) = source_meta {
        for col in src.primary_key.columns() {
            cols.push(col.to_string());
        }
    }

    cols.extend(reverse_fields.iter().cloned());

    // _base: always pass through the JSONB column from reverse view
    cols.push("_base".to_string());

    let sql = format!(
        "-- Delta: {name} (updates/inserts/deletes)\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         SELECT\n  {columns}\n\
         FROM {rev_view};\n",
        name = mapping.name,
        columns = cols.join(",\n  "),
    );

    Ok(sql)
}
