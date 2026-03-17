use anyhow::Result;
use std::collections::HashSet;

use crate::model::{Mapping, Target};
use crate::qi;

pub fn mixed_order_fields_for_target(mappings: &[Mapping], target_name: &str) -> HashSet<String> {
    let mut generated: HashSet<String> = HashSet::new();
    let mut external: HashSet<String> = HashSet::new();

    for m in mappings.iter().filter(|m| m.target.name() == target_name) {
        for fm in &m.fields {
            if let Some(ref tgt) = fm.target {
                if fm.order {
                    generated.insert(tgt.clone());
                } else if fm.source.is_some() || fm.source_path.is_some() {
                    external.insert(tgt.clone());
                }
            }
        }
    }

    generated
        .intersection(&external)
        .cloned()
        .collect::<HashSet<_>>()
}

/// Render canonical ordering layer for targets mixing generated and native order keys.
///
/// Produces: `CREATE OR REPLACE VIEW _ordered_{target_name} AS ...`
pub fn render_ordered_view(
    target_name: &str,
    target: &Target,
    mappings: &[&Mapping],
) -> Result<String> {
    let view_name = qi(&format!("_ordered_{target_name}"));
    let resolved_view = qi(&format!("_resolved_{target_name}"));

    let mut generated: HashSet<String> = HashSet::new();
    let mut external: HashSet<String> = HashSet::new();
    for m in mappings {
        for fm in &m.fields {
            if let Some(ref tgt) = fm.target {
                if fm.order {
                    generated.insert(tgt.clone());
                } else if fm.source.is_some() || fm.source_path.is_some() {
                    external.insert(tgt.clone());
                }
            }
        }
    }
    let mixed_fields: HashSet<String> = generated.intersection(&external).cloned().collect();

    if mixed_fields.is_empty() {
        let mut passthrough_cols: Vec<String> = vec!["r._entity_id".to_string()];
        for fname in target.fields.keys() {
            passthrough_cols.push(format!("r.{}", qi(fname)));
        }
        let sql = format!(
            "-- Ordered: {target_name} (passthrough)\n\
             CREATE OR REPLACE VIEW {view_name} AS\n\
             SELECT\n  {columns}\n\
             FROM {resolved_view} AS r;\n",
            columns = passthrough_cols.join(",\n  "),
        );
        return Ok(sql);
    }

    let mut mapping_owner_for_field: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for m in mappings {
        for fm in &m.fields {
            if let Some(ref tgt) = fm.target {
                mapping_owner_for_field
                    .entry(tgt.clone())
                    .or_insert_with(|| m.name.clone());
            }
        }
    }

    // Partition ordering by parent reference identity fields where available.
    let mut partition_fields: Vec<String> = Vec::new();
    for (fname, fdef) in &target.fields {
        if fdef.strategy() != crate::model::Strategy::Identity {
            continue;
        }
        let used_as_parent_ref = mappings.iter().any(|m| {
            m.fields
                .iter()
                .any(|fm| fm.target.as_deref() == Some(fname.as_str()) && fm.references.is_some())
        });
        if used_as_parent_ref {
            partition_fields.push(fname.clone());
        }
    }

    let partition_list = partition_fields
        .iter()
        .map(|f| format!("b.{}", qi(f)))
        .collect::<Vec<_>>()
        .join(", ");
    let partition_by = if partition_list.is_empty() {
        "".to_string()
    } else {
        format!("PARTITION BY {partition_list} ")
    };
    let partition_match = if partition_fields.is_empty() {
        "TRUE".to_string()
    } else {
        partition_fields
            .iter()
            .map(|f| {
                let q = qi(f);
                format!("a.{q} IS NOT DISTINCT FROM b.{q}")
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    };

    let mut base_cols: Vec<String> = vec!["r._entity_id".to_string()];
    for fname in target.fields.keys() {
        let qf = qi(fname);
        base_cols.push(format!("r.{qf}"));
    }

    for fname in &mixed_fields {
        let qf = qi(fname);
        base_cols.push(format!(
            "CASE WHEN (r.{qf})::text ~ '^[0-9]{{10}}$' THEN NULL ELSE (r.{qf})::text END AS {}",
            qi(&format!("__native_{fname}"))
        ));
        let owner = mapping_owner_for_field
            .get(fname)
            .cloned()
            .unwrap_or_else(|| "".to_string());
        let qi_id = qi(&format!("_id_{target_name}"));
        base_cols.push(format!(
            "(SELECT min((i.{qf})::bigint) \
             FROM {qi_id} i \
             WHERE i._entity_id_resolved = r._entity_id \
               AND i._mapping = '{owner}' \
               AND (i.{qf})::text ~ '^[0-9]{{10}}$') AS {}",
            qi(&format!("__gen_{fname}"))
        ));
    }

    let mut ranked_cols: Vec<String> = vec!["b.*".to_string()];
    for fname in &mixed_fields {
        let q_native = qi(&format!("__native_{fname}"));
        let q_native_rank = qi(&format!("__native_rank_{fname}"));
        ranked_cols.push(format!(
            "CASE \
             WHEN b.{q_native} IS NULL THEN NULL \
             ELSE row_number() OVER ({partition_by}ORDER BY b.{q_native}, b._entity_id) \
             END AS {q_native_rank}"
        ));
    }

    let mut final_cols: Vec<String> = vec!["b._entity_id".to_string()];
    for fname in target.fields.keys() {
        final_cols.push(format!("b.{}", qi(fname)));
    }
    for fname in &mixed_fields {
        let q_gen = qi(&format!("__gen_{fname}"));
        let q_native_rank = qi(&format!("__native_rank_{fname}"));
        let left_rank = format!(
            "(SELECT max(a.{q_native_rank}) FROM _ranked a WHERE {partition_match} AND a.{q_native_rank} IS NOT NULL AND a.{q_gen} <= b.{q_gen})"
        );
        let right_rank = format!(
            "(SELECT min(a.{q_native_rank}) FROM _ranked a WHERE {partition_match} AND a.{q_native_rank} IS NOT NULL AND a.{q_gen} > b.{q_gen})"
        );
        final_cols.push(format!(
            "CASE \
             WHEN b.{q_native_rank} IS NOT NULL THEN (b.{q_native_rank})::bigint * 1000000 \
             WHEN b.{q_gen} IS NULL THEN NULL \
             WHEN {left_rank} IS NOT NULL AND {right_rank} IS NOT NULL THEN ({left_rank})::bigint * 1000000 + (b.{q_gen})::bigint \
             WHEN {left_rank} IS NOT NULL THEN ({left_rank})::bigint * 1000000 + 500000 + (b.{q_gen})::bigint \
             WHEN {right_rank} IS NOT NULL THEN GREATEST((({right_rank})::bigint - 1) * 1000000 + (b.{q_gen})::bigint, 0) \
             ELSE (b.{q_gen})::bigint \
             END AS {}",
            qi(&format!("_order_rank_{fname}"))
        ));
    }

    let sql = format!(
        "-- Ordered: {target_name} (canonical order ranks)\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         WITH _base AS (\n  \
           SELECT\n    {base_cols}\n  \
           FROM {resolved_view} AS r\n\
         ),\n  \
         _ranked AS (\n  \
           SELECT\n    {ranked_cols}\n  \
           FROM _base AS b\n\
         )\n\
         SELECT\n  {final_cols}\n\
         FROM _ranked AS b;\n",
        base_cols = base_cols.join(",\n    "),
        ranked_cols = ranked_cols.join(",\n    "),
        final_cols = final_cols.join(",\n  "),
    );

    Ok(sql)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn mixed_order_field_is_canonicalized() {
        let doc = parser::parse_str(
            r#"
version: "1.0"
sources:
  a: { primary_key: id }
  b: { primary_key: id }
targets:
  t:
    fields:
      key: { strategy: identity }
      ord: { strategy: coalesce }
mappings:
  - name: a
    source: a
    target: t
    fields:
      - { source: id, target: key }
      - { target: ord, order: true }
  - name: b
    source: b
    target: t
    fields:
      - { source: id, target: key }
      - { source: ord_native, target: ord }
"#,
        )
        .unwrap();

        let target = doc.targets.get("t").unwrap();
        let mappings: Vec<&Mapping> = doc
            .mappings
            .iter()
            .filter(|m| m.target.name() == "t")
            .collect();

        let sql = render_ordered_view("t", target, &mappings).unwrap();
        assert!(sql.contains("_ordered_t"));
        assert!(sql.contains("_order_rank_ord"));
    }

    #[test]
    fn mixed_order_detector_finds_target_fields() {
        let doc = parser::parse_str(
            r#"
version: "1.0"
sources:
  a: { primary_key: id }
  b: { primary_key: id }
targets:
  t:
    fields:
      key: { strategy: identity }
      ord: { strategy: coalesce }
mappings:
  - name: a
    source: a
    target: t
    fields:
      - { source: id, target: key }
      - { target: ord, order: true }
  - name: b
    source: b
    target: t
    fields:
      - { source: id, target: key }
      - { source: ord_native, target: ord }
"#,
        )
        .unwrap();

        let fields = mixed_order_fields_for_target(&doc.mappings, "t");
        assert!(fields.contains("ord"));
    }
}
