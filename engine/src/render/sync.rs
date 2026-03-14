use anyhow::Result;
use indexmap::IndexMap;

use crate::model::{Mapping, Source, Strategy, Target};

/// Render a sync view that projects the resolved golden record back to source
/// shape and classifies each row as insert/update/delete/noop.
///
/// Produces: `CREATE OR REPLACE VIEW sync_{mapping_name} AS ...`
///
/// Combines the former reverse and delta views into a single view with a CTE:
///   _rev CTE: LEFT JOIN _resolved → _id to project back to source shape
///   Outer SELECT: CASE expression classifying _action
pub fn render_sync_view(
    mapping: &Mapping,
    target_name: &str,
    target: Option<&Target>,
    _all_targets: &IndexMap<String, Target>,
    source_meta: Option<&Source>,
) -> Result<String> {
    let view_name = format!("sync_{}", mapping.name);
    let id_view = format!("_id_{target_name}");
    let resolved_view = format!("_resolved_{target_name}");

    // ── Build reverse SELECT expressions (the _rev CTE) ───────────────

    let mut rev_exprs: Vec<String> = Vec::new();
    rev_exprs.push("id._src_id".to_string());
    rev_exprs.push("COALESCE(id._entity_id_resolved, r._entity_id) AS _cluster_id".to_string());

    let pk_columns: std::collections::HashSet<&str> = match source_meta {
        Some(src) => {
            rev_exprs.extend(src.primary_key.reverse_select_exprs("id"));
            src.primary_key.columns().into_iter().collect()
        }
        None => std::collections::HashSet::new(),
    };

    // Collect reverse-mapped field names (excluding PKs) for outer SELECT
    let mut reverse_fields: Vec<String> = Vec::new();

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
                    rev_exprs.push(format!("{rev_expr} AS _rev_computed"));
                }
                continue;
            }
        };

        let target_field = fm.target.as_deref();

        let expr = if let Some(ref rev_expr) = fm.reverse_expression {
            rev_expr.clone()
        } else if let Some(tgt) = target_field {
            let strategy = target.and_then(|t| t.fields.get(tgt)).map(|f| f.strategy());
            match strategy {
                Some(Strategy::Identity) | Some(Strategy::Collect) => {
                    format!("COALESCE(id.{tgt}, r.{tgt})")
                }
                _ => format!("r.{tgt}"),
            }
        } else {
            continue;
        };

        rev_exprs.push(format!("{expr} AS {source_name}"));
        reverse_fields.push(source_name);
    }

    // Always pass through _base
    rev_exprs.push("id._base".to_string());

    // ── Build outer SELECT (action classification) ─────────────────────

    // Delete conditions
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

    // Noop conditions
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

    // Outer columns
    let mut outer_cols: Vec<String> = vec![
        format!("{action_expr} AS _action"),
        "_src_id".to_string(),
        "_cluster_id".to_string(),
    ];
    if let Some(src) = source_meta {
        for col in src.primary_key.columns() {
            outer_cols.push(col.to_string());
        }
    }
    outer_cols.extend(reverse_fields.iter().cloned());
    outer_cols.push("_base".to_string());

    // ── Assemble the full SQL ──────────────────────────────────────────

    let sql = format!(
        "-- Sync: {name} ({target_name} → {source})\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         WITH _rev AS (\n\
         SELECT\n  {rev_columns}\n\
         FROM {resolved_view} AS r\n\
         LEFT JOIN {id_view} AS id\n  \
           ON id._entity_id_resolved = r._entity_id\n  \
           AND id._mapping = '{mapping_name}'\n\
         )\n\
         SELECT\n  {outer_columns}\n\
         FROM _rev;\n",
        name = mapping.name,
        source = mapping.source.dataset,
        rev_columns = rev_exprs.join(",\n  "),
        mapping_name = mapping.name,
        outer_columns = outer_cols.join(",\n  "),
    );

    Ok(sql)
}
