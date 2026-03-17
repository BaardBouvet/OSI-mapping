use anyhow::Result;
use std::collections::HashMap;

use crate::model::{Mapping, ParentFieldRef, Source, Target};
use crate::{qi, sql_escape};

/// A parsed segment from a `source_path` expression.
#[derive(Debug, Clone)]
pub(crate) enum PathSegment {
    /// JSON object key: `->>'key'` (leaf) or `->'key'` (intermediate)
    Key(String),
    /// JSON array index: `->>N` (leaf) or `->N` (intermediate)
    Index(i64),
}

/// Parse a `source_path` string into typed segments.
///
/// Handles:
/// - Dot-separated keys: `metadata.tier` → \[Key("metadata"), Key("tier")\]
/// - Bracket-quoted keys: `config.['api.endpoint']` → \[Key("config"), Key("api.endpoint")\]
/// - Array indices: `contacts[0].email` → \[Key("contacts"), Index(0), Key("email")\]
/// - Combined: `data.['x.y'][2].z` → \[Key("data"), Key("x.y"), Index(2), Key("z")\]
pub(crate) fn parse_path_segments(path: &str) -> Vec<PathSegment> {
    let mut segments = Vec::new();
    let chars: Vec<char> = path.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Skip leading dot separator.
        if chars[i] == '.' {
            i += 1;
            continue;
        }

        if chars[i] == '[' {
            // Bracket expression: ['key'] or [N].
            let close = chars[i..]
                .iter()
                .position(|&c| c == ']')
                .map(|p| i + p)
                .unwrap_or(chars.len());
            let inner: String = chars[i + 1..close].iter().collect();
            i = close + 1;

            // Strip single quotes: ['key'] → key
            let inner = inner
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(&inner);

            if let Ok(n) = inner.parse::<i64>() {
                segments.push(PathSegment::Index(n));
            } else {
                segments.push(PathSegment::Key(inner.to_string()));
            }
        } else {
            // Bare key — read until next `.` or `[`.
            let start = i;
            while i < chars.len() && chars[i] != '.' && chars[i] != '[' {
                i += 1;
            }
            let key: String = chars[start..i].iter().collect();
            if !key.is_empty() {
                segments.push(PathSegment::Key(key));
            }
        }
    }

    segments
}

/// Build the SQL operator+operand for one `PathSegment`.
fn segment_sql(seg: &PathSegment, is_last: bool) -> String {
    let arrow = if is_last { "->>" } else { "->" };
    match seg {
        PathSegment::Key(k) => format!("{arrow}'{k}'"),
        PathSegment::Index(n) => format!("{arrow}{n}"),
    }
}

/// Generate SQL to extract a value from a JSONB column/expression via a path.
///
/// Root context (`base` is None):
///   `"metadata.tier"` → `"metadata"->>'tier'`
///   `"contacts[0].email"` → `"contacts"->0->>'email'`
///
/// Nested context (`base` is Some, e.g. `"item.value"`):
///   `"meta.tier"` → `(item.value->'meta'->>'tier')`
///   All segments are JSON navigation (no column quoting on the first).
pub fn json_path_expr(source_path: &str) -> String {
    json_path_expr_with_base(source_path, None)
}

fn json_path_expr_with_base(source_path: &str, base: Option<&str>) -> String {
    let segments = parse_path_segments(source_path);

    let (root, keys) = if let Some(b) = base {
        (b.to_string(), &segments[..])
    } else {
        match &segments[0] {
            PathSegment::Key(k) => (qi(k), &segments[1..]),
            PathSegment::Index(n) => (qi(&n.to_string()), &segments[1..]),
        }
    };

    if keys.is_empty() {
        return root;
    }

    let mut expr = root;
    for (i, seg) in keys.iter().enumerate() {
        let is_last = i == keys.len() - 1;
        expr = format!("{expr}{}", segment_sql(seg, is_last));
    }

    if base.is_some() {
        format!("({expr})")
    } else {
        expr
    }
}

/// Resolve a source field name to its SQL expression for nested array mappings.
///
/// - Parent field aliases → root table column (single-segment path) or
///   intermediate JSONB extraction (multi-segment path).
/// - Regular fields → `(item.value->>'field_name')` (JSONB item extraction).
/// - When no path is set → quoted column name as before.
fn resolve_nested_source(
    source_name: &str,
    parent_field_exprs: &HashMap<String, String>,
    has_path: bool,
) -> String {
    if let Some(expr) = parent_field_exprs.get(source_name) {
        expr.clone()
    } else if has_path {
        format!("(item.value->>'{source_name}')")
    } else {
        qi(source_name)
    }
}

/// Render a CREATE VIEW statement for a forward mapping.
///
/// Produces: `CREATE OR REPLACE VIEW _fwd_{mapping_name} AS SELECT ...`
///
/// `nested_base_cols` lists source columns (e.g., JSONB array columns) from
/// nested-path child mappings that should be included in `_base` for noop
/// detection in the delta view.
pub fn render_forward_view(
    mapping: &Mapping,
    source_meta: Option<&Source>,
    target: Option<&Target>,
    nested_base_cols: &[String],
) -> Result<String> {
    let view_name = qi(&format!("_fwd_{}", mapping.name));
    let body = render_forward_body(mapping, source_meta, target, nested_base_cols)?;
    Ok(format!(
        "-- Forward: {name}\nCREATE OR REPLACE VIEW {view_name} AS\n{body};\n",
        name = mapping.name,
    ))
}

/// Render the forward SELECT body for a mapping (no CREATE VIEW wrapper).
///
/// Returns the SQL fragment: `SELECT ... FROM source [LEFT JOIN ...] [WHERE ...]`
pub fn render_forward_body(
    mapping: &Mapping,
    source_meta: Option<&Source>,
    target: Option<&Target>,
    nested_base_cols: &[String],
) -> Result<String> {
    let source = qi(source_meta
        .map(|s| s.table_name(&mapping.source.dataset))
        .unwrap_or(&mapping.source.dataset));

    let has_path = mapping.source.path.is_some();
    let path_depth = mapping
        .source
        .path
        .as_ref()
        .map(|p| p.split('.').count())
        .unwrap_or(0);

    // Build parent field alias → SQL expression map for nested sources.
    let mut parent_field_exprs: HashMap<String, String> = HashMap::new();
    if has_path {
        let path_segments: Vec<&str> = mapping
            .source
            .path
            .as_ref()
            .map(|p| p.split('.').collect())
            .unwrap_or_default();
        for (alias, pref) in &mapping.source.parent_fields {
            let (col, qualified_path) = match pref {
                ParentFieldRef::Simple(c) => (c.as_str(), None),
                ParentFieldRef::Qualified { field, path } => (field.as_str(), path.as_deref()),
            };
            let expr = match qualified_path {
                Some(qpath) => {
                    // Qualified ref: find which nesting level the path refers to.
                    // The path names the array column at a specific level. We need
                    // the row/item BEFORE that array was unpacked.
                    // E.g. source.path = "modules.features", qualified path = "modules"
                    //   → segment index 0 → root table level → qi(col)
                    // E.g. source.path = "a.b.c", qualified path = "a.b"
                    //   → segment index 1 → _nest_0 level
                    let qsegments: Vec<&str> = qpath.split('.').collect();
                    let last_q = qsegments.last().copied().unwrap_or("");
                    // Find which path segment the qualified path's last component matches.
                    let level = path_segments.iter().position(|&s| s == last_q).unwrap_or(0);
                    if level == 0 {
                        // Before the first array: root table column.
                        qi(col)
                    } else {
                        // Before segment N: the item at segment N-1.
                        format!("(_nest_{}.value->>'{col}')", level - 1)
                    }
                }
                None => {
                    // Simple ref: immediate parent level.
                    if path_depth <= 1 {
                        qi(col)
                    } else {
                        format!("(_nest_{}.value->>'{col}')", path_depth - 2)
                    }
                }
            };
            parent_field_exprs.insert(alias.clone(), expr);
        }
    }

    let mut cols: Vec<String> = Vec::new();

    // Source row identifier — declared PK when present, _row_id fallback otherwise.
    let src_id_expr = source_meta
        .map(|s| s.primary_key.src_id_expr(None))
        .unwrap_or_else(|| "_row_id::text".to_string());
    cols.push(format!("{src_id_expr} AS _src_id"));
    cols.push(format!("'{}'::text AS _mapping", mapping.name));

    // Cluster identity — always emitted for UNION ALL compatibility.
    if let Some(ref cf) = mapping.cluster_field {
        // cluster_field: use the source column directly, fallback to md5 singleton.
        let qcf = qi(cf);
        let fallback = format!("md5('{}' || ':' || {})", mapping.name, src_id_expr);
        cols.push(format!("COALESCE({qcf}, {fallback}) AS _cluster_id"));
    } else if let Some(ref cm) = mapping.cluster_members {
        // cluster_members: LEFT JOIN happens in FROM clause (handled below).
        // _cluster_id comes from the join, fallback to md5 singleton.
        let qcm_cluster = qi(&cm.cluster_id);
        let fallback = format!("md5('{}' || ':' || {})", mapping.name, src_id_expr);
        cols.push(format!(
            "COALESCE(_cm.{qcm_cluster}, {fallback}) AS _cluster_id"
        ));
    } else {
        // No cluster config: emit NULL placeholder for UNION ALL compatibility.
        cols.push("NULL::text AS _cluster_id".to_string());
    }

    // Mapping-level priority (always present, NULL when unset)
    cols.push(match mapping.priority {
        Some(p) => format!("{p} AS _priority"),
        None => "NULL::int AS _priority".into(),
    });

    // Mapping-level last_modified (always present, NULL when unset)
    cols.push(match &mapping.last_modified {
        Some(ts) => {
            if let Some(field) = ts.field_name() {
                format!("{} AS _last_modified", qi(field))
            } else if let Some(expr) = ts.expression() {
                format!("({expr}) AS _last_modified")
            } else {
                "NULL::text AS _last_modified".into()
            }
        }
        None => "NULL::text AS _last_modified".into(),
    });

    // If target is known, emit ALL target fields in target-definition order
    // (NULL for fields this mapping doesn't contribute to).
    if let Some(target) = target {
        for (fname, fdef) in &target.fields {
            let qfname = qi(fname);
            // Use declared type if available, fall back to text.
            let cast_type = fdef.field_type().unwrap_or("text");
            let null_type = fdef.field_type().unwrap_or("text");
            let fm = mapping
                .fields
                .iter()
                .find(|fm| fm.is_forward() && fm.target.as_deref() == Some(fname.as_str()));

            if let Some(fm) = fm {
                let expr = if let Some(ref e) = fm.expression {
                    e.clone()
                } else if let Some(ref sp) = fm.source_path {
                    if has_path {
                        // Nested context: all segments are JSON keys under item.value
                        json_path_expr_with_base(sp, Some("item.value"))
                    } else {
                        json_path_expr(sp)
                    }
                } else if let Some(ref src) = fm.source {
                    resolve_nested_source(src, &parent_field_exprs, has_path)
                } else {
                    "NULL".into()
                };
                // Cast to target field type for UNION ALL compatibility across mappings.
                cols.push(format!("{expr}::{cast_type} AS {qfname}"));

                // Per-field priority (always present, NULL when unset)
                cols.push(match fm.priority {
                    Some(p) => format!("{p} AS {}", qi(&format!("_priority_{fname}"))),
                    None => format!("NULL::int AS {}", qi(&format!("_priority_{fname}"))),
                });

                // Per-field timestamp (always present, NULL when unset)
                cols.push(match &fm.last_modified {
                    Some(ts) => {
                        if let Some(field) = ts.field_name() {
                            format!("{} AS {}", qi(field), qi(&format!("_ts_{fname}")))
                        } else if let Some(expr) = ts.expression() {
                            format!("({expr}) AS {}", qi(&format!("_ts_{fname}")))
                        } else {
                            format!("NULL::text AS {}", qi(&format!("_ts_{fname}")))
                        }
                    }
                    None => format!("NULL::text AS {}", qi(&format!("_ts_{fname}"))),
                });
            } else {
                // Not mapped by this mapping — emit NULL placeholders
                cols.push(format!("NULL::{null_type} AS {qfname}"));
                cols.push(format!(
                    "NULL::int AS {}",
                    qi(&format!("_priority_{fname}"))
                ));
                cols.push(format!("NULL::text AS {}", qi(&format!("_ts_{fname}"))));
            }
        }
    } else {
        // External target: emit only mapped fields (no normalization possible)
        for fm in &mapping.fields {
            if !fm.is_forward() {
                continue;
            }
            if let Some(ref tgt) = fm.target {
                let expr = if let Some(ref e) = fm.expression {
                    e.clone()
                } else if let Some(ref sp) = fm.source_path {
                    if has_path {
                        json_path_expr_with_base(sp, Some("item.value"))
                    } else {
                        json_path_expr(sp)
                    }
                } else if let Some(ref src) = fm.source {
                    resolve_nested_source(src, &parent_field_exprs, has_path)
                } else {
                    continue;
                };
                cols.push(format!("{expr}::text AS {}", qi(tgt)));
            }
        }
    }

    // _base: JSONB snapshot of raw source columns involved in field mappings.
    // Built here (pre-expression) so it flows through identity via SELECT *.
    // Includes reverse_only fields with source columns for noop detection.
    {
        let mut base_parts: Vec<String> = Vec::new();
        for fm in &mapping.fields {
            if fm.is_forward() || fm.is_reverse() {
                if let Some(ref sp) = fm.source_path {
                    let resolved = if has_path {
                        json_path_expr_with_base(sp, Some("item.value"))
                    } else {
                        json_path_expr(sp)
                    };
                    let part = format!("'{}', {resolved}", sql_escape(sp));
                    if !base_parts.contains(&part) {
                        base_parts.push(part);
                    }
                } else if let Some(ref src) = fm.source {
                    let resolved = resolve_nested_source(src, &parent_field_exprs, has_path);
                    let part = format!("'{src}', {resolved}");
                    if !base_parts.contains(&part) {
                        base_parts.push(part);
                    }
                }
            }
        }
        // Include nested array source columns for noop detection.
        for col in nested_base_cols {
            let qcol = qi(col);
            let part = format!("'{col}', {qcol}");
            if !base_parts.contains(&part) {
                base_parts.push(part);
            }
        }
        if base_parts.is_empty() {
            cols.push("NULL::jsonb AS _base".to_string());
        } else {
            cols.push(format!(
                "jsonb_build_object({}) AS _base",
                base_parts.join(", ")
            ));
        }
    }

    let mut sql = format!(
        "SELECT\n  {columns}\nFROM {source}",
        columns = cols.join(",\n  "),
    );

    // LEFT JOIN cluster_members table when declared.
    if let Some(ref cm) = mapping.cluster_members {
        let qcm_table = qi(&cm.table_name(&mapping.name));
        let qcm_src_key = qi(&cm.source_key);
        sql.push_str(&format!(
            "\nLEFT JOIN {qcm_table} AS _cm ON _cm.{qcm_src_key} = {src_id_expr}"
        ));
    }

    // Nested arrays via LATERAL jsonb_array_elements
    if let Some(ref path) = mapping.source.path {
        let segments: Vec<&str> = path.split('.').collect();
        for (i, seg) in segments.iter().enumerate() {
            let alias = if i == segments.len() - 1 {
                "item".to_string()
            } else {
                format!("_nest_{i}")
            };
            let parent = if i == 0 {
                qi(seg)
            } else {
                format!("_nest_{}.value->'{seg}'", i - 1)
            };
            sql.push_str(&format!(
                "\nCROSS JOIN LATERAL jsonb_array_elements({parent}) AS {alias}"
            ));
        }
    }

    if let Some(ref filter) = mapping.filter {
        sql.push_str(&format!("\nWHERE {filter}"));
    }

    Ok(sql)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn hello_world_forward_views_have_matching_columns() {
        let yaml = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("examples/hello-world/mapping.yaml"),
        )
        .unwrap();
        let doc = parser::parse_str(&yaml).unwrap();
        let target = doc.targets.get("contact").unwrap();

        let sqls: Vec<String> = doc
            .mappings
            .iter()
            .map(|m| render_forward_body(m, None, Some(target), &[]).unwrap())
            .collect();

        // Both views must have identical column sets
        for sql in &sqls {
            assert!(
                sql.contains("_row_id::text AS _src_id") || sql.contains("_row_id AS _src_id"),
                "missing _src_id"
            );
            assert!(sql.contains("AS _mapping"), "missing _mapping");
            assert!(
                sql.contains("AS _priority\n") || sql.contains("AS _priority,"),
                "missing _priority"
            );
            assert!(sql.contains("AS _last_modified"), "missing _last_modified");
            assert!(sql.contains("AS \"email\""), "missing email");
            assert!(sql.contains("AS \"name\""), "missing name");
            assert!(
                sql.contains("AS \"_priority_email\""),
                "missing _priority_email"
            );
            assert!(
                sql.contains("AS \"_priority_name\""),
                "missing _priority_name"
            );
            assert!(sql.contains("AS \"_ts_email\""), "missing _ts_email");
            assert!(sql.contains("AS \"_ts_name\""), "missing _ts_name");
        }
    }
}
