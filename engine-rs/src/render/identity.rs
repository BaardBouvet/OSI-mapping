use anyhow::Result;
use indexmap::IndexMap;
use std::collections::HashMap;

use crate::model::{Mapping, Source, Strategy, Target};
use crate::qi;

/// Render an identity / transitive closure view for a target entity.
///
/// Produces: `CREATE OR REPLACE VIEW _id_{target_name} AS ...`
///
/// `forward_names` lists the mapping names that have forward views (`_fwd_{name}`).
/// The identity view references these views via `SELECT * FROM _fwd_{name}`.
///
/// The identity view:
/// 1. UNIONs ALL forward views into `_id_base`.
/// 2. Assigns deterministic `_entity_id` via `md5(_mapping || ':' || _src_id)`.
/// 3. Finds connected components via recursive CTE using identity field matching
///    and (optionally) pairwise link edges from `links` declarations.
/// 4. Outputs all forward columns plus `_entity_id` and `_entity_id_resolved`.
pub fn render_identity_view(
    target_name: &str,
    target: Option<&Target>,
    mappings: &[&Mapping],
    all_mappings: &[Mapping],
    sources: &IndexMap<String, Source>,
    forward_names: &[String], // mapping names with forward views
) -> Result<String> {
    let view_name = qi(&format!("_id_{target_name}"));

    let Some(target) = target else {
        return Ok(format!(
            "-- Identity: {target_name} (external target, skipped)\n\n"
        ));
    };

    // Collect identity fields, grouped by link_group.
    let mut ungrouped_identity: Vec<String> = Vec::new();
    let mut link_groups: HashMap<String, Vec<String>> = HashMap::new();

    for (fname, fdef) in &target.fields {
        if fdef.strategy() == Strategy::Identity {
            if let Some(lg) = fdef.link_group() {
                link_groups
                    .entry(lg.to_string())
                    .or_default()
                    .push(fname.clone());
            } else {
                ungrouped_identity.push(fname.clone());
            }
        }
    }

    // Check if any link mappings target this entity.
    let has_link_mappings = all_mappings
        .iter()
        .any(|m| m.target.name() == target_name && m.has_links());
    let has_cluster_id = mappings
        .iter()
        .any(|m| m.cluster_members.is_some() || m.cluster_field.is_some());

    if ungrouped_identity.is_empty()
        && link_groups.is_empty()
        && !has_link_mappings
        && !has_cluster_id
    {
        // No identity fields, no links, no cluster_id — pass-through with a row-level entity_id
        let union_parts: Vec<String> = forward_names
            .iter()
            .map(|name| format!("SELECT * FROM {}", qi(&format!("_fwd_{name}"))))
            .collect();
        let base = union_parts.join("\n  UNION ALL\n  ");
        let eid = "md5(_mapping || ':' || _src_id)";
        return Ok(format!(
            "-- Identity: {target_name} (no identity fields)\n\
             CREATE OR REPLACE VIEW {view_name} AS\n\
             WITH _id_base AS (\n  {base}\n)\n\
             SELECT *, {eid} AS _entity_id,\n       \
             {eid} AS _entity_id_resolved\n\
             FROM _id_base;\n",
        ));
    }

    let mut sql = format!("-- Identity: {target_name}\n");

    // Reference forward views and UNION ALL into _id_base.
    let union_parts: Vec<String> = forward_names
        .iter()
        .map(|name| format!("SELECT * FROM {}", qi(&format!("_fwd_{name}"))))
        .collect();
    let base_query = union_parts.join("\n  UNION ALL\n  ");

    sql.push_str(&format!(
        "CREATE OR REPLACE VIEW {view_name} AS\n\
         WITH RECURSIVE _id_base AS (\n  {base_query}\n),\n",
    ));

    // Deterministic row identity — md5 of (mapping, src_id, identity fields).
    // Including identity field values ensures nested array items with different
    // identities get distinct entity IDs (they share _src_id but differ in identity).
    let all_id_fields: Vec<&String> = ungrouped_identity
        .iter()
        .chain(link_groups.values().flat_map(|v| v.iter()))
        .collect();
    let eid = if all_id_fields.is_empty() {
        "md5(_mapping || ':' || _src_id)".to_string()
    } else {
        let id_parts: Vec<String> = all_id_fields
            .iter()
            .map(|f| format!("COALESCE({}::text, '')", qi(f)))
            .collect();
        format!(
            "md5(_mapping || ':' || _src_id || ':' || {})",
            id_parts.join(" || ':' || ")
        )
    };
    sql.push_str(&format!(
        "_id_numbered AS (\n  \
           SELECT *, {eid} AS _entity_id\n  \
           FROM _id_base\n),\n"
    ));

    // Build join conditions for identity matching.
    // n and n2 are the aliases for the two _id_numbered rows being compared.
    let mut match_conditions: Vec<String> = Vec::new();

    for field in &ungrouped_identity {
        let qf = qi(field);
        match_conditions.push(format!("(n.{qf} IS NOT NULL AND n.{qf} = n2.{qf})"));
    }

    for fields in link_groups.values() {
        let group_cond: Vec<String> = fields
            .iter()
            .map(|f| {
                let qf = qi(f);
                format!("(n.{qf} IS NOT NULL AND n.{qf} = n2.{qf})")
            })
            .collect();
        match_conditions.push(format!("({})", group_cond.join(" AND ")));
    }

    // cluster_members / cluster_field: rows sharing a non-NULL _cluster_id are linked.
    if has_cluster_id {
        match_conditions
            .push("(n._cluster_id IS NOT NULL AND n._cluster_id = n2._cluster_id)".to_string());
    }

    let match_expr = if match_conditions.is_empty() {
        "FALSE".to_string()
    } else {
        match_conditions.join(" OR ")
    };

    // Generate pairwise link edges from `links` declarations (batch-safe path).
    // Each link mapping targeting this target produces edges between pairs of
    // referenced source rows.
    let mut link_edge_parts: Vec<String> = Vec::new();

    for link_mapping in all_mappings
        .iter()
        .filter(|m| m.target.name() == target_name && m.has_links())
    {
        let link_source = qi(sources
            .get(&link_mapping.source.dataset)
            .map(|s| s.table_name(&link_mapping.source.dataset))
            .unwrap_or(&link_mapping.source.dataset));

        // For each pair of link references, generate a JOIN that produces edges
        let links = &link_mapping.links;
        for i in 0..links.len() {
            for j in (i + 1)..links.len() {
                let link_a = &links[i];
                let link_b = &links[j];

                // Find the referenced mappings' names
                let ref_mapping_a = &link_a.references;
                let ref_mapping_b = &link_b.references;

                // Find the referenced source PKs
                let ref_a_source = all_mappings
                    .iter()
                    .find(|m| m.name == *ref_mapping_a)
                    .map(|m| &m.source.dataset);
                let ref_b_source = all_mappings
                    .iter()
                    .find(|m| m.name == *ref_mapping_b)
                    .map(|m| &m.source.dataset);

                let pk_a = ref_a_source
                    .and_then(|ds| sources.get(ds))
                    .map(|s| &s.primary_key);
                let pk_b = ref_b_source
                    .and_then(|ds| sources.get(ds))
                    .map(|s| &s.primary_key);

                if let (Some(pk_a), Some(pk_b)) = (pk_a, pk_b) {
                    let pairs_a = link_a.field.column_pairs(pk_a);
                    let pairs_b = link_b.field.column_pairs(pk_b);

                    // Build the src_id expression for each side
                    let a_src_id = if pairs_a.len() == 1 {
                        format!("lt.{}::text", qi(&pairs_a[0].0))
                    } else {
                        let mut sorted: Vec<(&str, &str)> = pairs_a
                            .iter()
                            .map(|(l, _p)| (l.as_str(), l.as_str()))
                            .collect();
                        sorted.sort_by_key(|(_, pk)| pk.to_string());
                        let parts: Vec<String> = sorted
                            .iter()
                            .map(|(link_col, pk_col)| format!("'{}', lt.{}", pk_col, qi(link_col)))
                            .collect();
                        format!("jsonb_build_object({})::text", parts.join(", "))
                    };
                    let b_src_id = if pairs_b.len() == 1 {
                        format!("lt.{}::text", qi(&pairs_b[0].0))
                    } else {
                        let mut sorted: Vec<(&str, &str)> = pairs_b
                            .iter()
                            .map(|(l, _p)| (l.as_str(), l.as_str()))
                            .collect();
                        sorted.sort_by_key(|(_, pk)| pk.to_string());
                        let parts: Vec<String> = sorted
                            .iter()
                            .map(|(link_col, pk_col)| format!("'{}', lt.{}", pk_col, qi(link_col)))
                            .collect();
                        format!("jsonb_build_object({})::text", parts.join(", "))
                    };

                    // NULL checks for link columns
                    let null_checks_a: Vec<String> = pairs_a
                        .iter()
                        .map(|(l, _)| format!("lt.{} IS NOT NULL", qi(l)))
                        .collect();
                    let null_checks_b: Vec<String> = pairs_b
                        .iter()
                        .map(|(l, _)| format!("lt.{} IS NOT NULL", qi(l)))
                        .collect();
                    let null_checks = [null_checks_a, null_checks_b].concat().join(" AND ");

                    link_edge_parts.push(format!(
                        "SELECT\n    \
                           a._entity_id AS entity_a,\n    \
                           b._entity_id AS entity_b\n  \
                         FROM {link_source} lt\n  \
                         JOIN _id_numbered a\n    \
                           ON a._mapping = '{ref_mapping_a}' AND a._src_id = {a_src_id}\n  \
                         JOIN _id_numbered b\n    \
                           ON b._mapping = '{ref_mapping_b}' AND b._src_id = {b_src_id}\n  \
                         WHERE {null_checks}"
                    ));
                }
            }
        }
    }

    let has_link_edges = !link_edge_parts.is_empty();

    if has_link_edges {
        sql.push_str(&format!(
            "_link_edges AS (\n  {}\n),\n",
            link_edge_parts.join("\n  UNION ALL\n  ")
        ));
    }

    // Recursive CTE: connected components via iterative minimum propagation.
    // Base: each row is its own component + link edges (if any).
    // Recursive: for each pair sharing identity values, propagate the smaller component id.
    // PostgreSQL UNION (not UNION ALL) deduplicates → guaranteed termination.
    if has_link_edges {
        sql.push_str(&format!(
            "_id_closure AS (\n  \
               SELECT _entity_id, _entity_id AS _component\n  \
               FROM _id_numbered\n  \
               UNION\n  \
               SELECT entity_a, entity_b FROM _link_edges\n  \
               UNION\n  \
               SELECT entity_b, entity_a FROM _link_edges\n  \
               UNION\n  \
               SELECT n._entity_id, c._component\n  \
               FROM _id_closure c\n  \
               JOIN _id_numbered n2 ON c._entity_id = n2._entity_id\n  \
               JOIN _id_numbered n ON n._entity_id <> n2._entity_id\n    \
                 AND ({match_expr})\n  \
               WHERE c._component < n._entity_id\n\
             ),\n"
        ));
    } else {
        sql.push_str(&format!(
            "_id_closure AS (\n  \
               SELECT _entity_id, _entity_id AS _component\n  \
               FROM _id_numbered\n  \
               UNION\n  \
               SELECT n._entity_id, c._component\n  \
               FROM _id_closure c\n  \
               JOIN _id_numbered n2 ON c._entity_id = n2._entity_id\n  \
               JOIN _id_numbered n ON n._entity_id <> n2._entity_id\n    \
                 AND ({match_expr})\n  \
               WHERE c._component < n._entity_id\n\
             ),\n"
        ));
    }

    // Final: assign stable entity ID as minimum component
    sql.push_str(
        "_id_final AS (\n  \
           SELECT _entity_id, MIN(_component) AS _entity_id_resolved\n  \
           FROM _id_closure\n  \
           GROUP BY _entity_id\n\
         )\n",
    );

    // Join back to get all forward columns with resolved entity IDs
    sql.push_str(
        "SELECT n.*, f._entity_id_resolved\n\
         FROM _id_numbered n\n\
         JOIN _id_final f ON n._entity_id = f._entity_id;\n",
    );

    Ok(sql)
}
