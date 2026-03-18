use anyhow::Result;
use indexmap::IndexMap;
use std::collections::BTreeMap;

use super::forward::{parse_path_segments, PathSegment};
use crate::model::{Mapping, Source};
use crate::{qi, sql_escape};

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
                        match self {
                            JsonNode::Object(m) => m,
                            _ => unreachable!(),
                        }
                    }
                };
                let child = map
                    .entry(k.clone())
                    .or_insert_with(|| JsonNode::Object(IndexMap::new()));
                child.insert(&keys[1..], full_name);
            }
            PathSegment::Index(n) => {
                let arr = match self {
                    JsonNode::Array(a) => a,
                    _ => {
                        *self = JsonNode::Array(BTreeMap::new());
                        match self {
                            JsonNode::Array(a) => a,
                            _ => unreachable!(),
                        }
                    }
                };
                let child = arr
                    .entry(*n)
                    .or_insert_with(|| JsonNode::Object(IndexMap::new()));
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
fn delta_output_exprs(out_cols: &[String], mappings: &[&Mapping]) -> Vec<String> {
    // Collect source_path info: physical_root → JsonNode tree.
    let mut json_trees: IndexMap<String, JsonNode> = IndexMap::new();
    // Track which source_names belong to which root.
    let mut root_for_name: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

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
    /// Target field name of the `order: true` field, if any.
    /// When present, `jsonb_agg` uses this for ORDER BY instead of first item_field.
    order_field: Option<String>,
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
///
/// When `written_col` is `Some`, a second noop branch compares resolved fields
/// against the `_ws.{written_col}` JSONB column (target-centric noop).
fn action_case(
    mapping: &Mapping,
    pk_columns: &std::collections::HashSet<&str>,
    written_col: Option<&str>,
) -> String {
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
            Some(format!(
                "_base->>'{}' IS NOT DISTINCT FROM {}::text",
                sql_escape(src),
                qi(src)
            ))
        })
        .collect();

    let mut branches = Vec::new();

    if mapping.is_child() && !mapping.is_nested() {
        // Child mappings extract data from a shared source table.
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
    // Target-centric noop: compare resolved fields against _written.
    if let Some(wcol) = written_col {
        let written_noop_parts: Vec<String> = mapping
            .fields
            .iter()
            .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
            .filter_map(|fm| {
                let src = fm.source_name()?;
                if pk_columns.contains(src) {
                    return None;
                }
                Some(format!(
                    "_ws.{}->>'{}' IS NOT DISTINCT FROM {}::text",
                    qi(wcol),
                    sql_escape(src),
                    qi(src)
                ))
            })
            .collect();
        if !written_noop_parts.is_empty() {
            branches.push(format!(
                "WHEN _ws.{} IS NOT NULL AND {} THEN 'noop'",
                qi(wcol),
                written_noop_parts.join(" AND ")
            ));
        }
    }
    // When all reverse-mapped source fields are PK columns, there's nothing
    // to compare — existing rows are always noops.
    let has_non_pk_reverse = mapping
        .fields
        .iter()
        .any(|fm| fm.is_reverse() && fm.source_name().is_some_and(|s| !pk_columns.contains(s)));
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
        let mapping = mappings[0];
        let written_col = if mapping.written_noop {
            mapping.written_state.as_ref().map(|ws| ws.written.as_str())
        } else {
            None
        };
        let action_expr = action_case(mapping, &pk_columns, written_col);
        let mut cols: Vec<String> = vec![format!("{action_expr} AS _action")];
        let mut out_exprs = delta_output_exprs(&out_cols, mappings);
        let rev_view = qi(&format!("_rev_{}", mapping.name));
        // Qualify _cluster_id to avoid ambiguity with LEFT JOINed _written.
        if mapping.written_state.is_some() {
            let qi_cluster = qi("_cluster_id");
            if let Some(pos) = out_exprs.iter().position(|e| e == &qi_cluster) {
                out_exprs[pos] = format!("{rev_view}.{qi_cluster}");
            }
        }
        cols.extend(out_exprs);

        let mut from = rev_view.clone();
        if let Some(ref ws) = mapping.written_state {
            let ws_table = qi(&ws.table_name(&mapping.name));
            let ws_cluster = qi(&ws.cluster_id);
            from.push_str(&format!(
                "\nLEFT JOIN {ws_table} AS _ws ON _ws.{ws_cluster} = {rev_view}.{}",
                qi("_cluster_id")
            ));
        }

        let sql = format!(
            "-- Delta: {source_name} (change detection)\n\
             CREATE OR REPLACE VIEW {view_name} AS\n\
             SELECT\n  {columns}\n\
             FROM {from};\n",
            columns = cols.join(",\n  "),
        );
        return Ok(sql);
    }

    // Multiple mappings: check for child pattern (primary + child → merged row)
    // vs pure routing pattern (all non-child → UNION ALL).
    let primary: Vec<&&Mapping> = mappings.iter().filter(|m| !m.is_child()).collect();
    let children_with_reverse: Vec<&&Mapping> = mappings
        .iter()
        .filter(|m| {
            m.is_child()
                && !m.is_nested()
                && m.fields.iter().any(|f| {
                    f.is_reverse() && f.source_name().is_some_and(|s| !pk_columns.contains(s))
                })
        })
        .collect();

    if !primary.is_empty() && !children_with_reverse.is_empty() {
        return render_delta_with_children(
            source_name,
            &view_name,
            &primary,
            &children_with_reverse,
            &reverse_fields,
            &pk_columns,
            &out_cols,
            source_meta,
        );
    }

    // Multiple mappings without child merge: UNION ALL approach.
    let all_mappings: Vec<&Mapping> = mappings.to_vec();
    render_delta_union_all(
        source_name,
        &view_name,
        &all_mappings,
        &reverse_fields,
        &pk_columns,
        &out_cols,
    )
}

/// Build the CASE expression for a merged child delta.
///
/// Insert/delete logic comes from primary mappings only.
/// Noop checks ALL reverse fields against the merged `_base`.
fn merged_action_case(
    primary_mappings: &[&&Mapping],
    pk_columns: &std::collections::HashSet<&str>,
    all_reverse_fields: &[String],
    written_col: Option<&str>,
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
        .map(|src| {
            format!(
                "_base->>'{}' IS NOT DISTINCT FROM {}::text",
                sql_escape(src),
                qi(src)
            )
        })
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

    // Target-centric noop: compare resolved fields against _written.
    if let Some(wcol) = written_col {
        let written_noop_parts: Vec<String> = all_reverse_fields
            .iter()
            .filter(|f| !pk_columns.contains(f.as_str()))
            .map(|src| {
                format!(
                    "_ws.{}->>'{}' IS NOT DISTINCT FROM {}::text",
                    qi(wcol),
                    sql_escape(src),
                    qi(src)
                )
            })
            .collect();
        if !written_noop_parts.is_empty() {
            branches.push(format!(
                "WHEN _ws.{} IS NOT NULL AND {} THEN 'noop'",
                qi(wcol),
                written_noop_parts.join(" AND ")
            ));
        }
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

/// Render a delta view that merges child mappings into the parent row
/// via LEFT JOIN on `_src_id`, producing one row per source record.
///
/// The merged `_base` combines JSONB from primary + all children, enabling
/// unified noop detection across all fields.
#[allow(clippy::too_many_arguments)]
fn render_delta_with_children(
    source_name: &str,
    view_name: &str,
    primary_mappings: &[&&Mapping],
    child_mappings: &[&&Mapping],
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

    // For each child mapping, find fields it uniquely contributes.
    struct ChildInfo<'a> {
        mapping: &'a Mapping,
        alias: String,
        fields: Vec<String>,
    }
    let mut claimed: std::collections::HashSet<String> =
        primary_fields.iter().map(|s| s.to_string()).collect();
    let mut child_infos: Vec<ChildInfo> = Vec::new();
    for (i, m) in child_mappings.iter().enumerate() {
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
            child_infos.push(ChildInfo {
                mapping: m,
                alias,
                fields: new_fields,
            });
        }
    }

    // If no child mapping contributes unique fields, fall through to UNION ALL.
    if child_infos.is_empty() {
        // Delegate to the standard UNION ALL path by returning a union of all mappings.
        let all: Vec<&Mapping> = primary_mappings
            .iter()
            .chain(child_mappings.iter())
            .map(|m| **m)
            .collect();
        return render_delta_union_all(
            source_name,
            view_name,
            &all,
            reverse_fields,
            pk_columns,
            out_cols,
        );
    }

    // Field → alias map for sourcing columns in the merged CTE.
    let mut field_alias: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for info in &child_infos {
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
                // Primary-only out_cols (without child-unique fields).
                let mut projected: Vec<String> =
                    vec!["_src_id".to_string(), "\"_cluster_id\"".to_string()];
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

    // Child CTEs: only the unique fields + _src_id + _base
    for info in &child_infos {
        let rev = qi(&format!("_rev_{}", info.mapping.name));
        let mut cols: Vec<String> = vec!["_src_id".to_string()];
        for f in &info.fields {
            cols.push(qi(f));
        }
        cols.push("_base".to_string());
        ctes.push(format!(
            "{} AS (SELECT {} FROM {})",
            info.alias,
            cols.join(", "),
            rev
        ));
    }

    // Merged CTE: LEFT JOIN children on _src_id, merge _base.
    let mut merged_cols: Vec<String> =
        vec!["_p._src_id".to_string(), "_p.\"_cluster_id\"".to_string()];
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
            child_infos
                .iter()
                .map(|e| format!("COALESCE({}._base, '{{}}'::jsonb)", e.alias)),
        )
        .collect();
    merged_cols.push(format!("{} AS _base", base_parts.join(" || ")));

    let joins: Vec<String> = child_infos
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
        .chain(child_mappings.iter())
        .map(|m| **m)
        .collect();
    // Use written_state from the first primary mapping that declares it.
    let primary_ws = primary_mappings
        .iter()
        .find_map(|m| m.written_state.as_ref());
    let written_col = if primary_mappings.iter().any(|m| m.written_noop) {
        primary_ws.map(|ws| ws.written.as_str())
    } else {
        None
    };
    let action_expr = merged_action_case(primary_mappings, pk_columns, reverse_fields, written_col);
    let mut outer_cols: Vec<String> = vec![format!("{action_expr} AS _action")];
    let mut out_exprs = delta_output_exprs(
        out_cols,
        &all_mappings_for_output
            .iter()
            .collect::<Vec<_>>()
            .iter()
            .map(|m| **m)
            .collect::<Vec<_>>(),
    );
    // Qualify _cluster_id when _written is joined.
    if primary_ws.is_some() {
        let qi_cluster = qi("_cluster_id");
        if let Some(pos) = out_exprs.iter().position(|e| e == &qi_cluster) {
            out_exprs[pos] = format!("_merged.{qi_cluster}");
        }
    }
    outer_cols.extend(out_exprs);

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

    let mut from = "_merged".to_string();
    if let Some(ws) = primary_ws {
        let ws_table = qi(&ws.table_name(&primary_mappings[0].name));
        let ws_cluster = qi(&ws.cluster_id);
        from.push_str(&format!(
            "\nLEFT JOIN {ws_table} AS _ws ON _ws.{ws_cluster} = _merged.{}",
            qi("_cluster_id")
        ));
    }

    let sql = format!(
        "-- Delta: {source_name} (change detection)\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         WITH\n  {ctes}\n\
         SELECT\n  {columns}\n\
         FROM {from};\n",
        ctes = ctes.join(",\n  "),
        columns = outer_cols.join(",\n  "),
    );

    Ok(sql)
}

/// Standard UNION ALL delta for multiple mappings without child merge.
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
            let written_col = if m.written_noop {
                m.written_state.as_ref().map(|ws| ws.written.as_str())
            } else {
                None
            };
            let case_expr = action_case(m, pk_columns, written_col);
            let rev_view = qi(&format!("_rev_{}", m.name));
            let m_fields: std::collections::HashSet<&str> = m
                .fields
                .iter()
                .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
                .filter_map(|fm| fm.source_name())
                .collect();
            let mut projected: Vec<String> = vec![format!("{case_expr} AS _action")];
            for col in out_cols {
                if col == "_cluster_id" && m.written_state.is_some() {
                    projected.push(format!("{rev_view}.{}", qi(col)));
                } else if col == "_cluster_id"
                    || col == "_base"
                    || pk_columns.contains(col.as_str())
                    || m_fields.contains(col.as_str())
                {
                    projected.push(qi(col));
                } else {
                    projected.push(format!("NULL::text AS {}", qi(col)));
                }
            }
            let mut from = rev_view.clone();
            if let Some(ref ws) = m.written_state {
                let ws_table = qi(&ws.table_name(&m.name));
                let ws_cluster = qi(&ws.cluster_id);
                from.push_str(&format!(
                    " LEFT JOIN {ws_table} AS _ws ON _ws.{ws_cluster} = {rev_view}.{}",
                    qi("_cluster_id")
                ));
            }
            format!("SELECT {} FROM {}", projected.join(", "), from)
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
        order_field: Option<String>,
    }

    let mut all_nested: Vec<FlatNested<'a>> = Vec::new();
    for m in mappings {
        if let Some(path) = m.source.path.as_deref() {
            let parent_aliases: std::collections::HashSet<String> =
                m.source.parent_fields.keys().cloned().collect();
            let mut item_fields: Vec<String> = m
                .fields
                .iter()
                .filter(|fm| fm.is_reverse() && fm.source_name().is_some())
                .filter_map(|fm| fm.source_name().map(|s| s.to_string()))
                .filter(|src| !pk_columns.contains(src.as_str()) && !parent_aliases.contains(src))
                .collect();
            // Include order/order_prev/order_next target names (no source_name)
            for fm in &m.fields {
                if (fm.order || fm.order_prev || fm.order_next) && fm.is_reverse() {
                    if let Some(ref tgt) = fm.target {
                        if !item_fields.contains(tgt) {
                            item_fields.push(tgt.clone());
                        }
                    }
                }
            }
            let parent_fk_field = m.source.parent_fields.keys().next().cloned();
            let order_field = m
                .fields
                .iter()
                .find(|fm| fm.order)
                .and_then(|fm| fm.target.clone());
            let segments: Vec<String> = path.split('.').map(|s| s.to_string()).collect();
            all_nested.push(FlatNested {
                segments,
                mapping: m,
                item_fields,
                parent_fk_field,
                order_field,
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
                order_field: flat.order_field,
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
                    order_field: flat.order_field,
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
    // Exclude order/order_prev/order_next fields from the JSONB object —
    // they are ordering metadata, not data content.
    let order_targets: std::collections::HashSet<&str> = node
        .mapping
        .fields
        .iter()
        .filter(|fm| fm.order || fm.order_prev || fm.order_next)
        .filter_map(|fm| fm.target.as_deref())
        .collect();
    let table_alias = "n.";
    let mut obj_parts: Vec<String> = node
        .item_fields
        .iter()
        .filter(|f| !order_targets.contains(f.as_str()))
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

    // Determine ORDER BY field: use order_field if set, otherwise first item_field.
    let order_by_field = node
        .order_field
        .as_deref()
        .or_else(|| node.item_fields.first().map(|s| s.as_str()))
        .unwrap_or("_src_id");
    let order_rank_field = format!("_order_rank_{order_by_field}");
    let q_order_field = qi(order_by_field);
    let order_expr_leaf = format!(
        "CASE WHEN NULLIF(to_jsonb(n)->>'{order_rank_field}', '')::bigint IS NULL THEN 1 ELSE 0 END, \
         NULLIF(to_jsonb(n)->>'{order_rank_field}', '')::bigint, \
         n.{q_order_field}"
    );
    let order_expr_nested = format!(
        "CASE WHEN NULLIF(to_jsonb(n)->>'{order_rank_field}', '')::bigint IS NULL THEN 1 ELSE 0 END, \
         NULLIF(to_jsonb(n)->>'{order_rank_field}', '')::bigint, \
         n.{q_order_field}"
    );

    if child_results.is_empty() {
        // Leaf node: simple aggregation.
        all_ctes.push(format!(
            "{alias} AS (\n\
               SELECT n.{qgroup} AS _parent_key, \
             COALESCE(jsonb_agg({obj_expr} ORDER BY {order_expr_leaf}), '[]'::jsonb) AS {qsegment}\n\
               FROM {rev_view} AS n\n\
               WHERE n.{qgroup} IS NOT NULL\n\
               GROUP BY n.{qgroup}\n\
             )",
        ));
    } else {
        // Interior node: join child CTEs, then aggregate.
        let mut joins = Vec::new();
        for cr in &child_results {
            // The child CTE groups by _parent_key which corresponds to the
            // child's parent_fk_field. We join it to this node's identity column —
            // which is the first item_field (typically the PK-like field).
            let join_col = node
                .item_fields
                .first()
                .map(|s| s.as_str())
                .unwrap_or("_src_id");
            joins.push(format!(
                "LEFT JOIN {child_alias} ON {child_alias}._parent_key = n.{qjoin}::text",
                child_alias = cr.alias,
                qjoin = qi(join_col),
            ));
        }

        all_ctes.push(format!(
            "{alias} AS (\n\
             SELECT n.{qgroup} AS _parent_key, \
             COALESCE(jsonb_agg({obj_expr} ORDER BY {order_expr_nested}), '[]'::jsonb) AS {qsegment}\n\
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
    let parent_pk_col: String = pk_columns
        .iter()
        .next()
        .copied()
        .unwrap_or("_src_id")
        .to_string();

    // Build the noop detection CASE.
    let mut noop_parts: Vec<String> = Vec::new();
    for fm in &parent.fields {
        if fm.is_reverse() {
            if let Some(src) = fm.source_name() {
                if !pk_columns.contains(src) {
                    noop_parts.push(format!(
                        "p._base->>'{}' IS NOT DISTINCT FROM p.{}::text",
                        sql_escape(src),
                        qi(src)
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
    if parent.is_child() && !parent.is_nested() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse(yaml: &str) -> crate::model::MappingDocument {
        parser::parse_str(yaml).expect("valid test YAML")
    }

    #[test]
    fn simple_noop_detection() {
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
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let source_meta = doc.sources.get("s");
        let sql = render_delta_view("s", &mappings, source_meta).unwrap();
        assert!(
            sql.contains("IS NOT DISTINCT FROM"),
            "noop detection should use IS NOT DISTINCT FROM"
        );
        assert!(
            sql.contains("_base"),
            "noop check should reference _base column"
        );
    }

    #[test]
    fn nested_array_cte_structure() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  order: { fields: { oid: { strategy: identity } } }
  line: { fields: { lid: { strategy: identity }, oref: { strategy: coalesce, references: order } } }
mappings:
  - name: s_orders
    source: s
    target: order
    fields: [{ source: id, target: oid }]
  - name: s_lines
    parent: s_orders
    array: lines
    parent_fields: { pid: id }
    target: line
    fields:
      - { source: lid, target: lid }
      - { source: pid, target: oref, references: s_orders }
"#,
        );
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let source_meta = doc.sources.get("s");
        let sql = render_delta_view("s", &mappings, source_meta).unwrap();
        assert!(
            sql.contains("jsonb_agg"),
            "nested array delta should use jsonb_agg CTE"
        );
        assert!(
            sql.contains("_parent_key"),
            "nested array CTE should include _parent_key"
        );
    }

    #[test]
    fn merged_delta_union() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t1: { fields: { name: { strategy: coalesce } } }
  t2: { fields: { email: { strategy: coalesce } } }
mappings:
  - name: s_t1
    source: s
    target: t1
    fields: [{ source: name, target: name }]
  - name: s_t2
    source: s
    target: t2
    fields: [{ source: email, target: email }]
"#,
        );
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let source_meta = doc.sources.get("s");
        let sql = render_delta_view("s", &mappings, source_meta).unwrap();
        assert!(
            sql.contains("UNION ALL"),
            "multi-target delta should produce UNION ALL of reverse views"
        );
        assert!(
            sql.contains("_rev_s_t1") && sql.contains("_rev_s_t2"),
            "should reference both reverse views"
        );
    }

    #[test]
    fn text_norm_both_sides() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  order: { fields: { oid: { strategy: identity } } }
  line: { fields: { lid: { strategy: identity }, oref: { strategy: coalesce, references: order } } }
mappings:
  - name: s_orders
    source: s
    target: order
    fields: [{ source: id, target: oid }]
  - name: s_lines
    parent: s_orders
    array: lines
    parent_fields: { pid: id }
    target: line
    fields:
      - { source: lid, target: lid }
      - { source: pid, target: oref, references: s_orders }
"#,
        );
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let source_meta = doc.sources.get("s");
        let sql = render_delta_view("s", &mappings, source_meta).unwrap();
        // _osi_text_norm should be applied in noop comparison for nested arrays
        let norm_count = sql.matches("_osi_text_norm").count();
        assert!(
            norm_count >= 2,
            "_osi_text_norm should appear on both sides of noop comparison, found {norm_count}"
        );
    }

    #[test]
    fn written_state_adds_noop_branch() {
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
    written_state: true
    written_noop: true
    fields: [{ source: name, target: name }]
"#,
        );
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let source_meta = doc.sources.get("s");
        let sql = render_delta_view("s", &mappings, source_meta).unwrap();
        // Should LEFT JOIN the _written table.
        assert!(
            sql.contains("LEFT JOIN \"_written_s\""),
            "delta should LEFT JOIN _written table:\n{sql}"
        );
        // Should have _ws._written noop comparison.
        assert!(
            sql.contains("_ws.\"_written\""),
            "delta should reference _ws._written for noop detection:\n{sql}"
        );
        // Should still have _base noop comparison (fast path).
        assert!(
            sql.contains("_base"),
            "delta should keep _base noop as fast path:\n{sql}"
        );
    }

    #[test]
    fn written_state_without_noop_no_comparison() {
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
    written_state: true
    fields: [{ source: name, target: name }]
"#,
        );
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let source_meta = doc.sources.get("s");
        let sql = render_delta_view("s", &mappings, source_meta).unwrap();
        // Should LEFT JOIN the _written table (still needed for delete detection).
        assert!(
            sql.contains("LEFT JOIN \"_written_s\""),
            "delta should LEFT JOIN _written table:\n{sql}"
        );
        // Should NOT have _ws._written noop comparison — written_noop is off.
        assert!(
            !sql.contains("_ws.\"_written\""),
            "delta should NOT reference _ws._written without written_noop:\n{sql}"
        );
    }

    #[test]
    fn written_state_union_all() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t1: { fields: { name: { strategy: coalesce } } }
  t2: { fields: { email: { strategy: coalesce } } }
mappings:
  - name: s_t1
    source: s
    target: t1
    written_state: true
    written_noop: true
    fields: [{ source: name, target: name }]
  - name: s_t2
    source: s
    target: t2
    fields: [{ source: email, target: email }]
"#,
        );
        let mappings: Vec<&_> = doc.mappings.iter().collect();
        let source_meta = doc.sources.get("s");
        let sql = render_delta_view("s", &mappings, source_meta).unwrap();
        // Only s_t1 has written_state — its branch should have LEFT JOIN.
        assert!(
            sql.contains("LEFT JOIN \"_written_s_t1\""),
            "s_t1 branch should LEFT JOIN _written:\n{sql}"
        );
        // s_t2 does NOT have written_state — no LEFT JOIN for it.
        assert!(
            !sql.contains("_written_s_t2"),
            "s_t2 branch should not reference _written:\n{sql}"
        );
    }
}
