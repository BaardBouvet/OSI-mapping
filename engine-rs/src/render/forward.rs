use anyhow::Result;

use crate::model::{Mapping, Source, Target};

/// Render a CREATE VIEW statement for a forward mapping.
///
/// Produces: `CREATE OR REPLACE VIEW _fwd_{mapping_name} AS SELECT ...`
pub fn render_forward_view(
    mapping: &Mapping,
    source_meta: Option<&Source>,
    target: Option<&Target>,
) -> Result<String> {
    let view_name = format!("_fwd_{}", mapping.name);
    let body = render_forward_body(mapping, source_meta, target)?;
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
) -> Result<String> {
    let source = source_meta
        .map(|s| s.table_name(&mapping.source.dataset).to_string())
        .unwrap_or_else(|| mapping.source.dataset.clone());

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
        let fallback = format!("md5('{}' || ':' || {})", mapping.name, src_id_expr);
        cols.push(format!(
            "COALESCE({cf}, {fallback}) AS _cluster_id"
        ));
    } else if let Some(ref cm) = mapping.cluster_members {
        // cluster_members: LEFT JOIN happens in FROM clause (handled below).
        // _cluster_id comes from the join, fallback to md5 singleton.
        let cm_cluster = &cm.cluster_id;
        let fallback = format!("md5('{}' || ':' || {})", mapping.name, src_id_expr);
        cols.push(format!(
            "COALESCE(_cm.{cm_cluster}, {fallback}) AS _cluster_id"
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
                format!("{field} AS _last_modified")
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
        for (fname, _fdef) in &target.fields {
            let fm = mapping.fields.iter().find(|fm| {
                fm.is_forward() && fm.target.as_deref() == Some(fname.as_str())
            });

            if let Some(fm) = fm {
                let expr = if let Some(ref e) = fm.expression {
                    e.clone()
                } else if let Some(ref src) = fm.source {
                    src.clone()
                } else {
                    "NULL".into()
                };
                // Cast to text for UNION ALL compatibility across mappings.
                cols.push(format!("{expr}::text AS {fname}"));

                // Per-field priority (always present, NULL when unset)
                cols.push(match fm.priority {
                    Some(p) => format!("{p} AS _priority_{fname}"),
                    None => format!("NULL::int AS _priority_{fname}"),
                });

                // Per-field timestamp (always present, NULL when unset)
                cols.push(match &fm.last_modified {
                    Some(ts) => {
                        if let Some(field) = ts.field_name() {
                            format!("{field} AS _ts_{fname}")
                        } else if let Some(expr) = ts.expression() {
                            format!("({expr}) AS _ts_{fname}")
                        } else {
                            format!("NULL::text AS _ts_{fname}")
                        }
                    }
                    None => format!("NULL::text AS _ts_{fname}"),
                });
            } else {
                // Not mapped by this mapping — emit NULL placeholders
                cols.push(format!("NULL::text AS {fname}"));
                cols.push(format!("NULL::int AS _priority_{fname}"));
                cols.push(format!("NULL::text AS _ts_{fname}"));
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
                } else if let Some(ref src) = fm.source {
                    src.clone()
                } else {
                    continue;
                };
                cols.push(format!("{expr}::text AS {tgt}"));
            }
        }
    }

    // _base: JSONB snapshot of raw source columns involved in field mappings.
    // Built here (pre-expression) so it flows through identity via SELECT *.
    {
        let mut base_parts: Vec<String> = Vec::new();
        for fm in &mapping.fields {
            if !fm.is_forward() {
                continue;
            }
            if let Some(ref src) = fm.source {
                base_parts.push(format!("'{src}', {src}"));
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
        let cm_table = cm.table_name(&mapping.name);
        let cm_src_key = &cm.source_key;
        sql.push_str(&format!(
            "\nLEFT JOIN {cm_table} AS _cm ON _cm.{cm_src_key} = {src_id_expr}"
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
                seg.to_string()
            } else {
                format!("_nest_{}.value", i - 1)
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
            .map(|m| render_forward_body(m, None, Some(target)).unwrap())
            .collect();

        // Both views must have identical column sets
        for sql in &sqls {
            assert!(
                sql.contains("_row_id::text AS _src_id")
                    || sql.contains("_row_id AS _src_id"),
                "missing _src_id"
            );
            assert!(sql.contains("AS _mapping"), "missing _mapping");
            assert!(sql.contains("AS _priority\n") || sql.contains("AS _priority,"), "missing _priority");
            assert!(sql.contains("AS _last_modified"), "missing _last_modified");
            assert!(sql.contains("AS email"), "missing email");
            assert!(sql.contains("AS name"), "missing name");
            assert!(sql.contains("AS _priority_email"), "missing _priority_email");
            assert!(sql.contains("AS _priority_name"), "missing _priority_name");
            assert!(sql.contains("AS _ts_email"), "missing _ts_email");
            assert!(sql.contains("AS _ts_name"), "missing _ts_name");
        }
    }
}
