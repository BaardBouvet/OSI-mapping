use anyhow::Result;
use indexmap::IndexMap;

use crate::model::{Mapping, Source, Strategy, Target};

/// Render a reverse mapping view that projects a resolved target back to source shape.
///
/// Produces: `CREATE OR REPLACE VIEW _rev_{mapping_name} AS ...`
///
/// The reverse view joins:
///   `_resolved_{target}` (for resolved non-identity values)
///   `_id_{target}` (LEFT JOIN for per-source-row identity values + entity assignment)
///
/// A LEFT JOIN from resolved → identity ensures that entities WITHOUT a member
/// from this mapping still produce a row (with `_src_id = NULL`).  The delta
/// view then classifies those as inserts.
///
/// For identity/collect-strategy fields: uses `COALESCE(id.{field}, r.{field})`
///   — source's own value when it exists, resolved value for insert rows.
/// For other fields: uses `r.{target_field}` (resolved/merged value).
/// For fields with `reverse_expression`: uses the expression as-is.
///
/// When `source_meta` is provided the original PK columns are restored from
/// `_src_id` (e.g. `id._src_id AS contact_id`).  Otherwise `_src_id` is
/// emitted verbatim for backward compatibility.
pub fn render_reverse_view(
    mapping: &Mapping,
    target_name: &str,
    target: Option<&Target>,
    _all_targets: &IndexMap<String, Target>,
    source_meta: Option<&Source>,
) -> Result<String> {
    let view_name = format!("_rev_{}", mapping.name);
    let id_view = format!("_id_{target_name}");
    let resolved_view = format!("_resolved_{target_name}");

    let mut select_exprs: Vec<String> = Vec::new();
    // Always emit _src_id (delta view joins on it; NULL for insert rows).
    select_exprs.push("id._src_id".to_string());
    // Emit cluster identity for insert feedback.
    // For insert rows the identity side is NULL, so fall back to r._entity_id.
    select_exprs.push("COALESCE(id._entity_id_resolved, r._entity_id) AS _cluster_id".to_string());
    // Collect PK column names so we can skip duplicate aliases below.
    let pk_columns: std::collections::HashSet<&str> = match source_meta {
        Some(src) => {
            // Also restore human-friendly PK column names alongside _src_id.
            select_exprs.extend(src.primary_key.reverse_select_exprs("id"));
            src.primary_key.columns().into_iter().collect()
        }
        None => std::collections::HashSet::new(),
    };

    for fm in &mapping.fields {
        if !fm.is_reverse() {
            continue;
        }

        let source_name = match &fm.source {
            Some(s) => {
                // Skip fields whose source column is already emitted as a PK column.
                if pk_columns.contains(s.as_str()) {
                    continue;
                }
                s.clone()
            }
            None => {
                // reverse_only with reverse_expression but no source name
                if let Some(ref rev_expr) = fm.reverse_expression {
                    select_exprs.push(format!("{rev_expr} AS _rev_computed"));
                }
                continue;
            }
        };

        let target_field = fm.target.as_deref();

        let expr = if let Some(ref rev_expr) = fm.reverse_expression {
            // Custom reverse expression — references target field names from r.*
            rev_expr.clone()
        } else if let Some(tgt) = target_field {
            // Determine strategy to choose between id.{field} and r.{field}
            let strategy = target.and_then(|t| t.fields.get(tgt)).map(|f| f.strategy());
            match strategy {
                Some(Strategy::Identity) | Some(Strategy::Collect) => {
                    // Source's own forward value when it exists, resolved for insert rows.
                    format!("COALESCE(id.{tgt}, r.{tgt})")
                }
                _ => {
                    // Use resolved value
                    format!("r.{tgt}")
                }
            }
        } else {
            continue;
        };

        select_exprs.push(format!("{expr} AS {source_name}"));
    }

    // FROM _resolved LEFT JOIN _id: every entity gets a row.
    // Entities with a member from this mapping: id.* is populated.
    // Entities without: id.* is NULL → delta classifies as insert.
    let mut sql = format!(
        "-- Reverse: {name} ({target_name} → {source})\n\
         CREATE OR REPLACE VIEW {view_name} AS\nSELECT\n  {columns}\n\
         FROM {resolved_view} AS r\n\
         LEFT JOIN {id_view} AS id\n  \
           ON id._entity_id_resolved = r._entity_id\n  \
           AND id._mapping = '{mapping_name}'",
        name = mapping.name,
        source = mapping.source.dataset,
        columns = select_exprs.join(",\n  "),
        mapping_name = mapping.name,
    );

    // No WHERE clause — the reverse view emits ALL rows.
    // Filtering (reverse_required / reverse_filter) is handled by the delta
    // view, which classifies filtered-out rows as deletes.  This avoids a
    // diamond dependency: delta depends only on reverse, not on identity.

    sql.push_str(";\n");

    Ok(sql)
}
