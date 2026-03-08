use anyhow::Result;
use indexmap::IndexMap;

use crate::model::{Mapping, Strategy, Target};

/// Render a reverse mapping view that projects a resolved target back to source shape.
///
/// Produces: `CREATE OR REPLACE VIEW _rev_{mapping_name} AS ...`
///
/// The reverse view joins:
///   `_id_{target}` (for per-source-row identity values + entity assignment)
///   `_resolved_{target}` (for resolved non-identity values)
///
/// For identity-strategy fields: uses `id.{target_field}` (source's own forward value).
/// For other fields: uses `r.{target_field}` (resolved/merged value).
/// For fields with `reverse_expression`: uses the expression as-is.
pub fn render_reverse_view(
    mapping: &Mapping,
    target_name: &str,
    target: Option<&Target>,
    _all_targets: &IndexMap<String, Target>,
) -> Result<String> {
    let view_name = format!("_rev_{}", mapping.name);
    let id_view = format!("_id_{target_name}");
    let resolved_view = format!("_resolved_{target_name}");

    let mut select_exprs: Vec<String> = Vec::new();
    select_exprs.push("id._src_id".to_string());

    for fm in &mapping.fields {
        if !fm.is_reverse() {
            continue;
        }

        let source_name = match &fm.source {
            Some(s) => s.clone(),
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
                    // Use source's own forward value (from identity view)
                    format!("id.{tgt}")
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

    let mut sql = format!(
        "-- Reverse: {name} ({target_name} → {source})\n\
         CREATE OR REPLACE VIEW {view_name} AS\nSELECT\n  {columns}\n\
         FROM {id_view} AS id\n\
         JOIN {resolved_view} AS r ON r._entity_id = id._entity_id_resolved\n\
         WHERE id._mapping = '{mapping_name}'",
        name = mapping.name,
        source = mapping.source.dataset,
        columns = select_exprs.join(",\n  "),
        mapping_name = mapping.name,
    );

    // reverse_required: exclude rows where required target fields are null
    let required_fields: Vec<String> = mapping
        .fields
        .iter()
        .filter(|fm| fm.reverse_required)
        .filter_map(|fm| fm.target.clone())
        .collect();

    for rf in &required_fields {
        sql.push_str(&format!("\n  AND r.{rf} IS NOT NULL"));
    }

    // reverse_filter
    if let Some(ref rf) = mapping.reverse_filter {
        sql.push_str(&format!("\n  AND ({rf})"));
    }

    sql.push_str(";\n");

    Ok(sql)
}
