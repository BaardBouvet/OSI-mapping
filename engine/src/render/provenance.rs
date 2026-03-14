use anyhow::Result;

/// Render the provenance view for a target.
///
/// Lists all source rows belonging to each cluster:
///   `_cluster_id, _mapping, _src_id`
pub fn render_provenance_view(target_name: &str) -> Result<String> {
    let view_name = format!("_provenance_{target_name}");
    let id_view = format!("_id_{target_name}");

    let sql = format!(
        "-- Provenance: {target_name}\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         SELECT\n  \
           _entity_id_resolved AS _cluster_id,\n  \
           _mapping,\n  \
           _src_id\n\
         FROM {id_view};\n"
    );

    Ok(sql)
}
