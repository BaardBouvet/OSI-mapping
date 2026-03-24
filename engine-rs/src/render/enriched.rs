use anyhow::Result;

use crate::model::{Strategy, Target};
use crate::qi;
use crate::validate_expr::{is_enriched_expression, rewrite_target_refs};

/// Render an enriched view layering computed fields (enriched expressions)
/// on top of the resolved (or ordered) view.
///
/// Produces: `CREATE OR REPLACE VIEW _enriched_{target_name} AS ...`
///
/// Each enriched expression is placed in a LEFT JOIN LATERAL subquery.
/// The outer SELECT exposes all upstream columns plus the computed fields.
pub fn render_enriched_view(
    target_name: &str,
    target: &Target,
    all_target_names: &[&str],
    has_mixed_order: bool,
) -> Result<String> {
    let view_name = qi(&format!("_enriched_{target_name}"));
    let upstream_view = if has_mixed_order {
        qi(&format!("_ordered_{target_name}"))
    } else {
        qi(&format!("_resolved_{target_name}"))
    };

    // Collect enriched fields.
    let enriched_fields: Vec<(&str, &str)> = target
        .fields
        .iter()
        .filter_map(|(fname, fdef)| {
            if fdef.strategy() == Strategy::Expression {
                if let Some(expr) = fdef.expression() {
                    if is_enriched_expression(expr, all_target_names) {
                        return Some((fname.as_str(), expr));
                    }
                }
            }
            None
        })
        .collect();

    if enriched_fields.is_empty() {
        return Ok(String::new());
    }

    // Build SELECT columns: upstream.* + lateral results.
    let mut select_parts = vec![format!("{target_name}.*")];
    let mut lateral_joins = Vec::new();

    for (fname, expr) in &enriched_fields {
        let lat_alias = format!("_lat_{fname}");
        let rewritten = rewrite_target_refs(expr, all_target_names);

        // Wrap the expression: if it starts with SELECT (after trimming),
        // use it directly; otherwise wrap in a SELECT.
        let trimmed = rewritten.trim();
        let inner_sql = if trimmed.to_uppercase().starts_with("SELECT ")
            || trimmed.to_uppercase().starts_with("WITH ")
        {
            // Expression is already a full query — ensure result column is named "val".
            // Replace the last SELECT's first column alias or wrap.
            ensure_val_alias(trimmed)
        } else {
            // Scalar expression — wrap in SELECT.
            format!("SELECT ({trimmed}) AS val")
        };

        lateral_joins.push(format!(
            "LEFT JOIN LATERAL (\n  {inner_sql}\n) {qlat} ON true",
            qlat = qi(&lat_alias),
        ));
        select_parts.push(format!("{}.val AS {}", qi(&lat_alias), qi(fname)));
    }

    let sql = format!(
        "-- Enriched: {target_name}\n\
         CREATE OR REPLACE VIEW {view_name} AS\n\
         SELECT\n  {select}\n\
         FROM {upstream_view} {target_name}\n\
         {laterals};\n",
        select = select_parts.join(",\n  "),
        laterals = lateral_joins.join("\n"),
    );

    Ok(sql)
}

/// Ensure the query expression's result column is aliased as `val`.
///
/// If the expression ends with a top-level SELECT that doesn't have an
/// explicit AS val, we add it. For WITH ... SELECT patterns, we look for
/// the final SELECT.
fn ensure_val_alias(sql: &str) -> String {
    // Simple heuristic: if the SQL already contains "AS val" (case-insensitive)
    // near the end, use it as-is.
    let lower = sql.to_lowercase();
    if lower.contains(" as val") {
        return sql.to_string();
    }

    // Otherwise, wrap the whole thing: SELECT (...) AS val
    // But for WITH RECURSIVE / multi-line queries, we need to alias the
    // final result. The safest approach: wrap in a subquery.
    format!("SELECT ({sql}) AS val")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse(yaml: &str) -> crate::model::MappingDocument {
        parser::parse_str(yaml).expect("valid test YAML")
    }

    #[test]
    fn enriched_view_basic() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
  t: { primary_key: id }
targets:
  line_item:
    fields:
      item_id: { strategy: identity }
      qty: { strategy: coalesce, type: numeric }
      total_shipped:
        strategy: expression
        expression: |
          COALESCE((
            SELECT sum(s.qty)
            FROM shipment s
            WHERE s.line_item = line_item._entity_id
          ), 0)
        type: numeric
  shipment:
    fields:
      ship_id: { strategy: identity }
      line_item: { strategy: coalesce, references: line_item }
      qty: { strategy: coalesce, type: numeric }
mappings:
  - name: s
    source: s
    target: line_item
    fields:
      - { source: id, target: item_id }
      - { source: qty, target: qty }
  - name: t
    source: t
    target: shipment
    fields:
      - { source: id, target: ship_id }
      - { source: line, target: line_item }
      - { source: qty, target: qty }
"#,
        );
        let target = doc.targets.get("line_item").unwrap();
        let target_names: Vec<&str> = doc.targets.keys().map(|s| s.as_str()).collect();
        let sql = render_enriched_view("line_item", target, &target_names, false).unwrap();

        assert!(sql.contains("_enriched_line_item"), "view name");
        assert!(
            sql.contains("FROM \"_resolved_line_item\" line_item"),
            "upstream"
        );
        assert!(sql.contains("LEFT JOIN LATERAL"), "lateral join");
        assert!(
            sql.contains("\"_resolved_shipment\""),
            "rewritten target ref"
        );
        assert!(sql.contains("AS \"total_shipped\""), "field alias");
    }

    #[test]
    fn enriched_view_skips_non_enriched() {
        let doc = parse(
            r#"
version: "1.0"
sources:
  s: { primary_key: id }
targets:
  contact:
    fields:
      email: { strategy: identity }
      name: { strategy: coalesce }
mappings:
  - name: s
    source: s
    target: contact
    fields:
      - { source: email, target: email }
      - { source: name, target: name }
"#,
        );
        let target = doc.targets.get("contact").unwrap();
        let target_names: Vec<&str> = doc.targets.keys().map(|s| s.as_str()).collect();
        let sql = render_enriched_view("contact", target, &target_names, false).unwrap();
        assert!(sql.is_empty(), "no enriched view when no enriched fields");
    }
}
