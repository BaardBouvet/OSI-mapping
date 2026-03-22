use anyhow::Result;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

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
    mappings: &[&crate::model::Mapping],
    _all_targets: &IndexMap<String, Target>,
) -> Result<String> {
    let view_name = qi(&format!("_resolved_{target_name}"));
    let id_view = qi(&format!("_id_{target_name}"));

    let has_default_expr = target
        .fields
        .values()
        .any(|f| f.default_expression().is_some());

    // Detect mixed ordering fields: same target field receives both generated
    // ordinality (`order: true`) and external keys (`source:`/`source_path`).
    // For these fields, resolution should prefer external keys over generated
    // 10-digit ordinality strings when both are present.
    let mut generated_order_fields: HashSet<String> = HashSet::new();
    let mut external_order_fields: HashSet<String> = HashSet::new();
    for m in mappings {
        for fm in &m.fields {
            if let Some(ref tgt) = fm.target {
                if fm.order {
                    generated_order_fields.insert(tgt.clone());
                } else if fm.source.is_some() || fm.source_path.is_some() {
                    external_order_fields.insert(tgt.clone());
                }
            }
        }
    }
    let mixed_order_fields: HashSet<String> = generated_order_fields
        .intersection(&external_order_fields)
        .cloned()
        .collect();

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

    // ── Phase 2: echo-aware resolution for precision-loss ───────────
    // When a mapping declares `normalize` on a field with `last_modified`
    // strategy, a lower-precision source can produce a rounded echo of
    // the higher-precision value.  The echo CTE deduplicates these so
    // the higher-precision source wins within a normalized-value group.
    let mut normalize_map: HashMap<String, String> = HashMap::new();
    for m in mappings {
        for fm in &m.fields {
            if let (Some(ref tgt), Some(ref norm)) = (&fm.target, &fm.normalize) {
                normalize_map
                    .entry(tgt.clone())
                    .or_insert_with(|| norm.clone());
            }
        }
    }
    let echo_fields: Vec<String> = target
        .fields
        .iter()
        .filter(|(fname, fdef)| {
            fdef.strategy() == Strategy::LastModified
                && normalize_map.contains_key(*fname)
                && !grouped_fields.contains(fname.as_str())
        })
        .map(|(fname, _)| fname.clone())
        .collect();
    let has_echo = !echo_fields.is_empty();
    let src_view = if has_echo {
        qi("_echo")
    } else {
        id_view.clone()
    };

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
             FROM {src_view}\n    \
             WHERE {where_clause}\n    \
             ORDER BY _entity_id_resolved, {order_expr}\n  )",
            cte_alias = qi(&cte_alias),
            cols = cte_cols.join(", "),
            where_clause = null_checks.join(" OR "),
        ));
    }

    // Prepend echo CTE (reads from the raw identity view; all subsequent
    // CTEs and the main query read from _echo instead).
    if has_echo {
        let echo_rank_exprs: Vec<String> = echo_fields
            .iter()
            .map(|fname| {
                format!(
                    "ROW_NUMBER() OVER (\n        \
                     PARTITION BY _entity_id_resolved, {}\n        \
                     ORDER BY {} ASC, COALESCE({}, _last_modified) DESC NULLS LAST\n      \
                     ) AS {}",
                    qi(&format!("_normalize_{fname}")),
                    qi(&format!("_has_normalize_{fname}")),
                    qi(&format!("_ts_{fname}")),
                    qi(&format!("_echo_rank_{fname}"))
                )
            })
            .collect();
        group_ctes.insert(
            0,
            format!(
                "{} AS (\n    SELECT *,\n      {}\n    FROM {}\n  )",
                qi("_echo"),
                echo_rank_exprs.join(",\n      "),
                id_view,
            ),
        );
    }

    // ── Main aggregation expressions ────────────────────────────────
    let has_groups = !groups.is_empty();

    let mut agg_exprs: Vec<String> = Vec::new();
    if has_groups {
        agg_exprs.push(format!("{src_view}._entity_id_resolved AS _entity_id"));
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
                "(array_agg({qfname} ORDER BY {order_rank}, COALESCE({}, _priority, 999) ASC NULLS LAST) \
                 FILTER (WHERE {qfname} IS NOT NULL))[1]",
                qi(&format!("_priority_{fname}")),
                order_rank = if mixed_order_fields.contains(fname.as_str()) {
                    // Generated ordinality keys are exactly 10 digits.
                    // Prefer non-generated (external/native) keys in mixed mode.
                    format!(
                        "CASE WHEN ({qfname})::text ~ '^[0-9]{{10}}$' THEN 1 ELSE 0 END ASC"
                    )
                } else {
                    "0 ASC".to_string()
                }
            ),
            Strategy::LastModified => {
                if echo_fields.contains(fname) {
                    format!(
                        "(array_agg({qfname} ORDER BY COALESCE({}, _last_modified) DESC NULLS LAST) \
                         FILTER (WHERE {qfname} IS NOT NULL AND {} = 1))[1]",
                        qi(&format!("_ts_{fname}")),
                        qi(&format!("_echo_rank_{fname}"))
                    )
                } else {
                    format!(
                        "(array_agg({qfname} ORDER BY COALESCE({}, _last_modified) DESC NULLS LAST) \
                         FILTER (WHERE {qfname} IS NOT NULL))[1]",
                        qi(&format!("_ts_{fname}"))
                    )
                }
            }
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

    // ── Element set membership filtering ────────────────────────────
    // When the target declares `elements: coalesce` or `elements: last_modified`,
    // only elements from the winning mapping (per parent) survive.
    let element_where = if let Some(ref elem_strategy) = target.elements {
        // Find the parent reference field: an identity field with `references:`.
        let parent_ref_field = target
            .fields
            .iter()
            .find(|(_, fdef)| fdef.strategy() == Strategy::Identity && fdef.references().is_some())
            .map(|(fname, _)| fname.as_str());

        if let Some(parent_ref) = parent_ref_field {
            let q_parent_ref = qi(parent_ref);
            let order_expr = match elem_strategy {
                crate::model::ElementStrategy::Coalesce => {
                    "sub._priority ASC NULLS LAST".to_string()
                }
                crate::model::ElementStrategy::LastModified => {
                    "sub.last_touch DESC NULLS LAST".to_string()
                }
                crate::model::ElementStrategy::Collect => String::new(),
            };

            if !order_expr.is_empty() {
                let agg_col = match elem_strategy {
                    crate::model::ElementStrategy::LastModified => {
                        "MAX(_last_modified) AS last_touch, MIN(_priority) AS _priority".to_string()
                    }
                    _ => "MIN(_priority) AS _priority, NULL::text AS last_touch".to_string(),
                };

                group_ctes.push(format!(
                    "\"_element_winner\" AS (\n    \
                     SELECT DISTINCT ON (sub.{q_parent_ref})\n      \
                     sub.{q_parent_ref}, sub._mapping\n    \
                     FROM (\n      \
                     SELECT {q_parent_ref}, _mapping, {agg_col}\n      \
                     FROM {src_view}\n      \
                     WHERE {q_parent_ref} IS NOT NULL\n      \
                     GROUP BY {q_parent_ref}, _mapping\n    \
                     ) sub\n    \
                     ORDER BY sub.{q_parent_ref}, {order_expr}\n  )"
                ));

                Some(format!(
                    "EXISTS (\n    \
                     SELECT 1 FROM \"_element_winner\" _ew\n    \
                     WHERE _ew.{q_parent_ref} = {src_view}.{q_parent_ref}\n      \
                     AND _ew._mapping = {src_view}._mapping\n  )"
                ))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

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
                "LEFT JOIN {cte_alias} ON {cte_alias}._entity_id_resolved = {src_view}._entity_id_resolved"
            )
        })
        .collect();

    let needs_cte = has_default_expr || !group_ctes.is_empty() || element_where.is_some();

    let where_clause = element_where
        .as_ref()
        .map(|w| format!("\n  WHERE {w}"))
        .unwrap_or_default();

    if needs_cte {
        let mut all_ctes = group_ctes;

        // Inner aggregation query.
        let eid_ref = if has_groups {
            format!("{src_view}._entity_id_resolved")
        } else {
            "_entity_id_resolved".to_string()
        };
        let group_by_clause = if group_by_extra.is_empty() {
            format!("GROUP BY {eid_ref}")
        } else {
            format!("GROUP BY {eid_ref}, {}", group_by_extra.join(", "))
        };

        let from_clause = if joins.is_empty() {
            src_view
        } else {
            format!("{src_view}\n  {}", joins.join("\n  "))
        };

        if has_default_expr {
            // CTE approach: aggregate first, then apply default_expressions in outer SELECT.
            all_ctes.push(format!(
                "_agg AS (\n  SELECT\n    {agg_columns}\n  FROM {from_clause}{where_clause}\n  {group_by}\n)",
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
                "WITH\n  {ctes}\nSELECT\n  {columns}\nFROM {from_clause}{where_clause}\n{group_by};\n",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse(yaml: &str) -> crate::model::MappingDocument {
        parser::parse_str(yaml).expect("valid test YAML")
    }

    #[test]
    fn coalesce_strategy() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t: { fields: { name: { strategy: coalesce } } }
mappings:
  - name: s
    source: s
    target: t
    fields: [{ source: name, target: name }]
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let sql = render_resolution_view("t", target, &mappings, &doc.targets).unwrap();
        assert!(
            sql.contains("array_agg") && sql.contains("FILTER (WHERE"),
            "coalesce should use array_agg with FILTER"
        );
        assert!(
            sql.contains("_priority"),
            "coalesce should order by priority"
        );
    }

    #[test]
    fn last_modified_strategy() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t: { fields: { email: { strategy: last_modified } } }
mappings:
  - name: s
    source: s
    target: t
    fields: [{ source: email, target: email }]
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let sql = render_resolution_view("t", target, &mappings, &doc.targets).unwrap();
        assert!(
            sql.contains("_last_modified") || sql.contains("_ts_email"),
            "last_modified should reference timestamp column"
        );
        assert!(
            sql.contains("DESC NULLS LAST"),
            "last_modified should order DESC NULLS LAST"
        );
    }

    #[test]
    fn bool_or_strategy() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t: { fields: { active: { strategy: bool_or } } }
mappings:
  - name: s
    source: s
    target: t
    fields: [{ source: active, target: active }]
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let sql = render_resolution_view("t", target, &mappings, &doc.targets).unwrap();
        assert!(
            sql.contains("bool_or("),
            "bool_or strategy should produce bool_or() aggregate"
        );
    }

    #[test]
    fn group_distinct_on() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t:
    fields:
      addr_street: { strategy: coalesce, group: address }
      addr_city: { strategy: coalesce, group: address }
      name: { strategy: coalesce }
mappings:
  - name: s
    source: s
    target: t
    fields:
      - { source: street, target: addr_street }
      - { source: city, target: addr_city }
      - { source: name, target: name }
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let sql = render_resolution_view("t", target, &mappings, &doc.targets).unwrap();
        assert!(
            sql.contains("DISTINCT ON"),
            "grouped fields should use DISTINCT ON CTE"
        );
        assert!(
            sql.contains("_grp_address"),
            "should create CTE for address group"
        );
    }

    #[test]
    fn default_expression_fallback() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t:
    fields:
      name: { strategy: coalesce, default_expression: "'Unknown'" }
mappings:
  - name: s
    source: s
    target: t
    fields: [{ source: name, target: name }]
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let sql = render_resolution_view("t", target, &mappings, &doc.targets).unwrap();
        assert!(
            sql.contains("COALESCE") && sql.contains("'Unknown'"),
            "default_expression should produce COALESCE(..., default_expr)"
        );
    }
}
