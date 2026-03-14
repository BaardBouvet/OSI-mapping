use anyhow::Result;

use crate::model::Target;

/// Render an analytics view that exposes the resolved golden record
/// in a clean, consumer-friendly shape for BI and analytics tools.
///
/// Produces: `CREATE OR REPLACE VIEW _analytics_{target_name} AS ...`
///
/// Emits `_cluster_id` (aliased from `_entity_id`) and all resolved
/// business fields — no internal metadata columns.
pub fn render_analytics_view(
    target_name: &str,
    target: &Target,
) -> Result<String> {
    let view_name = format!("_analytics_{target_name}");
    let resolved_view = format!("_resolved_{target_name}");

    let mut select_exprs: Vec<String> = Vec::new();
    select_exprs.push("_entity_id AS _cluster_id".to_string());

    for (fname, _fdef) in &target.fields {
        select_exprs.push(fname.clone());
    }

    let sql = format!(
        "-- Analytics: {target_name}\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         SELECT\n  {columns}\nFROM {resolved_view};\n",
        columns = select_exprs.join(",\n  "),
    );

    Ok(sql)
}
