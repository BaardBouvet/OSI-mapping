use anyhow::Result;

use crate::model::{Mapping, Source};

/// Render a delta view that classifies rows as updated, inserted, or deleted
/// by comparing the reverse view with the original source.
///
/// Produces: `CREATE OR REPLACE VIEW _delta_{mapping_name} AS ...`
///
/// Uses FULL OUTER JOIN on source identity (`_row_id` fallback or declared PK)
/// against reverse `_src_id` to detect:
/// - `update`: row exists in both source and reverse
/// - `insert`: row exists in reverse but not in source
/// - `delete`: row exists in source but not in reverse
pub fn render_delta_view(mapping: &Mapping, source_meta: Option<&Source>) -> Result<String> {
    let view_name = format!("_delta_{}", mapping.name);
    let rev_view = format!("_rev_{}", mapping.name);
    let source_table = source_meta
        .map(|s| s.table_name(&mapping.source.dataset).to_string())
        .unwrap_or_else(|| mapping.source.dataset.clone());

    let src_id_expr = source_meta
        .map(|s| s.primary_key.src_id_expr(Some("src")))
        .unwrap_or_else(|| "src._row_id::text".to_string());
    let src_missing_predicate = source_meta
        .map(|s| s.primary_key.src_missing_predicate(Some("src")))
        .unwrap_or_else(|| "src._row_id IS NULL".to_string());

    // Collect reverse-mapped source field names for change detection
    let reverse_fields: Vec<String> = mapping
        .fields
        .iter()
        .filter(|fm| fm.is_reverse() && fm.source.is_some())
        .filter_map(|fm| fm.source.clone())
        .collect();

    // Output columns from the reverse view
    let rev_col_list: String = reverse_fields
        .iter()
        .map(|f| format!("rev.{f}"))
        .collect::<Vec<_>>()
        .join(",\n  ");

    let sql = format!(
        "-- Delta: {name} (updates/inserts/deletes)\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         SELECT\n  \
           CASE\n    \
             WHEN {src_missing_predicate} THEN 'insert'\n    \
             WHEN rev._src_id IS NULL THEN 'delete'\n    \
             ELSE 'update'\n  \
           END AS _action,\n  \
           COALESCE(rev._src_id, {src_id_expr}) AS _src_id,\n  \
           {rev_cols}\n\
         FROM {source_table} AS src\n\
         FULL OUTER JOIN {rev_view} AS rev ON {src_id_expr} = rev._src_id;\n",
        name = mapping.name,
        rev_cols = rev_col_list,
    );

    Ok(sql)
}
