use anyhow::Result;
use indexmap::IndexMap;

use crate::model::{Mapping, PrimaryKey, Source, Strategy, Target};
use crate::qi;

/// Build a map from PK column name to its base SQL expression (without AS alias).
///
/// Type resolution order for each PK column:
/// 1. Target field `type:` — if the PK column maps to a typed identity field
/// 2. Source `fields:` type — explicit column type on the source definition
/// 3. Default — plain text (no cast)
fn pk_base_expr_map(
    pk: &PrimaryKey,
    src_alias: &str,
    mapping: &Mapping,
    target: Option<&Target>,
    source_meta: Option<&Source>,
) -> IndexMap<String, String> {
    // Resolve the type for a PK column.
    let type_for_pk = |pk_col: &str| -> Option<&str> {
        // 1. Check if PK maps to a typed identity field on the target.
        let from_target = mapping.fields.iter()
            .find(|f| f.source.as_deref() == Some(pk_col))
            .and_then(|fm| fm.target.as_deref())
            .and_then(|tgt_name| target?.fields.get(tgt_name))
            .and_then(|fdef| {
                if fdef.strategy() == Strategy::Identity { fdef.field_type.as_deref() } else { None }
            });
        if from_target.is_some() { return from_target; }

        // 2. Check source-level fields type declaration.
        source_meta
            .and_then(|s| s.fields.get(pk_col))
            .and_then(|f| f.field_type.as_deref())
    };

    match pk {
        PrimaryKey::Single(col) => {
            let base = match type_for_pk(col) {
                Some(t) => format!("{src_alias}._src_id::{t}"),
                None => format!("{src_alias}._src_id"),
            };
            let mut map = IndexMap::new();
            map.insert(col.clone(), base);
            map
        }
        PrimaryKey::Composite(cols) => {
            let mut sorted: Vec<&str> = cols.iter().map(|c| c.as_str()).collect();
            sorted.sort();
            sorted.iter().map(|col| {
                let raw = format!("({src_alias}._src_id::jsonb->>'{col}')");
                let base = match type_for_pk(col) {
                    Some(t) => format!("{raw}::{t}"),
                    None => raw,
                };
                (col.to_string(), base)
            }).collect()
        }
    }
}

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
    all_mappings: &[Mapping],
    all_sources: &IndexMap<String, Source>,
) -> Result<String> {
    let view_name = qi(&format!("_rev_{}", mapping.name));
    let id_view = format!("_id_{target_name}");
    let resolved_view = qi(&format!("_resolved_{target_name}"));

    let mut select_exprs: Vec<String> = Vec::new();
    select_exprs.push("id._src_id".to_string());
    select_exprs.push("COALESCE(id._entity_id_resolved, r._entity_id) AS _cluster_id".to_string());

    // Build PK column base expressions (col → SQL expr without AS alias).
    let pk_base_map: IndexMap<String, String> = match source_meta {
        Some(src) => pk_base_expr_map(&src.primary_key, "id", mapping, target, source_meta),
        None => IndexMap::new(),
    };

    // Determine which PK columns have reverse field mappings.
    // These will be handled in the field loop with COALESCE(pk_extraction, field_expr)
    // so that insert rows (where _src_id is NULL) get resolved values.
    let pk_with_reverse: std::collections::HashSet<&str> = mapping.fields.iter()
        .filter(|f| f.is_reverse() && f.source.is_some())
        .filter_map(|f| f.source.as_deref())
        .filter(|s| pk_base_map.contains_key(*s))
        .collect();

    // Add plain PK extraction for columns NOT handled by the field loop.
    for (col, base) in &pk_base_map {
        if !pk_with_reverse.contains(col.as_str()) {
            select_exprs.push(format!("{base} AS {}", qi(col)));
        }
    }

    for fm in &mapping.fields {
        if !fm.is_reverse() {
            continue;
        }

        let source_name = match fm.source_name() {
            Some(s) => s.to_string(),
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

                    // For parent_fields references in nested sources the
                    // return value must match what the delta CTE joins on:
                    //  • Root referenced mapping  → _src_id (= PK as text);
                    //    the delta root join uses p.{pk}::text.
                    //  • Nested referenced mapping → identity field;
                    //    the delta intermediate join uses the first item_field
                    //    (= identity column) at that nesting level.
                    let is_parent_field = mapping.source.parent_fields.contains_key(&source_name);
                    let ref_mapping = all_mappings.iter().find(|m| m.name == *ref_mapping_name);
                    let return_expr = match &fm.references_field {
                        Some(rf) => format!("ref_local.{}", qi(rf)),
                        None if is_parent_field => {
                            let ref_is_nested = ref_mapping.map_or(false, |m| m.is_child());
                            if ref_is_nested && identity_fields.len() == 1 {
                                format!("ref_local.{}", qi(identity_fields[0]))
                            } else {
                                "ref_local._src_id".to_string()
                            }
                        }
                        None => {
                            // Auto-detect: if the referenced mapping has a single PK
                            // that maps to a typed identity field, return that field
                            // (preserves the declared type) instead of _src_id (always text).
                            let typed_identity = ref_mapping.and_then(|rm| {
                                let ref_source = all_sources.get(&rm.source.dataset)?;
                                match &ref_source.primary_key {
                                    PrimaryKey::Single(pk_col) => {
                                        // Find the field mapping where this PK column is the source
                                        let target_field_name = rm.fields.iter()
                                            .find(|f| f.source.as_deref() == Some(pk_col.as_str()))
                                            .and_then(|f| f.target.as_deref())?;
                                        // Check if that target field has a type declaration
                                        let ref_tgt = _all_targets.get(rm.target.name())?;
                                        let fdef = ref_tgt.fields.get(target_field_name)?;
                                        if fdef.field_type.is_some() && fdef.strategy() == Strategy::Identity {
                                            Some(target_field_name)
                                        } else {
                                            None
                                        }
                                    }
                                    PrimaryKey::Composite(_) => None,
                                }
                            });
                            match typed_identity {
                                Some(field_name) => format!("ref_local.{}", qi(field_name)),
                                None => "ref_local._src_id".to_string(),
                            }
                        }
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

        select_exprs.push(if let Some(pk_base) = pk_base_map.get(&source_name) {
            // PK column with reverse field mapping: COALESCE preserves PK for
            // updates while resolving through references/identity for inserts.
            format!("COALESCE({pk_base}, {expr}) AS {}", qi(&source_name))
        } else {
            format!("{expr} AS {}", qi(&source_name))
        });
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
