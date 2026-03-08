use anyhow::Result;

use crate::model::{Mapping, Target};

/// Render a forward mapping view.
///
/// Emits a normalized column set so all forward views for the same target
/// are UNION ALL compatible:
///   `_src_id, _mapping, _priority, _last_modified,
///    {field}, _priority_{field}, _ts_{field}, ...`
///
/// Source tables are expected to have a `_row_id SERIAL PRIMARY KEY`.
pub fn render_forward_view(mapping: &Mapping, target: Option<&Target>) -> Result<String> {
    let view_name = format!("_fwd_{}", mapping.name);
    let source = &mapping.source.dataset;

    let mut cols: Vec<String> = Vec::new();

    // Source row identifier — source tables must have _row_id SERIAL
    cols.push("_row_id AS _src_id".into());
    cols.push(format!("'{}'::text AS _mapping", mapping.name));

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
                cols.push(format!("{expr} AS {fname}"));

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
                cols.push(format!("{expr} AS {tgt}"));
            }
        }
    }

    let mut sql = format!(
        "-- Forward: {name} ({source} → {target})\n\
         CREATE OR REPLACE VIEW {view_name} AS\nSELECT\n  {columns}\nFROM {source}",
        name = mapping.name,
        target = mapping.target.name(),
        columns = cols.join(",\n  "),
    );

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

    sql.push_str(";\n");
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
            .map(|m| render_forward_view(m, Some(target)).unwrap())
            .collect();

        // Both views must have identical column sets
        for sql in &sqls {
            assert!(sql.contains("_row_id AS _src_id"), "missing _src_id");
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
