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
/// No WHERE clause — all filtering deferred to delta.
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
    select_exprs.push("id._src_id".to_string());
    select_exprs.push("COALESCE(id._entity_id_resolved, r._entity_id) AS _cluster_id".to_string());

    let pk_columns: std::collections::HashSet<&str> = match source_meta {
        Some(src) => {
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
                if pk_columns.contains(s.as_str()) {
                    continue;
                }
                s.clone()
            }
            None => {
                if let Some(ref rev_expr) = fm.reverse_expression {
                    select_exprs.push(format!("{rev_expr} AS _rev_computed"));
                }
                continue;
            }
        };

        let target_field = fm.target.as_deref();

        let expr = if let Some(ref rev_expr) = fm.reverse_expression {
            rev_expr.clone()
        } else if let Some(tgt) = target_field {
            let field_def = target.and_then(|t| t.fields.get(tgt));
            let strategy = field_def.map(|f| f.strategy());
            let ref_target = field_def.and_then(|f| f.references());

            if let Some(ref_target_name) = ref_target {
                // Reference field: translate entity reference back to source namespace.
                // Requires explicit fm.references to know which mapping to resolve through.
                if let Some(ref ref_mapping_name) = fm.references {
                    let id_ref = format!("_id_{ref_target_name}");
                    format!(
                        "(SELECT ref_local._src_id \
                         FROM {id_ref} ref_match \
                         JOIN {id_ref} ref_local \
                           ON ref_local._entity_id_resolved = ref_match._entity_id_resolved \
                         WHERE ref_match._src_id = r.{tgt}::text \
                         AND ref_local._mapping = '{ref_mapping_name}' \
                         LIMIT 1)"
                    )
                } else {
                    // No explicit references — pass through raw value.
                    format!("r.{tgt}")
                }
            } else {
                match strategy {
                    Some(Strategy::Identity) | Some(Strategy::Collect) => {
                        format!("COALESCE(id.{tgt}, r.{tgt})")
                    }
                    _ => format!("r.{tgt}"),
                }
            }
        } else {
            continue;
        };

        select_exprs.push(format!("{expr} AS {source_name}"));
    }

    // _base: pass through from identity view (built in forward view).
    select_exprs.push("id._base".to_string());

    let sql = format!(
        "-- Reverse: {name} ({target_name} → {source})\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         SELECT\n  {columns}\n\
         FROM {resolved_view} AS r\n\
         LEFT JOIN {id_view} AS id\n  \
           ON id._entity_id_resolved = r._entity_id\n  \
           AND id._mapping = '{mapping_name}';\n",
        name = mapping.name,
        source = mapping.source.dataset,
        columns = select_exprs.join(",\n  "),
        mapping_name = mapping.name,
    );

    Ok(sql)
}
