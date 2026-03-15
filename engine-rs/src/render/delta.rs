use anyhow::Result;

use crate::model::{Mapping, Source};
use crate::qi;

/// Build the CASE expression that classifies a row from a single mapping's
/// reverse view as insert / delete / noop / update.
fn action_case(mapping: &Mapping, pk_columns: &std::collections::HashSet<&str>) -> String {
    // Delete conditions from reverse_required + reverse_filter.
    let mut delete_conditions: Vec<String> = Vec::new();
    for fm in &mapping.fields {
        if fm.reverse_required {
            if let Some(ref src) = fm.source {
                delete_conditions.push(format!("{} IS NULL", qi(src)));
            }
        }
    }
    if let Some(ref rf) = mapping.reverse_filter {
        delete_conditions.push(format!("({rf}) IS NOT TRUE"));
    }

    // Noop detection: compare each reverse-mapped source field against _base.
    let noop_parts: Vec<String> = mapping
        .fields
        .iter()
        .filter(|fm| fm.is_reverse() && fm.source.is_some())
        .filter_map(|fm| {
            let src = fm.source.as_ref()?;
            if pk_columns.contains(src.as_str()) {
                return None;
            }
            Some(format!("_base->>'{src}' IS NOT DISTINCT FROM {}::text", qi(src)))
        })
        .collect();

    let mut branches = Vec::new();

    if mapping.embedded {
        // Embedded mappings extract data from a shared source table.
        // They should not produce insert rows (can't insert partial records).
        branches.push("WHEN _src_id IS NULL THEN NULL".to_string());
    } else {
        // Filter inserts: if delete conditions exist, an entity with _src_id IS NULL
        // that also matches the delete conditions should be excluded (not inserted).
        if !delete_conditions.is_empty() {
            branches.push(format!(
                "WHEN _src_id IS NULL AND ({}) THEN NULL",
                delete_conditions.join(" OR ")
            ));
        }
        branches.push("WHEN _src_id IS NULL THEN 'insert'".to_string());
    }
    if !delete_conditions.is_empty() {
        branches.push(format!(
            "WHEN {} THEN 'delete'",
            delete_conditions.join(" OR ")
        ));
    }
    if !noop_parts.is_empty() {
        branches.push(format!("WHEN {} THEN 'noop'", noop_parts.join(" AND ")));
    }
    // When all reverse-mapped source fields are PK columns, there's nothing
    // to compare — existing rows are always noops.
    let has_non_pk_reverse = mapping.fields.iter().any(|fm| {
        fm.is_reverse()
            && fm.source.as_deref().map_or(false, |s| !pk_columns.contains(s))
    });
    if !has_non_pk_reverse {
        branches.push("ELSE 'noop'".to_string());
    } else {
        branches.push("ELSE 'update'".to_string());
    }

    format!("CASE\n      {}\n    END", branches.join("\n      "))
}

/// Render a delta view that classifies rows as insert/update/delete/noop.
///
/// Produces: `CREATE OR REPLACE VIEW _delta_{source} AS ...`
///
/// When a source has multiple mappings (routing), each mapping's reverse view
/// computes its own `_action` and the results are combined via UNION ALL.
pub fn render_delta_view(
    source_name: &str,
    mappings: &[&Mapping],
    source_meta: Option<&Source>,
) -> Result<String> {
    let view_name = qi(&format!("_delta_{source_name}"));

    let pk_columns: std::collections::HashSet<&str> = source_meta
        .map(|src| src.primary_key.columns().into_iter().collect())
        .unwrap_or_default();

    // Collect all reverse-mapped source fields across all mappings (union).
    let mut reverse_fields: Vec<String> = Vec::new();
    for mapping in mappings {
        for fm in &mapping.fields {
            if fm.is_reverse() {
                if let Some(ref src) = fm.source {
                    if !pk_columns.contains(src.as_str()) && !reverse_fields.contains(src) {
                        reverse_fields.push(src.clone());
                    }
                }
            }
        }
    }

    // Output columns (after _action).
    let mut out_cols: Vec<String> = vec!["_cluster_id".to_string()];
    if let Some(src) = source_meta {
        for col in src.primary_key.columns() {
            out_cols.push(col.to_string());
        }
    }
    out_cols.extend(reverse_fields.iter().cloned());
    out_cols.push("_base".to_string());

    if mappings.len() == 1 {
        // Single mapping: simple SELECT with CASE from the one reverse view.
        let action_expr = action_case(mappings[0], &pk_columns);
        let mut cols: Vec<String> = vec![format!("{action_expr} AS _action")];
        cols.extend(out_cols.iter().map(|c| qi(c)));

        let rev_view = qi(&format!("_rev_{}", mappings[0].name));
        let sql = format!(
            "-- Delta: {source_name} (change detection)\n\
             CREATE OR REPLACE VIEW {view_name} AS\n\
             SELECT\n  {columns}\n\
             FROM {rev_view};\n",
            columns = cols.join(",\n  "),
        );
        return Ok(sql);
    }

    // Multiple mappings: each computes _action inside its SELECT, then UNION ALL.
    let selects: Vec<String> = mappings
        .iter()
        .map(|m| {
            let case_expr = action_case(m, &pk_columns);

            // This mapping's reverse source fields.
            let m_fields: std::collections::HashSet<&str> = m
                .fields
                .iter()
                .filter(|fm| fm.is_reverse() && fm.source.is_some())
                .filter_map(|fm| fm.source.as_deref())
                .collect();

            let mut projected: Vec<String> = vec![format!("{case_expr} AS _action")];
            for col in &out_cols {
                if col == "_cluster_id" || col == "_base"
                    || pk_columns.contains(col.as_str())
                {
                    projected.push(qi(col));
                } else if m_fields.contains(col.as_str()) {
                    projected.push(qi(col));
                } else {
                    projected.push(format!("NULL::text AS {}", qi(col)));
                }
            }
            format!("SELECT {} FROM {}", projected.join(", "), qi(&format!("_rev_{}", m.name)))
        })
        .collect();

    let sql = format!(
        "-- Delta: {source_name} (change detection)\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         {selects};\n",
        selects = selects.join("\nUNION ALL\n"),
    );

    Ok(sql)
}
