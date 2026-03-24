use anyhow::Result;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use crate::model::{Mapping, Source, Strategy, Target};
use crate::qi;

/// Build the explicit column list for SELECT from forward views.
///
/// This avoids `SELECT *` which would pick up tool-added columns
/// (e.g. `__pgt_row_id` from pg_trickle stream tables).
fn forward_column_list(target: &Target, mappings: &[&Mapping]) -> String {
    let mut cols = vec![
        "_src_id".to_string(),
        "_mapping".to_string(),
        "_cluster_id".to_string(),
        "_priority".to_string(),
        "_last_modified".to_string(),
    ];

    // Determine which fields have normalize declared across any mapping.
    let normalize_fields: HashSet<String> = mappings
        .iter()
        .flat_map(|m| m.fields.iter())
        .filter(|fm| fm.normalize.is_some())
        .filter_map(|fm| fm.target.clone())
        .collect();

    for (fname, _) in &target.fields {
        cols.push(qi(fname));
        cols.push(qi(&format!("_priority_{fname}")));
        cols.push(qi(&format!("_ts_{fname}")));
        if normalize_fields.contains(fname) {
            cols.push(qi(&format!("_normalize_{fname}")));
            cols.push(qi(&format!("_has_normalize_{fname}")));
        }
    }
    cols.push("_base".to_string());
    cols.join(", ")
}

/// Render an identity / transitive closure view for a target entity.
///
/// Produces: `CREATE OR REPLACE VIEW _id_{target_name} AS ...`
///
/// `forward_names` lists the mapping names that have forward views (`_fwd_{name}`).
/// The identity view references these views with explicit column lists to avoid
/// picking up tool-added columns (e.g. `__pgt_row_id` from pg_trickle).
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
        let col_list = forward_column_list(target, mappings);
        let union_parts: Vec<String> = forward_names
            .iter()
            .map(|name| format!("SELECT {col_list} FROM {}", qi(&format!("_fwd_{name}"))))
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
    // Use explicit column list to avoid picking up tool-added columns
    // (e.g. __pgt_row_id from pg_trickle stream tables).
    let col_list = forward_column_list(target, mappings);
    let union_parts: Vec<String> = forward_names
        .iter()
        .map(|name| format!("SELECT {col_list} FROM {}", qi(&format!("_fwd_{name}"))))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse(yaml: &str) -> crate::model::MappingDocument {
        parser::parse_str(yaml).expect("valid test YAML")
    }

    #[test]
    fn recursive_cte_structure() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  a: { primary_key: id }
  b: { primary_key: id }
targets:
  t: { fields: { email: { strategy: identity }, name: { strategy: coalesce } } }
mappings:
  - name: a
    source: a
    target: t
    fields:
      - { source: email, target: email }
      - { source: name, target: name }
  - name: b
    source: b
    target: t
    fields:
      - { source: email, target: email }
      - { source: name, target: name }
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let fwd_names: Vec<String> = vec!["a".into(), "b".into()];
        let sql = render_identity_view(
            "t",
            Some(target),
            &mappings,
            &doc.mappings,
            &doc.sources,
            &fwd_names,
        )
        .unwrap();
        assert!(
            sql.contains("WITH RECURSIVE"),
            "should use recursive CTE for identity resolution"
        );
        assert!(sql.contains("_id_closure"), "should define _id_closure CTE");
        assert!(sql.contains("_component"), "should track component IDs");
    }

    #[test]
    fn union_all_base() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  a: { primary_key: id }
  b: { primary_key: id }
targets:
  t: { fields: { email: { strategy: identity } } }
mappings:
  - name: a
    source: a
    target: t
    fields: [{ source: email, target: email }]
  - name: b
    source: b
    target: t
    fields: [{ source: email, target: email }]
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let fwd_names: Vec<String> = vec!["a".into(), "b".into()];
        let sql = render_identity_view(
            "t",
            Some(target),
            &mappings,
            &doc.mappings,
            &doc.sources,
            &fwd_names,
        )
        .unwrap();
        assert!(
            sql.contains("_fwd_a"),
            "should reference forward view for mapping a"
        );
        assert!(
            sql.contains("_fwd_b"),
            "should reference forward view for mapping b"
        );
        assert!(sql.contains("UNION ALL"), "should UNION ALL forward views");
    }

    #[test]
    fn link_group_edges() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t:
    fields:
      ssn: { strategy: identity, link_group: national_id }
      passport: { strategy: identity, link_group: national_id }
      name: { strategy: coalesce }
mappings:
  - name: s
    source: s
    target: t
    fields:
      - { source: ssn, target: ssn }
      - { source: passport, target: passport }
      - { source: name, target: name }
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let fwd_names: Vec<String> = vec!["s".into()];
        let sql = render_identity_view(
            "t",
            Some(target),
            &mappings,
            &doc.mappings,
            &doc.sources,
            &fwd_names,
        )
        .unwrap();
        // link_group fields should be combined with AND in the match condition
        assert!(
            sql.contains("\"ssn\""),
            "should reference ssn identity field"
        );
        assert!(
            sql.contains("\"passport\""),
            "should reference passport identity field"
        );
        assert!(
            sql.contains("WITH RECURSIVE"),
            "link_group should trigger recursive CTE"
        );
    }

    #[test]
    fn single_mapping_no_identity_no_recursion() {
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
        let fwd_names: Vec<String> = vec!["s".into()];
        let sql = render_identity_view(
            "t",
            Some(target),
            &mappings,
            &doc.mappings,
            &doc.sources,
            &fwd_names,
        )
        .unwrap();
        // No identity fields → pass-through path (no recursion needed)
        assert!(
            !sql.contains("WITH RECURSIVE"),
            "no identity fields should skip recursion"
        );
        assert!(sql.contains("_entity_id"), "should still assign entity IDs");
        assert!(
            sql.contains("_entity_id_resolved"),
            "should still assign resolved entity IDs"
        );
    }
}
