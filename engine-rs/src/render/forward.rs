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
    normalize_fields: &HashMap<String, String>,
) -> Result<String> {
    let view_name = qi(&format!("_fwd_{}", mapping.name));
    let body = render_forward_body(
        mapping,
        source_meta,
        target,
        nested_base_cols,
        normalize_fields,
    )?;
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
    normalize_fields: &HashMap<String, String>,
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
    let cluster_id_expr = if let Some(ref cf) = mapping.cluster_field {
        let qcf = qi(cf);
        let fallback = format!("md5('{}' || ':' || {})", mapping.name, src_id_expr);
        format!("COALESCE({qcf}, {fallback})")
    } else if let Some(ref cm) = mapping.cluster_members {
        let qcm_cluster = qi(&cm.cluster_id);
        let fallback = format!("md5('{}' || ':' || {})", mapping.name, src_id_expr);
        format!("COALESCE(_cm.{qcm_cluster}, {fallback})")
    } else {
        "NULL::text".to_string()
    };
    cols.push(format!("{cluster_id_expr} AS _cluster_id"));

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

    // Soft-delete detection: when the mapping declares soft_delete, non-identity
    // fields are NULLed in the forward view so soft-deleted rows cannot win
    // field resolution.  Identity fields keep their values for entity linking.
    let soft_delete_detect = mapping.soft_delete.as_ref().map(|sd| {
        if has_path {
            sd.detection_expr_with_base(Some("item.value"))
        } else {
            sd.detection_expr()
        }
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
                let expr = if fm.order {
                    // Tier 1 ordinal ordering: zero-padded position from WITH ORDINALITY
                    "lpad((item.idx - 1)::text, 10, '0')".to_string()
                } else if fm.order_prev || fm.order_next {
                    // Tier 2 linked-list CRDT: LAG/LEAD over identity fields
                    let window_fn = if fm.order_prev { "LAG" } else { "LEAD" };
                    // Find identity fields on the target for the neighbor reference
                    let identity_fields: Vec<&str> = target
                        .fields
                        .iter()
                        .filter(|(_, fd)| fd.strategy() == crate::model::Strategy::Identity)
                        .map(|(n, _)| n.as_str())
                        .collect();
                    let value_expr = if identity_fields.len() == 1 {
                        qi(identity_fields[0])
                    } else {
                        // Composite identity: JSONB object of neighbor's identity
                        let parts: Vec<String> = identity_fields
                            .iter()
                            .map(|f| format!("'{}', {}", crate::sql_escape(f), qi(f)))
                            .collect();
                        format!("jsonb_build_object({})", parts.join(", "))
                    };
                    // Partition by parent field aliases (or parent PK)
                    let partition_cols: Vec<String> =
                        parent_field_exprs.values().cloned().collect::<Vec<_>>();
                    let partition = if partition_cols.is_empty() {
                        String::new()
                    } else {
                        format!("PARTITION BY {} ", partition_cols.join(", "))
                    };
                    format!("{window_fn}({value_expr}) OVER ({partition}ORDER BY item.idx)")
                } else if let Some(ref e) = fm.expression {
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
                // Soft-deleted non-identity fields → NULL so they lose resolution.
                let is_identity = fdef.strategy() == crate::model::Strategy::Identity;
                if let (Some(detect), false) = (&soft_delete_detect, is_identity) {
                    cols.push(format!(
                        "CASE WHEN ({detect}) THEN NULL::{cast_type} ELSE {expr}::{cast_type} END AS {qfname}"
                    ));
                } else {
                    cols.push(format!("{expr}::{cast_type} AS {qfname}"));
                }

                // Per-field priority (always present, NULL when unset)
                // Soft-deleted non-identity fields → NULL priority so they cannot win.
                if let (Some(detect), false) = (&soft_delete_detect, is_identity) {
                    cols.push(match fm.priority {
                        Some(p) => format!(
                            "CASE WHEN ({detect}) THEN NULL::int ELSE {p} END AS {}",
                            qi(&format!("_priority_{fname}"))
                        ),
                        None => format!("NULL::int AS {}", qi(&format!("_priority_{fname}"))),
                    });
                } else {
                    cols.push(match fm.priority {
                        Some(p) => format!("{p} AS {}", qi(&format!("_priority_{fname}"))),
                        None => format!("NULL::int AS {}", qi(&format!("_priority_{fname}"))),
                    });
                }

                // Per-field timestamp (always present, NULL when unset)
                // When derive_timestamps is active and no explicit last_modified,
                // generate CASE that compares against written JSONB.
                // Soft-deleted non-identity fields → NULL timestamp.
                let derive_ts = mapping.derive_timestamps
                    && mapping.written_state.is_some()
                    && fm.last_modified.is_none();
                let source_col_for_ts = if derive_ts {
                    fm.source.as_deref().or(fm.source_path.as_deref())
                } else {
                    None
                };

                let ts_alias = qi(&format!("_ts_{fname}"));
                let ts_col = if let Some(ts) = &fm.last_modified {
                    if let Some(field) = ts.field_name() {
                        format!("{} AS {ts_alias}", qi(field))
                    } else if let Some(expr) = ts.expression() {
                        format!("({expr}) AS {ts_alias}")
                    } else {
                        format!("NULL::text AS {ts_alias}")
                    }
                } else if let Some(src_col) = source_col_for_ts {
                    let ws = mapping.written_state.as_ref().unwrap();
                    let wcol = qi(&ws.written);
                    let wat = qi(&ws.written_at);
                    let wts = qi(&ws.written_ts);
                    let src_esc = sql_escape(src_col);
                    // Build the mapping-level timestamp expression (if any).
                    // Used as the primary timestamp for changed fields, with
                    // _written_at as fallback.
                    let mapping_ts = mapping.last_modified.as_ref().and_then(|ts| {
                        if let Some(f) = ts.field_name() {
                            Some(qi(f))
                        } else {
                            ts.expression().map(|e| format!("({e})"))
                        }
                    });
                    let changed_expr = match &mapping_ts {
                        Some(ts) => format!("COALESCE({ts}, _ws.{wat}::text)"),
                        None => format!("_ws.{wat}::text"),
                    };
                    let bootstrap_expr = match &mapping_ts {
                        Some(ts) => ts.clone(),
                        None => "NULL::text".to_string(),
                    };
                    // Unchanged fields carry forward their per-field timestamp
                    // from _written_ts.  Changed fields use the source's own
                    // timestamp (if available), falling back to _written_at.
                    // Bootstrap (no _written_ts entry) → source timestamp or NULL.
                    format!(
                        "CASE \
                         WHEN {expr}::text IS NOT DISTINCT FROM _ws.{wcol}->>'{src_esc}' \
                         THEN _ws.{wts}->>'{src_esc}' \
                         WHEN _ws.{wts}->>'{src_esc}' IS NOT NULL \
                         THEN {changed_expr} \
                         ELSE {bootstrap_expr} \
                         END AS {ts_alias}",
                    )
                } else {
                    format!("NULL::text AS {ts_alias}")
                };
                // Wrap in soft-delete guard for non-identity fields.
                if let (Some(detect), false) = (&soft_delete_detect, is_identity) {
                    // Replace " AS <alias>" with CASE wrapping.
                    // The ts_col already ends with " AS <alias>", so we wrap the
                    // expression part.  Simplest: if already NULL, keep it;
                    // otherwise wrap.
                    let null_ts = format!("NULL::text AS {ts_alias}");
                    if ts_col == null_ts {
                        cols.push(ts_col);
                    } else {
                        // Strip trailing " AS <alias>" to get the bare expression.
                        let suffix = format!(" AS {ts_alias}");
                        let bare = ts_col.strip_suffix(&suffix).unwrap_or(&ts_col);
                        cols.push(format!(
                            "CASE WHEN ({detect}) THEN NULL::text ELSE ({bare})::text END AS {ts_alias}"
                        ));
                    }
                } else {
                    cols.push(ts_col);
                }

                // Echo-aware normalize columns (Phase 2 precision-loss).
                // When any mapping declares `normalize` for this target field,
                // all forward views emit a canonical normalized value so the
                // resolution view can detect and suppress echo values.
                if let Some(canonical_norm) = normalize_fields.get(fname) {
                    let norm_expr = canonical_norm.replace("%s", &format!("({expr})"));
                    let norm_alias = qi(&format!("_normalize_{fname}"));
                    let has_norm_alias = qi(&format!("_has_normalize_{fname}"));
                    let has_own = fm.normalize.is_some();
                    if let (Some(detect), false) = (&soft_delete_detect, is_identity) {
                        cols.push(format!(
                            "CASE WHEN ({detect}) THEN NULL::text ELSE {norm_expr} END AS {norm_alias}"
                        ));
                        cols.push(format!(
                            "CASE WHEN ({detect}) THEN NULL::boolean ELSE {has_own} END AS {has_norm_alias}"
                        ));
                    } else {
                        cols.push(format!("{norm_expr} AS {norm_alias}"));
                        cols.push(format!("{has_own} AS {has_norm_alias}"));
                    }
                }
            } else {
                // Not mapped by this mapping — emit NULL placeholders.
                // Exception: soft_delete.target auto-injects the detection
                // expression as if an implicit field mapping existed.
                let sd_target_match = mapping
                    .soft_delete
                    .as_ref()
                    .and_then(|sd| sd.target.as_deref())
                    .is_some_and(|t| t == fname);
                if sd_target_match {
                    let detect = soft_delete_detect
                        .as_deref()
                        .expect("soft_delete.target requires soft_delete detection");
                    cols.push(format!("({detect})::{cast_type} AS {qfname}"));
                } else {
                    cols.push(format!("NULL::{null_type} AS {qfname}"));
                }
                cols.push(format!(
                    "NULL::int AS {}",
                    qi(&format!("_priority_{fname}"))
                ));
                cols.push(format!("NULL::text AS {}", qi(&format!("_ts_{fname}"))));

                // Echo-aware NULL placeholders for normalize columns.
                if normalize_fields.contains_key(fname) {
                    cols.push(format!(
                        "NULL::text AS {}",
                        qi(&format!("_normalize_{fname}"))
                    ));
                    cols.push(format!(
                        "NULL::boolean AS {}",
                        qi(&format!("_has_normalize_{fname}"))
                    ));
                }
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
        // Include passthrough columns in _base for round-trip preservation.
        for col in mapping.effective_passthrough() {
            let resolved = resolve_nested_source(col, &parent_field_exprs, has_path);
            let part = format!("'{col}', {resolved}");
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

    // LEFT JOIN written state table for derive_timestamps.
    if mapping.derive_timestamps {
        if let Some(ref ws) = mapping.written_state {
            let ws_table = qi(&ws.table_name(&mapping.name));
            let ws_cluster = qi(&ws.cluster_id);
            sql.push_str(&format!(
                "\nLEFT JOIN {ws_table} AS _ws ON _ws.{ws_cluster} = {cluster_id_expr}"
            ));
        }
    }

    // Nested arrays via LATERAL jsonb_array_elements
    if let Some(ref path) = mapping.source.path {
        let has_ordering = mapping
            .fields
            .iter()
            .any(|f| f.order || f.order_prev || f.order_next);
        let segments: Vec<&str> = path.split('.').collect();
        for (i, seg) in segments.iter().enumerate() {
            let is_last = i == segments.len() - 1;
            let alias = if is_last {
                "item".to_string()
            } else {
                format!("_nest_{i}")
            };
            let parent = if i == 0 {
                qi(seg)
            } else {
                format!("_nest_{}.value->'{seg}'", i - 1)
            };
            if is_last && has_ordering {
                // WITH ORDINALITY provides item.idx for order fields
                sql.push_str(&format!(
                    "\nCROSS JOIN LATERAL jsonb_array_elements({parent}) WITH ORDINALITY AS {alias}(value, idx)"
                ));
            } else {
                sql.push_str(&format!(
                    "\nCROSS JOIN LATERAL jsonb_array_elements({parent}) AS {alias}"
                ));
            }
        }
    }

    if let Some(ref filter) = mapping.filter {
        sql.push_str(&format!("\nWHERE {filter}"));
    }

    // derive_tombstones: UNION ALL that synthesizes rows for absent entities.
    // Entities in cluster_members but not in the source get the target field
    // set to TRUE and all other fields NULL, so resolution propagates the
    // deletion via bool_or.
    if let (Some(ref dt_field), Some(ref cm), Some(source_meta)) = (
        &mapping.derive_tombstones,
        &mapping.cluster_members,
        source_meta,
    ) {
        let cm_table = qi(&cm.table_name(&mapping.name));
        let cm_src_key = qi(&cm.source_key);
        let cm_cluster = qi(&cm.cluster_id);
        let source_table = qi(source_meta.table_name(&mapping.source.dataset));

        // Build column list matching the main SELECT for UNION ALL compatibility.
        // Use a suffixed _mapping so the synthetic row contributes to resolution
        // but does NOT appear in this mapping's reverse view (which filters on
        // _mapping = 'name').  This prevents the delta from sending a redundant
        // delete back to the source that already hard-deleted the entity.
        let mut dt_cols: Vec<String> = Vec::new();
        dt_cols.push(format!("_dt_cm.{cm_src_key} AS _src_id"));
        dt_cols.push(format!("'{}:tombstone'::text AS _mapping", mapping.name));
        dt_cols.push(format!("_dt_cm.{cm_cluster} AS _cluster_id"));
        dt_cols.push("NULL::int AS _priority".into());
        dt_cols.push("NULL::text AS _last_modified".into());

        if let Some(target) = target {
            for (fname, fdef) in &target.fields {
                let qfname = qi(fname);
                let null_type = fdef.field_type().unwrap_or("text");
                let cast_type = fdef.field_type().unwrap_or("text");
                if fname == dt_field {
                    dt_cols.push(format!("TRUE::{cast_type} AS {qfname}"));
                } else {
                    dt_cols.push(format!("NULL::{null_type} AS {qfname}"));
                }
                dt_cols.push(format!(
                    "NULL::int AS {}",
                    qi(&format!("_priority_{fname}"))
                ));
                dt_cols.push(format!("NULL::text AS {}", qi(&format!("_ts_{fname}"))));
                if normalize_fields.contains_key(fname) {
                    dt_cols.push(format!(
                        "NULL::text AS {}",
                        qi(&format!("_normalize_{fname}"))
                    ));
                    dt_cols.push(format!(
                        "NULL::boolean AS {}",
                        qi(&format!("_has_normalize_{fname}"))
                    ));
                }
            }
        }
        dt_cols.push("NULL::jsonb AS _base".into());

        let pk_match = source_meta
            .primary_key
            .src_id_match_expr("_dt_src", &format!("_dt_cm.{cm_src_key}"));

        sql.push_str(&format!(
            "\nUNION ALL\nSELECT\n  {dt_select}\nFROM {cm_table} AS _dt_cm\nLEFT JOIN {source_table} AS _dt_src ON {pk_match}\nWHERE {absent}",
            dt_select = dt_cols.join(",\n  "),
            absent = source_meta.primary_key.src_missing_predicate(Some("_dt_src")),
        ));
    }

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
            .map(|m| render_forward_body(m, None, Some(target), &[], &HashMap::new()).unwrap())
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

    #[test]
    fn nested_array_lateral_join() {
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
        let m = &doc.mappings[1];
        let target = doc.targets.get("line").unwrap();
        let sql = render_forward_body(m, None, Some(target), &[], &HashMap::new()).unwrap();
        assert!(
            sql.contains("jsonb_array_elements"),
            "should have LATERAL jsonb_array_elements for nested array"
        );
        assert!(
            sql.contains("AS item"),
            "should alias the array expansion as item"
        );
    }

    #[test]
    fn source_path_extraction() {
        let expr = json_path_expr("metadata.tier");
        assert!(
            expr.contains("->>"),
            "source_path should produce JSON extraction operator"
        );
        assert!(
            expr.contains("metadata"),
            "should reference the column name"
        );
    }

    #[test]
    fn expression_passthrough() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t: { fields: { phone: { strategy: expression, expression: "max(phone)" } } }
mappings:
  - name: s
    source: s
    target: t
    fields:
      - { source: phone_raw, target: phone, expression: "regexp_replace(phone_raw, '[^0-9]', '')" }
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let sql = render_forward_body(&doc.mappings[0], None, Some(target), &[], &HashMap::new())
            .unwrap();
        assert!(
            sql.contains("regexp_replace"),
            "expression should appear in forward SQL"
        );
    }

    #[test]
    fn filter_in_where() {
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
    filter: "status = 'active'"
    fields:
      - { source: name, target: name }
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let sql = render_forward_body(&doc.mappings[0], None, Some(target), &[], &HashMap::new())
            .unwrap();
        assert!(
            sql.contains("WHERE status = 'active'"),
            "filter should appear as WHERE clause"
        );
    }

    #[test]
    fn parent_field_promoted() {
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
    parent_fields: { parent_id: id }
    target: line
    fields:
      - { source: lid, target: lid }
      - { source: parent_id, target: oref, references: s_orders }
"#,
        );
        let m = &doc.mappings[1];
        let target = doc.targets.get("line").unwrap();
        let sql = render_forward_body(m, None, Some(target), &[], &HashMap::new()).unwrap();
        // parent_id is a parent_field alias — should resolve to root column "id"
        assert!(
            sql.contains("\"id\""),
            "parent field should resolve to root column reference"
        );
    }

    #[test]
    fn base_includes_source_columns() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  t: { fields: { email: { strategy: identity }, name: { strategy: coalesce } } }
mappings:
  - name: s
    source: s
    target: t
    fields:
      - { source: email, target: email }
      - { source: name, target: name }
"#,
        );
        let target = doc.targets.get("t").unwrap();
        let sql = render_forward_body(&doc.mappings[0], None, Some(target), &[], &HashMap::new())
            .unwrap();
        assert!(
            sql.contains("jsonb_build_object(")
                && sql.contains("'email'")
                && sql.contains("'name'"),
            "_base should include all mapped source columns"
        );
    }

    #[test]
    fn order_true_emits_with_ordinality_and_lpad() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  item:
    fields:
      parent_id: { strategy: identity }
      value: { strategy: identity }
      item_order: { strategy: coalesce }
mappings:
  - name: parent
    source: s
    target: item
    fields:
      - { source: id, target: parent_id }
  - name: child
    source: s
    parent: parent
    array: items
    parent_fields:
      parent_id: id
    target: item
    fields:
      - { source: parent_id, target: parent_id, references: parent }
      - { target: item_order, order: true }
      - { source: value, target: value }
"#,
        );
        let target = doc.targets.get("item").unwrap();
        let sql = render_forward_body(&doc.mappings[1], None, Some(target), &[], &HashMap::new())
            .unwrap();
        assert!(
            sql.contains("WITH ORDINALITY AS item(value, idx)"),
            "should emit WITH ORDINALITY: {sql}"
        );
        assert!(
            sql.contains("lpad((item.idx - 1)::text, 10, '0')"),
            "should emit lpad ordinal expression: {sql}"
        );
    }

    #[test]
    fn order_prev_next_emit_lag_lead() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  step:
    fields:
      parent_id: { strategy: identity }
      instruction: { strategy: identity }
      step_order: { strategy: coalesce }
      step_prev: { strategy: coalesce }
      step_next: { strategy: coalesce }
mappings:
  - name: parent
    source: s
    target: step
    fields:
      - { source: id, target: parent_id }
  - name: child
    source: s
    parent: parent
    array: steps
    parent_fields:
      parent_id: id
    target: step
    fields:
      - { source: parent_id, target: parent_id, references: parent }
      - { target: step_order, order: true }
      - { target: step_prev, order_prev: true, order_next: false }
      - { target: step_next, order_prev: false, order_next: true }
      - { source: instruction, target: instruction }
"#,
        );
        let target = doc.targets.get("step").unwrap();
        let sql = render_forward_body(&doc.mappings[1], None, Some(target), &[], &HashMap::new())
            .unwrap();
        assert!(
            sql.contains("LAG(") && sql.contains("OVER ("),
            "should emit LAG window function: {sql}"
        );
        assert!(
            sql.contains("LEAD(") && sql.contains("ORDER BY item.idx)"),
            "should emit LEAD window function: {sql}"
        );
    }

    #[test]
    fn soft_delete_nulls_non_identity_fields() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  crm: { primary_key: id }
targets:
  contact:
    fields:
      email: { strategy: identity }
      name: { strategy: coalesce }
mappings:
  - name: crm_contact
    source: crm
    target: contact
    soft_delete: deleted_at
    fields:
      - { source: email, target: email }
      - { source: full_name, target: name, priority: 10 }
"#,
        );
        let target = doc.targets.get("contact").unwrap();
        let sql = render_forward_body(&doc.mappings[0], None, Some(target), &[], &HashMap::new())
            .unwrap();

        // Identity field "email" must NOT be wrapped (entities still link).
        assert!(
            sql.contains("\"email\"::text AS \"email\""),
            "identity field should be projected without CASE: {sql}"
        );
        // Non-identity field "name" must be wrapped in soft-delete CASE.
        assert!(
            sql.contains(
                "CASE WHEN (\"deleted_at\" IS NOT NULL) THEN NULL::text ELSE \"full_name\"::text END AS \"name\""
            ),
            "non-identity field should be wrapped in soft-delete CASE: {sql}"
        );
        // Priority for non-identity "name" must also be NULLed when soft-deleted.
        assert!(
            sql.contains(
                "CASE WHEN (\"deleted_at\" IS NOT NULL) THEN NULL::int ELSE 10 END AS \"_priority_name\""
            ),
            "non-identity priority should be NULLed when soft-deleted: {sql}"
        );
        // Timestamp for non-identity "name" — no explicit ts so it's already NULL (no wrapping needed).
        assert!(
            sql.contains("NULL::text AS \"_ts_name\""),
            "NULL timestamp does not need wrapping: {sql}"
        );
        // Identity "email" priority should NOT be wrapped.
        assert!(
            sql.contains("NULL::int AS \"_priority_email\""),
            "identity priority should not be wrapped: {sql}"
        );
    }

    #[test]
    fn soft_delete_target_emits_detection_as_field() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  crm: { primary_key: id }
targets:
  customer:
    fields:
      email: { strategy: identity }
      name: { strategy: coalesce }
      is_deleted: { strategy: bool_or, type: boolean }
mappings:
  - name: crm_customers
    source: crm
    target: customer
    soft_delete:
      field: deleted_at
      target: is_deleted
    fields:
      - { source: email, target: email }
      - { source: name, target: name }
"#,
        );
        let target = doc.targets.get("customer").unwrap();
        let source = doc.sources.get("crm").unwrap();
        let sql = render_forward_body(
            &doc.mappings[0],
            Some(source),
            Some(target),
            &[],
            &HashMap::new(),
        )
        .unwrap();
        // is_deleted should be emitted as the detection expression, not NULL
        assert!(
            sql.contains("(\"deleted_at\" IS NOT NULL)::boolean AS \"is_deleted\""),
            "soft_delete.target should inject detection expression:\n{sql}"
        );
        // Non-identity fields should still be NULLed on soft-delete
        assert!(
            sql.contains("CASE WHEN (\"deleted_at\" IS NOT NULL) THEN NULL::text ELSE"),
            "non-identity fields should be NULLed on soft-delete:\n{sql}"
        );
    }

    #[test]
    fn derive_tombstones_emits_union_all() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  erp: { primary_key: cust_id }
targets:
  customer:
    fields:
      email: { strategy: identity }
      name: { strategy: coalesce }
      is_deleted: { strategy: bool_or, type: boolean }
mappings:
  - name: erp_customers
    source: erp
    target: customer
    cluster_members: true
    derive_tombstones: is_deleted
    fields:
      - { source: email, target: email }
      - { source: name, target: name }
"#,
        );
        let target = doc.targets.get("customer").unwrap();
        let source = doc.sources.get("erp").unwrap();
        let sql = render_forward_body(
            &doc.mappings[0],
            Some(source),
            Some(target),
            &[],
            &HashMap::new(),
        )
        .unwrap();
        // Should have UNION ALL for absent entities
        assert!(
            sql.contains("UNION ALL"),
            "derive_tombstones should add UNION ALL:\n{sql}"
        );
        // Synthetic rows contribute TRUE for the target field
        assert!(
            sql.contains("TRUE::boolean AS \"is_deleted\""),
            "synthetic row should have TRUE for target field:\n{sql}"
        );
        // Other fields should be NULL
        assert!(
            sql.contains("NULL::text AS \"email\""),
            "synthetic row identity field should be NULL:\n{sql}"
        );
        // Should use cluster_members table
        assert!(
            sql.contains("_cluster_members_erp_customers"),
            "should reference cluster_members table:\n{sql}"
        );
        // Should detect absent entities
        assert!(
            sql.contains("_dt_src.\"cust_id\" IS NULL"),
            "should detect absent entities:\n{sql}"
        );
    }
}
