use anyhow::Result;

use crate::model::{Mapping, Source};
use crate::qi;

/// Info about a nested-path child mapping for delta re-assembly.
struct NestedChild<'a> {
    mapping: &'a Mapping,
    /// The source.path value (JSONB column name in the parent source, e.g. "lines").
    path: &'a str,
    /// Source field names that are array element data (non-PK, non-parent-alias).
    item_fields: Vec<String>,
    /// The parent_field alias that links back to the parent PK (for grouping).
    parent_fk_field: Option<String>,
}

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
///
/// When a source has nested-path child mappings (source.path), the delta
/// aggregates child reverse rows into JSONB arrays and joins them onto the
/// parent's reverse view, producing a single row per parent with nested arrays.
pub fn render_delta_view(
    source_name: &str,
    mappings: &[&Mapping],
    source_meta: Option<&Source>,
) -> Result<String> {
    let view_name = qi(&format!("_delta_{source_name}"));

    let pk_columns: std::collections::HashSet<&str> = source_meta
        .map(|src| src.primary_key.columns().into_iter().collect())
        .unwrap_or_default();

    // Separate parent (no path) from nested-path child mappings.
    let parent_mappings: Vec<&&Mapping> = mappings
        .iter()
        .filter(|m| m.source.path.is_none())
        .collect();
    let nested_mappings: Vec<NestedChild> = mappings
        .iter()
        .filter_map(|m| {
            let path = m.source.path.as_deref()?;
            // Only single-segment paths (direct children of the parent table).
            // Multi-segment paths (e.g., "children.grandchildren") are deeper
            // levels that need recursive re-assembly — not yet supported.
            if path.contains('.') {
                return None;
            }
            let parent_aliases: std::collections::HashSet<String> = m
                .source
                .parent_fields
                .keys()
                .cloned()
                .collect();
            let item_fields: Vec<String> = m
                .fields
                .iter()
                .filter(|fm| fm.is_reverse() && fm.source.is_some())
                .filter_map(|fm| fm.source.clone())
                .filter(|src| !pk_columns.contains(src.as_str()) && !parent_aliases.contains(src))
                .collect();
            // Find the parent_field alias that maps to a parent PK column.
            let parent_fk_field = m.source.parent_fields.keys().next().cloned();
            Some(NestedChild {
                mapping: m,
                path,
                item_fields,
                parent_fk_field,
            })
        })
        .collect();

    // If there are nested children, group them by parent mapping and aggregate.
    if !parent_mappings.is_empty() && !nested_mappings.is_empty() {
        return render_delta_with_nested(
            source_name,
            &view_name,
            &parent_mappings,
            &nested_mappings,
            &pk_columns,
            source_meta,
        );
    }

    // ── Standard path (no nested arrays) ──────────────────────────────

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

/// Render a delta view that aggregates nested-path child mappings into JSONB
/// arrays and joins them onto the parent reverse view.
fn render_delta_with_nested(
    source_name: &str,
    view_name: &str,
    parent_mappings: &[&&Mapping],
    nested_children: &[NestedChild],
    pk_columns: &std::collections::HashSet<&str>,
    _source_meta: Option<&Source>,
) -> Result<String> {
    // For now, use the first parent mapping as the base.
    // Future: support multiple parents via UNION ALL, each with their nested children.
    let parent = parent_mappings[0];
    let parent_rev = qi(&format!("_rev_{}", parent.name));

    // Collect parent's reverse source fields (non-PK).
    let parent_fields: Vec<String> = parent
        .fields
        .iter()
        .filter(|fm| fm.is_reverse() && fm.source.is_some())
        .filter_map(|fm| fm.source.clone())
        .filter(|src| !pk_columns.contains(src.as_str()))
        .collect();

    // Build CTE for each nested child: aggregate rows into JSONB array.
    let mut ctes: Vec<String> = Vec::new();
    let mut child_join_aliases: Vec<(String, String, Option<String>)> = Vec::new(); // (alias, path, parent_fk)

    // Determine the parent PK column(s) for joining.
    let parent_pk_col: String = pk_columns.iter().next().copied().unwrap_or("_src_id").to_string();

    for (i, child) in nested_children.iter().enumerate() {
        let alias = format!("_nested_{i}");
        let child_rev = qi(&format!("_rev_{}", child.mapping.name));

        // Build jsonb_build_object for array element fields.
        let obj_parts: Vec<String> = child
            .item_fields
            .iter()
            .map(|f| format!("'{f}', {}", qi(f)))
            .collect();
        let obj_expr = format!("jsonb_build_object({})", obj_parts.join(", "));

        // Group by the parent FK field (links child back to parent row).
        // Falls back to _src_id if no parent_field is declared.
        let group_col = child.parent_fk_field.as_deref().unwrap_or("_src_id");
        let qgroup = qi(group_col);

        ctes.push(format!(
            "{alias} AS (\n\
             SELECT {qgroup} AS _parent_key, COALESCE(jsonb_agg({obj_expr}), '[]'::jsonb) AS {path}\n\
             FROM {child_rev}\n\
             WHERE {qgroup} IS NOT NULL\n\
             GROUP BY {qgroup}\n\
             )",
            path = qi(child.path),
        ));
        child_join_aliases.push((alias, child.path.to_string(), child.parent_fk_field.clone()));
    }

    // Build the noop detection CASE.
    let mut noop_parts: Vec<String> = Vec::new();
    for fm in &parent.fields {
        if fm.is_reverse() {
            if let Some(ref src) = fm.source {
                if !pk_columns.contains(src.as_str()) {
                    noop_parts.push(format!(
                        "p._base->>'{src}' IS NOT DISTINCT FROM p.{}::text",
                        qi(src)
                    ));
                }
            }
        }
    }
    // Add nested array noop checks: compare original JSONB array with reconstructed.
    for (alias, path, _) in &child_join_aliases {
        let qpath = qi(path);
        noop_parts.push(format!(
            "COALESCE(p._base->>'{path}', '[]') IS NOT DISTINCT FROM COALESCE({alias}.{qpath}::text, '[]')"
        ));
    }

    let mut case_branches = Vec::new();
    if parent.embedded {
        case_branches.push("WHEN p._src_id IS NULL THEN NULL".to_string());
    } else {
        case_branches.push("WHEN p._src_id IS NULL THEN 'insert'".to_string());
    }
    if !noop_parts.is_empty() {
        case_branches.push(format!("WHEN {} THEN 'noop'", noop_parts.join(" AND ")));
    }
    case_branches.push("ELSE 'update'".to_string());
    let case_expr = format!("CASE\n      {}\n    END", case_branches.join("\n      "));

    // Build SELECT columns.
    let mut select_cols: Vec<String> = vec![format!("{case_expr} AS _action")];
    select_cols.push("p.\"_cluster_id\"".to_string());

    // PK columns from parent
    for pk in pk_columns.iter() {
        select_cols.push(format!("p.{}", qi(pk)));
    }

    // Parent reverse fields
    for f in &parent_fields {
        select_cols.push(format!("p.{}", qi(f)));
    }

    // Nested array columns
    for (alias, path, _) in &child_join_aliases {
        select_cols.push(format!("{alias}.{}", qi(path)));
    }

    select_cols.push("p.\"_base\"".to_string());

    // Build JOINs for nested children.
    let mut join_clauses: Vec<String> = Vec::new();
    for (alias, _path, _parent_fk) in &child_join_aliases {
        let qpk = qi(&parent_pk_col);
        join_clauses.push(format!(
            "LEFT JOIN {alias} ON {alias}._parent_key = p.{qpk}::text"
        ));
    }

    let cte_sql = if ctes.is_empty() {
        String::new()
    } else {
        format!("WITH {}\n", ctes.join(",\n"))
    };

    let sql = format!(
        "-- Delta: {source_name} (change detection)\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         {cte_sql}\
         SELECT\n  {columns}\n\
         FROM {parent_rev} AS p\n\
         {joins};\n",
        columns = select_cols.join(",\n  "),
        joins = join_clauses.join("\n"),
    );

    Ok(sql)
}
