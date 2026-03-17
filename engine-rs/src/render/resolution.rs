use anyhow::Result;
use indexmap::IndexMap;

use crate::model::{Strategy, Target};
use crate::qi;

/// Render a resolution view that merges contributions from multiple mappings
/// into a single golden record per entity.
///
/// Produces: `CREATE OR REPLACE VIEW _resolved_{target_name} AS ...`
///
/// Groups by `_entity_id_resolved` from the identity view and applies
/// per-field aggregation based on each field's strategy:
/// - `identity`: `min(F)` — representative scalar value
/// - `collect`:  `array_agg(DISTINCT F)` — all unique values
/// - `coalesce`: first non-null by priority (`_priority_F`, then `_priority`)
/// - `last_modified`: first non-null by timestamp (`_ts_F`, then `_last_modified`)
/// - `expression`: custom SQL aggregation expression
///
/// Fields with `group:` resolve atomically — all fields in a group come from
/// the same winning source row via a DISTINCT ON CTE.
pub fn render_resolution_view(
    target_name: &str,
    target: &Target,
    _mappings: &[&crate::model::Mapping],
    _all_targets: &IndexMap<String, Target>,
) -> Result<String> {
    let view_name = qi(&format!("_resolved_{target_name}"));
    let id_view = qi(&format!("_id_{target_name}"));

    let has_default_expr = target
        .fields
        .values()
        .any(|f| f.default_expression().is_some());

    // ── Collect groups ──────────────────────────────────────────────
    // group_name → Vec<(field_name, strategy)>
    let mut groups: IndexMap<String, Vec<(String, Strategy)>> = IndexMap::new();
    for (fname, fdef) in &target.fields {
        if let Some(g) = fdef.group() {
            groups
                .entry(g.to_string())
                .or_default()
                .push((fname.clone(), fdef.strategy()));
        }
    }

    let mut sql = format!(
        "-- Resolution: {target_name}\n\
         CREATE OR REPLACE VIEW {view_name} AS\n"
    );

    // ── Group CTEs ──────────────────────────────────────────────────
    let mut group_ctes: Vec<String> = Vec::new();
    let grouped_fields: std::collections::HashSet<&str> = groups
        .values()
        .flat_map(|fields| fields.iter().map(|(f, _)| f.as_str()))
        .collect();

    for (group_name, fields) in &groups {
        let cte_alias = format!("_grp_{group_name}");

        // Collect SELECT columns: _entity_id_resolved + all group fields.
        let mut cte_cols: Vec<String> = vec!["_entity_id_resolved".to_string()];
        for (fname, _) in fields {
            cte_cols.push(qi(fname));
        }

        // Determine ordering: use the group's dominant strategy.
        let dominant = fields[0].1;
        let order_expr = match dominant {
            Strategy::LastModified => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(f, _)| format!("COALESCE({}, _last_modified)", qi(&format!("_ts_{f}"))))
                    .collect();
                format!("GREATEST({}) DESC NULLS LAST", parts.join(", "))
            }
            Strategy::Coalesce => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(f, _)| {
                        format!(
                            "COALESCE({}, _priority, 999)",
                            qi(&format!("_priority_{f}"))
                        )
                    })
                    .collect();
                format!("LEAST({}) ASC NULLS LAST", parts.join(", "))
            }
            _ => {
                // For identity or other strategies, just use min-like ordering.
                // This shouldn't normally happen (groups are for last_modified/coalesce).
                "1".to_string()
            }
        };

        // WHERE: at least one group field is non-NULL.
        let null_checks: Vec<String> = fields
            .iter()
            .map(|(f, _)| format!("{} IS NOT NULL", qi(f)))
            .collect();

        group_ctes.push(format!(
            "{cte_alias} AS (\n    \
             SELECT DISTINCT ON (_entity_id_resolved)\n      \
             {cols}\n    \
             FROM {id_view}\n    \
             WHERE {where_clause}\n    \
             ORDER BY _entity_id_resolved, {order_expr}\n  )",
            cte_alias = qi(&cte_alias),
            cols = cte_cols.join(", "),
            where_clause = null_checks.join(" OR "),
        ));
    }

    // ── Main aggregation expressions ────────────────────────────────
    let has_groups = !groups.is_empty();

    let mut agg_exprs: Vec<String> = Vec::new();
    if has_groups {
        agg_exprs.push(format!("{id_view}._entity_id_resolved AS _entity_id"));
    } else {
        agg_exprs.push("_entity_id_resolved AS _entity_id".to_string());
    }

    let mut outer_defaults: Vec<(String, String)> = Vec::new();

    for (fname, fdef) in &target.fields {
        let qfname = qi(fname);

        // Grouped fields are handled via CTE join — skip aggregation.
        if grouped_fields.contains(fname.as_str()) {
            let group_name = fdef.group().unwrap();
            let cte_alias = qi(&format!("_grp_{group_name}"));
            let expr = format!("{cte_alias}.{qfname}");

            if has_default_expr {
                if let Some(de) = fdef.default_expression() {
                    outer_defaults.push((fname.clone(), de.to_string()));
                    agg_exprs.push(format!("{expr} AS {qfname}"));
                } else if let Some(default_val) = fdef.default_value() {
                    let val_str = default_val_to_sql(default_val);
                    agg_exprs.push(format!("COALESCE({expr}, {val_str}) AS {qfname}"));
                } else {
                    agg_exprs.push(format!("{expr} AS {qfname}"));
                }
            } else if let Some(default_val) = fdef.default_value() {
                let val_str = default_val_to_sql(default_val);
                agg_exprs.push(format!("COALESCE({expr}, {val_str}) AS {qfname}"));
            } else {
                agg_exprs.push(format!("{expr} AS {qfname}"));
            }
            continue;
        }

        let strategy = fdef.strategy();
        let base_expr = match strategy {
            Strategy::Identity => format!("min({qfname})"),
            Strategy::Collect => {
                format!("array_agg(DISTINCT {qfname}) FILTER (WHERE {qfname} IS NOT NULL)")
            }
            Strategy::Coalesce => format!(
                "(array_agg({qfname} ORDER BY COALESCE({}, _priority, 999) ASC NULLS LAST) \
                 FILTER (WHERE {qfname} IS NOT NULL))[1]",
                qi(&format!("_priority_{fname}"))
            ),
            Strategy::LastModified => format!(
                "(array_agg({qfname} ORDER BY COALESCE({}, _last_modified) DESC NULLS LAST) \
                 FILTER (WHERE {qfname} IS NOT NULL))[1]",
                qi(&format!("_ts_{fname}"))
            ),
            Strategy::Expression => {
                let default_e = format!("max({qfname})");
                let agg_expr = fdef.expression().unwrap_or(&default_e);
                agg_expr.to_string()
            }
            Strategy::BoolOr => format!("bool_or(({qfname})::boolean)"),
        };

        if has_default_expr {
            if let Some(de) = fdef.default_expression() {
                outer_defaults.push((fname.clone(), de.to_string()));
                agg_exprs.push(format!("{base_expr} AS {qfname}"));
            } else if let Some(default_val) = fdef.default_value() {
                let val_str = default_val_to_sql(default_val);
                agg_exprs.push(format!("COALESCE(({base_expr}), {val_str}) AS {qfname}"));
            } else {
                agg_exprs.push(format!("{base_expr} AS {qfname}"));
            }
        } else {
            let expr = if let Some(default_val) = fdef.default_value() {
                let val_str = default_val_to_sql(default_val);
                format!("COALESCE(({base_expr}), {val_str}) AS {qfname}")
            } else {
                format!("{base_expr} AS {qfname}")
            };
            agg_exprs.push(expr);
        }
    }

    // ── Assemble SQL ────────────────────────────────────────────────
    // Build GROUP BY: _entity_id_resolved + all grouped field references.
    let mut group_by_extra: Vec<String> = Vec::new();
    for (group_name, fields) in &groups {
        let cte_alias = qi(&format!("_grp_{group_name}"));
        for (fname, _) in fields {
            group_by_extra.push(format!("{cte_alias}.{}", qi(fname)));
        }
    }

    // Build LEFT JOINs for group CTEs.
    let joins: Vec<String> = groups
        .keys()
        .map(|g| {
            let cte_alias = qi(&format!("_grp_{g}"));
            format!(
                "LEFT JOIN {cte_alias} ON {cte_alias}._entity_id_resolved = {id_view}._entity_id_resolved"
            )
        })
        .collect();

    let needs_cte = has_default_expr || !group_ctes.is_empty();

    if needs_cte {
        let mut all_ctes = group_ctes;

        // Inner aggregation query.
        let eid_ref = if has_groups {
            format!("{id_view}._entity_id_resolved")
        } else {
            "_entity_id_resolved".to_string()
        };
        let group_by_clause = if group_by_extra.is_empty() {
            format!("GROUP BY {eid_ref}")
        } else {
            format!("GROUP BY {eid_ref}, {}", group_by_extra.join(", "))
        };

        let from_clause = if joins.is_empty() {
            id_view
        } else {
            format!("{id_view}\n  {}", joins.join("\n  "))
        };

        if has_default_expr {
            // CTE approach: aggregate first, then apply default_expressions in outer SELECT.
            all_ctes.push(format!(
                "_agg AS (\n  SELECT\n    {agg_columns}\n  FROM {from_clause}\n  {group_by}\n)",
                agg_columns = agg_exprs.join(",\n    "),
                group_by = group_by_clause,
            ));

            let mut outer_exprs: Vec<String> = Vec::new();
            outer_exprs.push("_entity_id".to_string());
            for (fname, _fdef) in &target.fields {
                let qfname = qi(fname);
                if let Some((_, de)) = outer_defaults.iter().find(|(n, _)| n == fname) {
                    outer_exprs.push(format!("COALESCE({qfname}, {de}) AS {qfname}"));
                } else {
                    outer_exprs.push(qfname);
                }
            }

            sql.push_str(&format!(
                "WITH\n  {ctes}\nSELECT\n  {outer_columns}\nFROM _agg;\n",
                ctes = all_ctes.join(",\n  "),
                outer_columns = outer_exprs.join(",\n  "),
            ));
        } else {
            // Groups but no default_expression: use WITH for group CTEs only.
            sql.push_str(&format!(
                "WITH\n  {ctes}\nSELECT\n  {columns}\nFROM {from_clause}\n{group_by};\n",
                ctes = all_ctes.join(",\n  "),
                columns = agg_exprs.join(",\n  "),
                group_by = group_by_clause,
            ));
        }
    } else {
        // Simple single-pass approach (no groups, no default_expression).
        sql.push_str(&format!(
            "SELECT\n  {columns}\nFROM {id_view}\nGROUP BY _entity_id_resolved;\n",
            columns = agg_exprs.join(",\n  "),
        ));
    }

    Ok(sql)
}

fn default_val_to_sql(val: &serde_yaml::Value) -> String {
    match val {
        serde_yaml::Value::String(s) => format!("'{s}'"),
        serde_yaml::Value::Number(n) => format!("'{n}'"),
        serde_yaml::Value::Bool(b) => format!("'{b}'"),
        _ => "NULL".to_string(),
    }
}
