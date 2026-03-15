use anyhow::Result;
use indexmap::IndexMap;

use crate::model::{Mapping, Source, Strategy, Target};
use crate::qi;

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
    let view_name = qi(&format!("_rev_{}", mapping.name));
    let id_view = format!("_id_{target_name}");
    let resolved_view = qi(&format!("_resolved_{target_name}"));

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
                    let id_ref = qi(&format!("_id_{ref_target_name}"));

                    // Match on _src_id first (standard FK references), then
                    // fall back to identity fields (vocabulary-style references
                    // where the resolved value may differ from _src_id).
                    let ref_target_def = _all_targets.get(ref_target_name);
                    let identity_fields: Vec<&str> = ref_target_def
                        .map(|t| {
                            t.fields
                                .iter()
                                .filter(|(_, fd)| fd.strategy() == Strategy::Identity)
                                .map(|(name, _)| name.as_str())
                                .collect()
                        })
                        .unwrap_or_default();

                    let qtgt = qi(tgt);
                    let mut match_parts = vec![
                        format!("ref_match._src_id = r.{qtgt}::text"),
                    ];
                    for f in &identity_fields {
                        match_parts.push(format!("ref_match.{}::text = r.{qtgt}::text", qi(f)));
                    }
                    let match_clause = match_parts.join(" OR ");

                    // For parent_fields references in nested sources, the _src_id
                    // is the root document's PK — not the array item's identity.
                    // Return the identity field value instead.
                    let is_parent_field = mapping.source.parent_fields.contains_key(&source_name);
                    let return_expr = match &fm.references_field {
                        Some(rf) => format!("ref_local.{}", qi(rf)),
                        None if is_parent_field && identity_fields.len() == 1 => {
                            format!("ref_local.{}", qi(identity_fields[0]))
                        }
                        None => "ref_local._src_id".to_string(),
                    };

                    format!(
                        "(SELECT {return_expr} \
                         FROM {id_ref} ref_match \
                         JOIN {id_ref} ref_local \
                           ON ref_local._entity_id_resolved = ref_match._entity_id_resolved \
                         WHERE ({match_clause}) \
                         AND ref_local._mapping = '{ref_mapping_name}' \
                         LIMIT 1)",
                    )
                } else {
                    // No explicit references — pass through raw value.
                    format!("r.{}", qi(tgt))
                }
            } else {
                match strategy {
                    Some(Strategy::Identity) | Some(Strategy::Collect) => {
                        let qtgt = qi(tgt);
                        format!("COALESCE(id.{qtgt}, r.{qtgt})")
                    }
                    _ => format!("r.{}", qi(tgt)),
                }
            }
        } else {
            continue;
        };

        select_exprs.push(format!("{expr} AS {}", qi(&source_name)));
    }

    // _base: pass through from identity view (built in forward view).
    select_exprs.push("id._base".to_string());

    // Include target fields referenced by reverse_filter that aren't already projected.
    if let Some(ref rf) = mapping.reverse_filter {
        if let Some(tgt) = target {
            for (field_name, _) in &tgt.fields {
                if rf.contains(field_name.as_str()) {
                    // Check if already projected (as a source alias).
                    let qfn = qi(field_name);
                    let already = select_exprs.iter().any(|e| e.ends_with(&format!(" AS {qfn}")) || e == &qfn);
                    if !already {
                        select_exprs.push(format!("r.{qfn}"));
                    }
                }
            }
        }
    }

    // Build the identity subquery with only the columns we need.
    // This avoids ambiguity when reverse_expression references target field names
    // that also exist in the identity view (which passes through all forward columns).
    let mut id_cols: Vec<String> = vec![
        "_src_id".to_string(),
        "_mapping".to_string(),
        "_entity_id_resolved".to_string(),
        "_base".to_string(),
    ];
    // Add identity/collect target fields referenced via COALESCE(id.{tgt}, r.{tgt})
    for fm in &mapping.fields {
        if !fm.is_reverse() { continue; }
        if let Some(ref tgt) = fm.target {
            let field_def = target.and_then(|t| t.fields.get(tgt.as_str()));
            let strategy = field_def.map(|f| f.strategy());
            match strategy {
                Some(Strategy::Identity) | Some(Strategy::Collect) => {
                    let qtgt = qi(tgt);
                    if !id_cols.contains(&qtgt) {
                        id_cols.push(qtgt);
                    }
                }
                _ => {}
            }
        }
    }
    let qi_id_view = qi(&id_view);
    let id_subquery = format!(
        "(SELECT {cols} FROM {qi_id_view}) AS id",
        cols = id_cols.join(", "),
    );

    let sql = format!(
        "-- Reverse: {name} ({target_name} → {source})\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         SELECT\n  {columns}\n\
         FROM {resolved_view} AS r\n\
         LEFT JOIN {id_subquery}\n  \
           ON id._entity_id_resolved = r._entity_id\n  \
           AND id._mapping = '{mapping_name}';\n",
        name = mapping.name,
        source = mapping.source.dataset,
        columns = select_exprs.join(",\n  "),
        mapping_name = mapping.name,
    );

    Ok(sql)
}
