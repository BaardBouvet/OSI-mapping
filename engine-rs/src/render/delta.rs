use anyhow::Result;
use indexmap::IndexMap;
use std::collections::BTreeMap;

use crate::model::{Mapping, Source};
use crate::{qi, sql_escape};
use super::forward::{PathSegment, parse_path_segments};

/// A tree node for rebuilding nested JSONB from extracted fields.
enum JsonNode {
    /// Leaf: reference a delta column by its full source_name.
    Leaf(String),
    /// Object branch: build `jsonb_build_object('k1', ..., 'k2', ...)`.
    Object(IndexMap<String, JsonNode>),
    /// Array branch: build `jsonb_build_array(elem0, elem1, ...)`.
    /// Gaps between indices are filled with NULL.
    Array(BTreeMap<i64, JsonNode>),
}

impl JsonNode {
    fn to_sql(&self) -> String {
        match self {
            JsonNode::Leaf(col) => qi(col),
            JsonNode::Object(map) => {
                let parts: Vec<String> = map
                    .iter()
                    .map(|(k, v)| format!("'{}', {}", sql_escape(k), v.to_sql()))
                    .collect();
                format!("jsonb_build_object({})", parts.join(", "))
            }
            JsonNode::Array(map) => {
                if map.is_empty() {
                    return "jsonb_build_array()".to_string();
                }
                let max_idx = *map.keys().next_back().unwrap();
                let mut elems = Vec::new();
                for i in 0..=max_idx {
                    if let Some(node) = map.get(&i) {
                        elems.push(node.to_sql());
                    } else {
                        elems.push("NULL".to_string());
                    }
                }
                format!("jsonb_build_array({})", elems.join(", "))
            }
        }
    }

    /// Insert a value at the given path of key segments.
    fn insert(&mut self, keys: &[PathSegment], full_name: String) {
        if keys.is_empty() {
            *self = JsonNode::Leaf(full_name);
            return;
        }
        match &keys[0] {
            PathSegment::Key(k) => {
                let map = match self {
                    JsonNode::Object(m) => m,
                    _ => {
                        *self = JsonNode::Object(IndexMap::new());
                        match self { JsonNode::Object(m) => m, _ => unreachable!() }
                    }
                };
                let child = map.entry(k.clone()).or_insert_with(|| JsonNode::Object(IndexMap::new()));
                child.insert(&keys[1..], full_name);
            }
            PathSegment::Index(n) => {
                let arr = match self {
                    JsonNode::Array(a) => a,
                    _ => {
                        *self = JsonNode::Array(BTreeMap::new());
                        match self { JsonNode::Array(a) => a, _ => unreachable!() }
                    }
                };
                let child = arr.entry(*n).or_insert_with(|| JsonNode::Object(IndexMap::new()));
                child.insert(&keys[1..], full_name);
            }
        }
    }
}

/// Build `jsonb_build_object(...)` expressions that reconstruct JSONB columns
/// from individual `source_path` fields in the delta output.
///
/// Takes the list of output columns and the mappings, detects `source_path`
/// fields, groups them by physical root column, and returns modified SELECT
/// expressions with JSONB reconstruction replacing individual path columns.
fn delta_output_exprs(
    out_cols: &[String],
    mappings: &[&Mapping],
) -> Vec<String> {
    // Collect source_path info: physical_root → JsonNode tree.
    let mut json_trees: IndexMap<String, JsonNode> = IndexMap::new();
    // Track which source_names belong to which root.
    let mut root_for_name: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for m in mappings {
        for fm in &m.fields {
            if let Some(ref sp) = fm.source_path {
                if fm.is_reverse() {
                    let segments = parse_path_segments(sp);
                    let root = match &segments[0] {
                        PathSegment::Key(k) => k.clone(),
                        PathSegment::Index(n) => n.to_string(),
                    };
                    let full_name = fm.source_name().unwrap().to_string();
                    let sub_keys = &segments[1..];

                    let tree = json_trees
                        .entry(root.clone())
                        .or_insert_with(|| JsonNode::Object(IndexMap::new()));
                    tree.insert(sub_keys, full_name.clone());
                    root_for_name.insert(full_name, root);
                }
            }
        }
    }

    if json_trees.is_empty() {
        return out_cols.iter().map(|c| qi(c)).collect();
    }

    let mut exprs = Vec::new();
    let mut emitted_roots: std::collections::HashSet<String> = std::collections::HashSet::new();

    for col in out_cols {
        if let Some(root) = root_for_name.get(col.as_str()) {
            // This column is a source_path sub-field — emit the reconstructed tree once.
            if !emitted_roots.contains(root) {
                emitted_roots.insert(root.clone());
                let tree = &json_trees[root];
                exprs.push(format!("{} AS {}", tree.to_sql(), qi(root)));
            }
        } else {
            exprs.push(qi(col));
        }
    }

    exprs
}

/// A node in the nesting tree for delta re-assembly.
/// Each node represents one level of nested arrays (e.g. "children" or "grandchildren").
struct NestingNode<'a> {
    /// The segment name (last part of path, e.g. "grandchildren").
    segment: String,
    /// The mapping for this nesting level.
    mapping: &'a Mapping,
    /// Source field names that are array element data (non-PK, non-parent-alias).
    item_fields: Vec<String>,
    /// The parent_field alias that links back to the parent level (for GROUP BY).
    parent_fk_field: Option<String>,
    /// Child nesting levels (deeper arrays within this one).
    children: Vec<NestingNode<'a>>,
}

/// Collected CTE output from recursive nesting tree traversal.
struct NestedCteResult {
    /// All CTE definitions (bottom-up order, leaves first).
    ctes: Vec<String>,
    /// The alias of the top-level CTE for this node.
    alias: String,
    /// The JSONB column name this node produces (= segment name).
    column: String,
}

/// Build the CASE expression that classifies a row from a single mapping's
/// reverse view as insert / delete / noop / update.
fn action_case(mapping: &Mapping, pk_columns: &std::collections::HashSet<&str>) -> String {
    // Delete conditions from reverse_required + reverse_filter.
    let mut delete_conditions: Vec<String> = Vec::new();
    for fm in &mapping.fields {
        if fm.reverse_required {
            if let Some(src) = fm.source_name() {
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
        .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
        .filter_map(|fm| {
            let src = fm.source_name()?;
            if pk_columns.contains(src) {
                return None;
            }
            Some(format!("_base->>'{}' IS NOT DISTINCT FROM {}::text", sql_escape(src), qi(src)))
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
            && fm.source_name().map_or(false, |s| !pk_columns.contains(s))
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

    // Build nesting tree from all nested-path mappings (including multi-segment).
    let nesting_roots = build_nesting_tree(mappings, &pk_columns);

    // If there are nested children, group them by parent mapping and aggregate.
    if !parent_mappings.is_empty() && !nesting_roots.is_empty() {
        return render_delta_with_nested(
            source_name,
            &view_name,
            &parent_mappings,
            &nesting_roots,
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
                if let Some(src) = fm.source_name() {
                    if !pk_columns.contains(src) && !reverse_fields.contains(&src.to_string()) {
                        reverse_fields.push(src.to_string());
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
        cols.extend(delta_output_exprs(&out_cols, mappings));

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

    // Multiple mappings: check for embedded pattern (primary + embedded → merged row)
    // vs pure routing pattern (all non-embedded → UNION ALL).
    let primary: Vec<&&Mapping> = mappings.iter().filter(|m| !m.embedded).collect();
    let embedded_with_reverse: Vec<&&Mapping> = mappings
        .iter()
        .filter(|m| {
            m.embedded
                && m.fields.iter().any(|f| {
                    f.is_reverse()
                        && f.source_name()
                            .map_or(false, |s| !pk_columns.contains(s))
                })
        })
        .collect();

    if !primary.is_empty() && !embedded_with_reverse.is_empty() {
        return render_delta_with_embedded(
            source_name,
            &view_name,
            &primary,
            &embedded_with_reverse,
            &reverse_fields,
            &pk_columns,
            &out_cols,
            source_meta,
        );
    }

    // Multiple mappings without embedded merge: UNION ALL approach.
    let all_mappings: Vec<&Mapping> = mappings.to_vec();
    render_delta_union_all(source_name, &view_name, &all_mappings, &reverse_fields, &pk_columns, &out_cols)
}

/// Build the CASE expression for a merged embedded delta.
///
/// Insert/delete logic comes from primary mappings only.
/// Noop checks ALL reverse fields against the merged `_base`.
fn merged_action_case(
    primary_mappings: &[&&Mapping],
    pk_columns: &std::collections::HashSet<&str>,
    all_reverse_fields: &[String],
) -> String {
    // Delete conditions from primary mappings only.
    let mut delete_conditions: Vec<String> = Vec::new();
    for m in primary_mappings {
        for fm in &m.fields {
            if fm.reverse_required {
                if let Some(src) = fm.source_name() {
                    delete_conditions.push(format!("{} IS NULL", qi(src)));
                }
            }
        }
        if let Some(ref rf) = m.reverse_filter {
            delete_conditions.push(format!("({rf}) IS NOT TRUE"));
        }
    }

    // Noop: all non-PK reverse fields match merged _base.
    let noop_parts: Vec<String> = all_reverse_fields
        .iter()
        .filter(|f| !pk_columns.contains(f.as_str()))
        .map(|src| format!("_base->>'{}' IS NOT DISTINCT FROM {}::text", sql_escape(src), qi(src)))
        .collect();

    let mut branches = Vec::new();

    if !delete_conditions.is_empty() {
        branches.push(format!(
            "WHEN _src_id IS NULL AND ({}) THEN NULL",
            delete_conditions.join(" OR ")
        ));
    }
    branches.push("WHEN _src_id IS NULL THEN 'insert'".to_string());

    if !delete_conditions.is_empty() {
        branches.push(format!(
            "WHEN {} THEN 'delete'",
            delete_conditions.join(" OR ")
        ));
    }

    if !noop_parts.is_empty() {
        branches.push(format!("WHEN {} THEN 'noop'", noop_parts.join(" AND ")));
    }

    let has_non_pk_reverse = all_reverse_fields
        .iter()
        .any(|f| !pk_columns.contains(f.as_str()));
    if !has_non_pk_reverse {
        branches.push("ELSE 'noop'".to_string());
    } else {
        branches.push("ELSE 'update'".to_string());
    }

    format!("CASE\n      {}\n    END", branches.join("\n      "))
}

/// Render a delta view that merges embedded mappings into the parent row
/// via LEFT JOIN on `_src_id`, producing one row per source record.
///
/// The merged `_base` combines JSONB from primary + all embedded, enabling
/// unified noop detection across all fields.
fn render_delta_with_embedded(
    source_name: &str,
    view_name: &str,
    primary_mappings: &[&&Mapping],
    embedded_mappings: &[&&Mapping],
    reverse_fields: &[String],
    pk_columns: &std::collections::HashSet<&str>,
    out_cols: &[String],
    source_meta: Option<&Source>,
) -> Result<String> {
    // --- Field ownership: which alias provides each reverse field ---
    let primary_fields: std::collections::HashSet<&str> = primary_mappings
        .iter()
        .flat_map(|m| m.fields.iter())
        .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
        .filter_map(|fm| fm.source_name())
        .filter(|s| !pk_columns.contains(*s))
        .collect();

    // For each embedded mapping, find fields it uniquely contributes.
    struct EmbInfo<'a> {
        mapping: &'a Mapping,
        alias: String,
        fields: Vec<String>,
    }
    let mut claimed: std::collections::HashSet<String> = primary_fields.iter().map(|s| s.to_string()).collect();
    let mut emb_infos: Vec<EmbInfo> = Vec::new();
    for (i, m) in embedded_mappings.iter().enumerate() {
        let alias = format!("_e{}", i + 1);
        let new_fields: Vec<String> = m
            .fields
            .iter()
            .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
            .filter_map(|fm| fm.source_name().map(|s| s.to_string()))
            .filter(|s| !pk_columns.contains(s.as_str()) && !claimed.contains(s.as_str()))
            .collect();
        for f in &new_fields {
            claimed.insert(f.clone());
        }
        if !new_fields.is_empty() {
            emb_infos.push(EmbInfo {
                mapping: m,
                alias,
                fields: new_fields,
            });
        }
    }

    // If no embedded mapping contributes unique fields, fall through to UNION ALL.
    if emb_infos.is_empty() {
        // Delegate to the standard UNION ALL path by returning a union of all mappings.
        let all: Vec<&Mapping> = primary_mappings
            .iter()
            .chain(embedded_mappings.iter())
            .map(|m| **m)
            .collect();
        return render_delta_union_all(source_name, view_name, &all, reverse_fields, pk_columns, out_cols);
    }

    // Field → alias map for sourcing columns in the merged CTE.
    let mut field_alias: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for info in &emb_infos {
        for f in &info.fields {
            field_alias.insert(f.as_str(), info.alias.as_str());
        }
    }
    // Primary fields: alias "_p"
    for f in &primary_fields {
        field_alias.insert(f, "_p");
    }

    // --- Build CTEs ---
    let mut ctes: Vec<String> = Vec::new();

    // Primary CTE
    if primary_mappings.len() == 1 {
        let rev = qi(&format!("_rev_{}", primary_mappings[0].name));
        ctes.push(format!("_p AS (SELECT * FROM {rev})"));
    } else {
        // Multiple primaries: UNION ALL with column normalization.
        let primary_selects: Vec<String> = primary_mappings
            .iter()
            .map(|m| {
                let m_fields: std::collections::HashSet<&str> = m
                    .fields
                    .iter()
                    .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
                    .filter_map(|fm| fm.source_name())
                    .collect();
                // Primary-only out_cols (without embedded-unique fields).
                let mut projected: Vec<String> = vec![
                    "_src_id".to_string(),
                    "\"_cluster_id\"".to_string(),
                ];
                if let Some(src) = source_meta {
                    for col in src.primary_key.columns() {
                        projected.push(qi(col));
                    }
                }
                for f in reverse_fields {
                    if primary_fields.contains(f.as_str()) {
                        if m_fields.contains(f.as_str()) {
                            projected.push(qi(f));
                        } else {
                            projected.push(format!("NULL::text AS {}", qi(f)));
                        }
                    }
                }
                projected.push("\"_base\"".to_string());
                format!(
                    "SELECT {} FROM {}",
                    projected.join(", "),
                    qi(&format!("_rev_{}", m.name))
                )
            })
            .collect();
        ctes.push(format!("_p AS ({})", primary_selects.join(" UNION ALL ")));
    }

    // Embedded CTEs: only the unique fields + _src_id + _base
    for info in &emb_infos {
        let rev = qi(&format!("_rev_{}", info.mapping.name));
        let mut cols: Vec<String> = vec!["_src_id".to_string()];
        for f in &info.fields {
            cols.push(qi(f));
        }
        cols.push("_base".to_string());
        ctes.push(format!("{} AS (SELECT {} FROM {})", info.alias, cols.join(", "), rev));
    }

    // Merged CTE: LEFT JOIN embedded on _src_id, merge _base.
    let mut merged_cols: Vec<String> = vec![
        "_p._src_id".to_string(),
        "_p.\"_cluster_id\"".to_string(),
    ];
    if let Some(src) = source_meta {
        for col in src.primary_key.columns() {
            merged_cols.push(format!("_p.{}", qi(col)));
        }
    }
    for field in reverse_fields {
        let alias = field_alias.get(field.as_str()).copied().unwrap_or("_p");
        merged_cols.push(format!("{alias}.{}", qi(field)));
    }
    // Merged _base: COALESCE each mapping's _base and merge with ||.
    let base_parts: Vec<String> = std::iter::once("COALESCE(_p._base, '{}'::jsonb)".to_string())
        .chain(
            emb_infos
                .iter()
                .map(|e| format!("COALESCE({}._base, '{{}}'::jsonb)", e.alias)),
        )
        .collect();
    merged_cols.push(format!("{} AS _base", base_parts.join(" || ")));

    let joins: Vec<String> = emb_infos
        .iter()
        .map(|e| format!("LEFT JOIN {} ON {}._src_id = _p._src_id", e.alias, e.alias))
        .collect();

    ctes.push(format!(
        "_merged AS (\n    SELECT {}\n    FROM _p\n    {}\n  )",
        merged_cols.join(",\n      "),
        joins.join("\n    "),
    ));

    // --- Outer SELECT with merged action ---
    let all_mappings_for_output: Vec<&Mapping> = primary_mappings
        .iter()
        .chain(embedded_mappings.iter())
        .map(|m| **m)
        .collect();
    let action_expr = merged_action_case(primary_mappings, pk_columns, reverse_fields);
    let mut outer_cols: Vec<String> = vec![format!("{action_expr} AS _action")];
    outer_cols.extend(delta_output_exprs(out_cols, &all_mappings_for_output.iter().collect::<Vec<_>>().iter().map(|m| **m).collect::<Vec<_>>()));

    // Include target fields for reverse_filter pass-through from primary.
    // These are already projected in the primary reverse view; we just need
    // them in the outer SELECT for the filter references in action_case.
    if let Some(m) = primary_mappings.first() {
        if let Some(ref rf) = m.reverse_filter {
            // Check if filter references fields not already in out_cols.
            // reverse_filter fields are target fields added to reverse view.
            // They appear in _p.* but may not be in out_cols.
            // The action_case references them directly so they must be in _merged.
            // Since _merged uses _p.*, they're available.  No extra work needed
            // because _merged gets them from the _p CTE (SELECT * FROM _rev_...).
            let _ = rf; // suppress unused warning
        }
    }

    let sql = format!(
        "-- Delta: {source_name} (change detection)\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         WITH\n  {ctes}\n\
         SELECT\n  {columns}\n\
         FROM _merged;\n",
        ctes = ctes.join(",\n  "),
        columns = outer_cols.join(",\n  "),
    );

    Ok(sql)
}

/// Standard UNION ALL delta for multiple mappings without embedded merge.
fn render_delta_union_all(
    source_name: &str,
    view_name: &str,
    mappings: &[&Mapping],
    _reverse_fields: &[String],
    pk_columns: &std::collections::HashSet<&str>,
    out_cols: &[String],
) -> Result<String> {
    let selects: Vec<String> = mappings
        .iter()
        .map(|m| {
            let case_expr = action_case(m, pk_columns);
            let m_fields: std::collections::HashSet<&str> = m
                .fields
                .iter()
                .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
                .filter_map(|fm| fm.source_name())
                .collect();
            let mut projected: Vec<String> = vec![format!("{case_expr} AS _action")];
            for col in out_cols {
                if col == "_cluster_id"
                    || col == "_base"
                    || pk_columns.contains(col.as_str())
                {
                    projected.push(qi(col));
                } else if m_fields.contains(col.as_str()) {
                    projected.push(qi(col));
                } else {
                    projected.push(format!("NULL::text AS {}", qi(col)));
                }
            }
            format!(
                "SELECT {} FROM {}",
                projected.join(", "),
                qi(&format!("_rev_{}", m.name))
            )
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

/// Build a nesting tree from all nested-path mappings.
///
/// Organizes mappings into a tree based on their `source.path` segments:
/// - `children` → root child
/// - `children.grandchildren` → child of "children" node
/// - `projects` → another root child
/// - `projects.tasks` → child of "projects" node
fn build_nesting_tree<'a>(
    mappings: &[&'a Mapping],
    pk_columns: &std::collections::HashSet<&str>,
) -> Vec<NestingNode<'a>> {
    // Collect all nested mappings with their parsed info.
    struct FlatNested<'a> {
        segments: Vec<String>,
        mapping: &'a Mapping,
        item_fields: Vec<String>,
        parent_fk_field: Option<String>,
    }

    let mut all_nested: Vec<FlatNested<'a>> = Vec::new();
    for m in mappings {
        if let Some(path) = m.source.path.as_deref() {
            let parent_aliases: std::collections::HashSet<String> =
                m.source.parent_fields.keys().cloned().collect();
            let item_fields: Vec<String> = m
                .fields
                .iter()
                .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
                .filter_map(|fm| fm.source_name().map(|s| s.to_string()))
                .filter(|src| !pk_columns.contains(src.as_str()) && !parent_aliases.contains(src))
                .collect();
            let parent_fk_field = m.source.parent_fields.keys().next().cloned();
            let segments: Vec<String> = path.split('.').map(|s| s.to_string()).collect();
            all_nested.push(FlatNested {
                segments,
                mapping: m,
                item_fields,
                parent_fk_field,
            });
        }
    }

    // Sort by depth (shallow first) so parents are inserted before children.
    all_nested.sort_by_key(|n| n.segments.len());

    // Build tree: insert each mapping at the correct depth.
    let mut roots: Vec<NestingNode<'a>> = Vec::new();

    for flat in all_nested {
        if flat.segments.len() == 1 {
            // Direct child of root.
            roots.push(NestingNode {
                segment: flat.segments[0].clone(),
                mapping: flat.mapping,
                item_fields: flat.item_fields,
                parent_fk_field: flat.parent_fk_field,
                children: Vec::new(),
            });
        } else {
            // Multi-segment path: find the parent node and insert as child.
            let parent_segments = &flat.segments[..flat.segments.len() - 1];
            let leaf_segment = flat.segments.last().unwrap().clone();
            if let Some(parent_node) = find_node_mut(&mut roots, parent_segments) {
                parent_node.children.push(NestingNode {
                    segment: leaf_segment,
                    mapping: flat.mapping,
                    item_fields: flat.item_fields,
                    parent_fk_field: flat.parent_fk_field,
                    children: Vec::new(),
                });
            }
        }
    }

    roots
}

/// Find a node in the nesting tree by its path segments.
fn find_node_mut<'a, 'b>(
    nodes: &'b mut [NestingNode<'a>],
    segments: &[String],
) -> Option<&'b mut NestingNode<'a>> {
    if segments.is_empty() {
        return None;
    }
    let first = &segments[0];
    for node in nodes.iter_mut() {
        if &node.segment == first {
            if segments.len() == 1 {
                return Some(node);
            }
            return find_node_mut(&mut node.children, &segments[1..]);
        }
    }
    None
}

/// Recursively generate CTEs for a nesting node (bottom-up: children first).
///
/// Returns all CTE definitions and the alias/column name of the top-level CTE.
fn build_nested_ctes(node: &NestingNode, cte_prefix: &str) -> NestedCteResult {
    let alias = format!("{cte_prefix}_{}", node.segment);
    let rev_view = qi(&format!("_rev_{}", node.mapping.name));
    let group_col = node.parent_fk_field.as_deref().unwrap_or("_src_id");

    // First, recursively process all children.
    let mut all_ctes: Vec<String> = Vec::new();
    let mut child_results: Vec<NestedCteResult> = Vec::new();
    for child in &node.children {
        let child_result = build_nested_ctes(child, &alias);
        all_ctes.extend(child_result.ctes.clone());
        child_results.push(child_result);
    }

    // Build jsonb_build_object parts for this node's own fields.
    let table_alias = if child_results.is_empty() { "" } else { "n." };
    let mut obj_parts: Vec<String> = node
        .item_fields
        .iter()
        .map(|f| format!("'{f}', {table_alias}{}", qi(f)))
        .collect();

    // Add child array columns to the object.
    for cr in &child_results {
        obj_parts.push(format!(
            "'{seg}', COALESCE({alias}.{qcol}, '[]'::jsonb)",
            seg = cr.column,
            alias = cr.alias,
            qcol = qi(&cr.column),
        ));
    }

    let obj_expr = format!("jsonb_build_object({})", obj_parts.join(", "));
    let qgroup = qi(group_col);
    let qsegment = qi(&node.segment);

    if child_results.is_empty() {
        // Leaf node: simple aggregation.
        all_ctes.push(format!(
            "{alias} AS (\n\
             SELECT {qgroup} AS _parent_key, \
             COALESCE(jsonb_agg({obj_expr} ORDER BY {table_alias}{first_item}), '[]'::jsonb) AS {qsegment}\n\
             FROM {rev_view}\n\
             WHERE {qgroup} IS NOT NULL\n\
             GROUP BY {qgroup}\n\
             )",
            first_item = qi(node.item_fields.first().map(|s| s.as_str()).unwrap_or("_src_id")),
        ));
    } else {
        // Interior node: join child CTEs, then aggregate.
        let mut joins = Vec::new();
        for cr in &child_results {
            // The child CTE groups by _parent_key which corresponds to the
            // child's parent_fk_field. We join it to this node's identity column —
            // which is the first item_field (typically the PK-like field).
            let join_col = node.item_fields.first().map(|s| s.as_str()).unwrap_or("_src_id");
            joins.push(format!(
                "LEFT JOIN {child_alias} ON {child_alias}._parent_key = n.{qjoin}::text",
                child_alias = cr.alias,
                qjoin = qi(join_col),
            ));
        }

        let first_item = qi(node.item_fields.first().map(|s| s.as_str()).unwrap_or("_src_id"));
        all_ctes.push(format!(
            "{alias} AS (\n\
             SELECT n.{qgroup} AS _parent_key, \
             COALESCE(jsonb_agg({obj_expr} ORDER BY n.{first_item}), '[]'::jsonb) AS {qsegment}\n\
             FROM {rev_view} AS n\n\
             {joins}\n\
             WHERE n.{qgroup} IS NOT NULL\n\
             GROUP BY n.{qgroup}\n\
             )",
            joins = joins.join("\n"),
        ));
    }

    NestedCteResult {
        ctes: all_ctes,
        alias,
        column: node.segment.clone(),
    }
}

/// Render a delta view that aggregates nested-path child mappings into JSONB
/// arrays and joins them onto the parent reverse view.
fn render_delta_with_nested(
    source_name: &str,
    view_name: &str,
    parent_mappings: &[&&Mapping],
    nesting_roots: &[NestingNode],
    pk_columns: &std::collections::HashSet<&str>,
    _source_meta: Option<&Source>,
) -> Result<String> {
    // For now, use the first parent mapping as the base.
    let parent = parent_mappings[0];
    let parent_rev = qi(&format!("_rev_{}", parent.name));

    // Collect parent's reverse source fields (non-PK).
    let parent_fields: Vec<String> = parent
        .fields
        .iter()
        .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
        .filter_map(|fm| fm.source_name().map(|s| s.to_string()))
        .filter(|src| !pk_columns.contains(src.as_str()))
        .collect();

    // Generate CTEs recursively for each root nesting node.
    let mut all_ctes: Vec<String> = Vec::new();
    let mut root_results: Vec<NestedCteResult> = Vec::new();
    for node in nesting_roots {
        let result = build_nested_ctes(node, "_nested");
        all_ctes.extend(result.ctes.clone());
        root_results.push(result);
    }

    // Determine the parent PK column(s) for joining.
    let parent_pk_col: String = pk_columns.iter().next().copied().unwrap_or("_src_id").to_string();

    // Build the noop detection CASE.
    let mut noop_parts: Vec<String> = Vec::new();
    for fm in &parent.fields {
        if fm.is_reverse() {
            if let Some(src) = fm.source_name() {
                if !pk_columns.contains(src) {
                    noop_parts.push(format!(
                        "p._base->>'{}' IS NOT DISTINCT FROM p.{}::text",
                        sql_escape(src), qi(src)
                    ));
                }
            }
        }
    }
    // Add nested array noop checks: compare original JSONB array with reconstructed.
    // Use _osi_text_norm on BOTH sides to normalize types (integers etc.) to text
    // so the comparison is type-agnostic even when target fields declare type: numeric.
    for rr in &root_results {
        let qcol = qi(&rr.column);
        noop_parts.push(format!(
            "COALESCE(_osi_text_norm(p._base->'{col}')::text, '[]') IS NOT DISTINCT FROM COALESCE(_osi_text_norm({alias}.{qcol})::text, '[]')",
            col = rr.column,
            alias = rr.alias,
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

    // Nested array columns (from root-level CTEs only)
    for rr in &root_results {
        select_cols.push(format!("{}.{}", rr.alias, qi(&rr.column)));
    }

    select_cols.push("p.\"_base\"".to_string());

    // Build JOINs for root nested CTEs.
    let mut join_clauses: Vec<String> = Vec::new();
    let qpk = qi(&parent_pk_col);
    for rr in &root_results {
        join_clauses.push(format!(
            "LEFT JOIN {} ON {}._parent_key = p.{qpk}::text",
            rr.alias, rr.alias,
        ));
    }

    let cte_sql = if all_ctes.is_empty() {
        String::new()
    } else {
        format!("WITH {}\n", all_ctes.join(",\n"))
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
