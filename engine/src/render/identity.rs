use anyhow::Result;
use std::collections::HashMap;

use crate::model::{Mapping, Strategy, Target};

/// Render an identity / transitive closure view for a target entity.
///
/// Produces: `CREATE OR REPLACE VIEW _id_{target_name} AS ...`
///
/// The identity view:
/// 1. UNIONs ALL forward views for a target (`SELECT *` — all columns are
///    normalized by the forward renderer).
/// 2. Numbers each row.
/// 3. Finds connected components via recursive CTE using identity field matching.
/// 4. Outputs all forward columns plus `_entity_id`.
pub fn render_identity_view(
    target_name: &str,
    target: Option<&Target>,
    mappings: &[&Mapping],
) -> Result<String> {
    let view_name = format!("_id_{target_name}");

    let target = match target {
        Some(t) => t,
        None => {
            return Ok(format!(
                "-- Identity: {target_name} (external target, skipped)\n\n"
            ));
        }
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

    if ungrouped_identity.is_empty() && link_groups.is_empty() {
        // No identity fields — just pass-through forward views with a row-level entity_id
        let union_parts: Vec<String> = mappings
            .iter()
            .map(|m| format!("SELECT * FROM _fwd_{}", m.name))
            .collect();
        let base = union_parts.join("\n  UNION ALL\n  ");
        return Ok(format!(
            "-- Identity: {target_name} (no identity fields)\n\
             CREATE OR REPLACE VIEW {view_name} AS\n\
             WITH _id_base AS (\n  {base}\n)\n\
             SELECT *, ROW_NUMBER() OVER () AS _entity_id\nFROM _id_base;\n"
        ));
    }

    let mut sql = format!("-- Identity: {target_name}\n");

    // UNION ALL of all forward views (SELECT * — columns are normalized)
    let union_parts: Vec<String> = mappings
        .iter()
        .map(|m| format!("SELECT * FROM _fwd_{}", m.name))
        .collect();
    let base_query = union_parts.join("\n  UNION ALL\n  ");

    sql.push_str(&format!(
        "CREATE OR REPLACE VIEW {view_name} AS\n\
         WITH RECURSIVE _id_base AS (\n  {base_query}\n),\n"
    ));

    // Number each row for the transitive closure algorithm
    sql.push_str(
        "_id_numbered AS (\n  \
           SELECT *, ROW_NUMBER() OVER () AS _entity_id\n  \
           FROM _id_base\n),\n"
    );

    // Build join conditions for identity matching.
    // n and n2 are the aliases for the two _id_numbered rows being compared.
    let mut match_conditions: Vec<String> = Vec::new();

    for field in &ungrouped_identity {
        match_conditions.push(format!(
            "(n.{field} IS NOT NULL AND n.{field} = n2.{field})"
        ));
    }

    for (_group_name, fields) in &link_groups {
        let group_cond: Vec<String> = fields
            .iter()
            .map(|f| format!("(n.{f} IS NOT NULL AND n.{f} = n2.{f})"))
            .collect();
        match_conditions.push(format!("({})", group_cond.join(" AND ")));
    }

    let match_expr = if match_conditions.is_empty() {
        "FALSE".to_string()
    } else {
        match_conditions.join(" OR ")
    };

    // Recursive CTE: connected components via iterative minimum propagation.
    // Base: each row is its own component.
    // Recursive: for each pair sharing identity values, propagate the smaller component id.
    // PostgreSQL UNION (not UNION ALL) deduplicates → guaranteed termination.
    sql.push_str(&format!(
        "_id_closure AS (\n  \
           SELECT _entity_id, _entity_id AS _component\n  \
           FROM _id_numbered\n  \
           UNION\n  \
           SELECT n._entity_id, LEAST(c._component, n2._entity_id)\n  \
           FROM _id_closure c\n  \
           JOIN _id_numbered n ON c._entity_id = n._entity_id\n  \
           JOIN _id_numbered n2 ON n2._entity_id <> n._entity_id\n    \
             AND ({match_expr})\n  \
           WHERE LEAST(c._component, n2._entity_id) < c._component\n\
         ),\n"
    ));

    // Final: assign stable entity ID as minimum component
    sql.push_str(
        "_id_final AS (\n  \
           SELECT _entity_id, MIN(_component) AS _entity_id_resolved\n  \
           FROM _id_closure\n  \
           GROUP BY _entity_id\n\
         )\n"
    );

    // Join back to get all forward columns with resolved entity IDs
    sql.push_str(
        "SELECT n.*, f._entity_id_resolved\n\
         FROM _id_numbered n\n\
         JOIN _id_final f ON n._entity_id = f._entity_id;\n"
    );

    Ok(sql)
}
