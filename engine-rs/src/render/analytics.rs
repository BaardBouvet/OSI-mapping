use anyhow::Result;

use crate::model::Target;
use crate::qi;

/// Render an analytics view that exposes the resolved golden record
/// in a clean, consumer-friendly shape for BI and analytics tools.
///
/// Produces: `CREATE OR REPLACE VIEW {target_name} AS ...`
///
/// Consumer-facing — named directly after the target (no underscore prefix).
/// Emits `_cluster_id` (aliased from `_entity_id`) and all resolved
/// business fields — no internal metadata columns.
pub fn render_analytics_view(target_name: &str, target: &Target) -> Result<String> {
    let resolved_view = qi(&format!("_resolved_{target_name}"));
    let qview_name = qi(target_name);

    let mut select_exprs: Vec<String> = Vec::new();
    select_exprs.push("_entity_id AS _cluster_id".to_string());

    for (fname, _fdef) in &target.fields {
        select_exprs.push(qi(fname));
    }

    let sql = format!(
        "-- {target_name}\n\
         CREATE OR REPLACE VIEW {qview_name} AS\n\
         SELECT\n  {columns}\nFROM {resolved_view};\n",
        columns = select_exprs.join(",\n  "),
    );

    Ok(sql)
}
