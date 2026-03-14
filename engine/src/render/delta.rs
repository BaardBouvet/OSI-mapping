use anyhow::Result;

use crate::model::Mapping;

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
pub fn render_delta_view(mapping: &Mapping) -> Result<String> {
    let view_name = format!("_delta_{}", mapping.name);
    let rev_view = format!("_rev_{}", mapping.name);

    // Collect reverse-mapped source field names
    let reverse_fields: Vec<String> = mapping
        .fields
        .iter()
        .filter(|fm| fm.is_reverse() && fm.source.is_some())
        .filter_map(|fm| fm.source.clone())
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

    // Build the CASE expression for _action
    let action_expr = if delete_conditions.is_empty() {
        "CASE WHEN _src_id IS NULL THEN 'insert' ELSE 'update' END".to_string()
    } else {
        let delete_pred = delete_conditions.join(" OR ");
        format!(
            "CASE\n    \
             WHEN _src_id IS NULL THEN 'insert'\n    \
             WHEN {delete_pred} THEN 'delete'\n    \
             ELSE 'update'\n  \
           END"
        )
    };

    let mut cols: Vec<String> = vec![
        format!("{action_expr} AS _action"),
        "_src_id".to_string(),
        "_cluster_id".to_string(),
    ];
    cols.extend(reverse_fields.iter().cloned());

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
