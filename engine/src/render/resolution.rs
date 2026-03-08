use anyhow::Result;
use indexmap::IndexMap;

use crate::model::{Strategy, Target};

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
    let view_name = format!("_resolved_{target_name}");
    let id_view = format!("_id_{target_name}");

    let mut sql = format!(
        "-- Resolution: {target_name}\n\
         CREATE OR REPLACE VIEW {view_name} AS\n"
    );

    let mut select_exprs: Vec<String> = Vec::new();
    select_exprs.push("_entity_id_resolved AS _entity_id".to_string());

    for (fname, fdef) in &target.fields {
        let strategy = fdef.strategy();
        let base_expr = match strategy {
            Strategy::Identity => {
                // Representative scalar — all values linked by this field are the
                // same (that's what linked them), so min() picks any of them.
                format!("min({fname})")
            }
            Strategy::Collect => {
                format!(
                    "array_agg(DISTINCT {fname}) FILTER (WHERE {fname} IS NOT NULL)"
                )
            }
            Strategy::Coalesce => {
                // Pick the first non-null value ordered by per-field priority,
                // falling back to mapping-level priority.
                format!(
                    "(array_agg({fname} ORDER BY COALESCE(_priority_{fname}, _priority, 999) ASC NULLS LAST) \
                     FILTER (WHERE {fname} IS NOT NULL))[1]"
                )
            }
            Strategy::LastModified => {
                // Pick the most recently modified non-null value.
                format!(
                    "(array_agg({fname} ORDER BY COALESCE(_ts_{fname}, _last_modified) DESC NULLS LAST) \
                     FILTER (WHERE {fname} IS NOT NULL))[1]"
                )
            }
            Strategy::Expression => {
                let default_expr = format!("max({fname})");
                let agg_expr = fdef.expression().unwrap_or(&default_expr);
                agg_expr.to_string()
            }
        };

        // Wrap with default/default_expression fallback
        let expr = if let Some(default_expr) = fdef.default_expression() {
            format!("COALESCE(({base_expr}), {default_expr}) AS {fname}")
        } else if let Some(default_val) = fdef.default_value() {
            let val_str = match default_val {
                serde_yaml::Value::String(s) => format!("'{s}'"),
                serde_yaml::Value::Number(n) => n.to_string(),
                serde_yaml::Value::Bool(b) => b.to_string(),
                _ => "NULL".to_string(),
            };
            format!("COALESCE(({base_expr}), {val_str}) AS {fname}")
        } else {
            format!("{base_expr} AS {fname}")
        };

        select_exprs.push(expr);
    }

    sql.push_str(&format!(
        "SELECT\n  {columns}\nFROM {id_view}\nGROUP BY _entity_id_resolved;\n",
        columns = select_exprs.join(",\n  "),
    ));

    Ok(sql)
}
