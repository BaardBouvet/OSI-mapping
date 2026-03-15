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
pub fn render_resolution_view(
    target_name: &str,
    target: &Target,
    _mappings: &[&crate::model::Mapping],
    _all_targets: &IndexMap<String, Target>,
) -> Result<String> {
    let view_name = qi(&format!("_resolved_{target_name}"));
    let id_view = qi(&format!("_id_{target_name}"));

    let has_default_expr = target.fields.values().any(|f| f.default_expression().is_some());

    let mut sql = format!(
        "-- Resolution: {target_name}\n\
         CREATE OR REPLACE VIEW {view_name} AS\n"
    );

    // Build per-field aggregation expressions (inner query).
    let mut agg_exprs: Vec<String> = Vec::new();
    agg_exprs.push("_entity_id_resolved AS _entity_id".to_string());

    // Track which fields need outer-level default_expression wrapping.
    let mut outer_defaults: Vec<(String, String)> = Vec::new(); // (fname, default_expr)

    for (fname, fdef) in &target.fields {
        let qfname = qi(fname);
        let strategy = fdef.strategy();
        let base_expr = match strategy {
            Strategy::Identity => format!("min({qfname})"),
            Strategy::Collect => format!(
                "array_agg(DISTINCT {qfname}) FILTER (WHERE {qfname} IS NOT NULL)"
            ),
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
                let default_e = format!("max({})", qfname);
                let agg_expr = fdef.expression().unwrap_or(&default_e);
                agg_expr.to_string()
            }
        };

        if has_default_expr {
            // When using CTE approach: inner query does pure aggregation;
            // defaults applied in outer query.
            if let Some(de) = fdef.default_expression() {
                outer_defaults.push((fname.clone(), de.to_string()));
                agg_exprs.push(format!("{base_expr} AS {qfname}"));
            } else if let Some(default_val) = fdef.default_value() {
                let val_str = match default_val {
                    serde_yaml::Value::String(s) => format!("'{s}'"),
                    serde_yaml::Value::Number(n) => format!("'{n}'"),
                    serde_yaml::Value::Bool(b) => format!("'{b}'"),
                    _ => "NULL".to_string(),
                };
                // Literal defaults are safe inline
                agg_exprs.push(format!("COALESCE(({base_expr}), {val_str}) AS {qfname}"));
            } else {
                agg_exprs.push(format!("{base_expr} AS {qfname}"));
            }
        } else {
            // Simple single-pass: apply defaults inline (no default_expression exists)
            let expr = if let Some(default_val) = fdef.default_value() {
                let val_str = match default_val {
                    serde_yaml::Value::String(s) => format!("'{s}'"),
                    serde_yaml::Value::Number(n) => format!("'{n}'"),
                    serde_yaml::Value::Bool(b) => format!("'{b}'"),
                    _ => "NULL".to_string(),
                };
                format!("COALESCE(({base_expr}), {val_str}) AS {qfname}")
            } else {
                format!("{base_expr} AS {qfname}")
            };
            agg_exprs.push(expr);
        }
    }

    if has_default_expr {
        // CTE approach: aggregate first, then apply default_expressions in outer SELECT.
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
            "WITH _agg AS (\n  \
             SELECT\n    {agg_columns}\n  \
             FROM {id_view}\n  \
             GROUP BY _entity_id_resolved\n)\n\
             SELECT\n  {outer_columns}\nFROM _agg;\n",
            agg_columns = agg_exprs.join(",\n    "),
            outer_columns = outer_exprs.join(",\n  "),
        ));
    } else {
        // Simple single-pass approach.
        sql.push_str(&format!(
            "SELECT\n  {columns}\nFROM {id_view}\nGROUP BY _entity_id_resolved;\n",
            columns = agg_exprs.join(",\n  "),
        ));
    }

    Ok(sql)
}
