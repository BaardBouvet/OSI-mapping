use anyhow::Result;

use crate::model::Target;
use crate::qi;

/// Render an analytics view that exposes the resolved golden record
/// in a clean, consumer-friendly shape for BI and analytics tools.
///
/// Produces: `CREATE OR REPLACE VIEW {target_name} AS ...`
///
/// Consumer-facing — named directly after the target (no underscore prefix).
/// Emits `_cluster_id` (aliased from `_entity_id`) and all resolved
/// business fields — no internal metadata columns.
pub fn render_analytics_view(target_name: &str, target: &Target) -> Result<String> {
    let resolved_view = qi(&format!("_resolved_{target_name}"));
    let qview_name = qi(target_name);

    let mut select_exprs: Vec<String> = Vec::new();
    select_exprs.push("_entity_id AS _cluster_id".to_string());

    for (fname, _fdef) in &target.fields {
        select_exprs.push(qi(fname));
    }

    let sql = format!(
        "-- {target_name}\n\
         CREATE OR REPLACE VIEW {qview_name} AS\n\
         SELECT\n  {columns}\nFROM {resolved_view};\n",
        columns = select_exprs.join(",\n  "),
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
    fn analytics_selects_from_resolved() {
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
        let sql = render_analytics_view("contact", target).unwrap();
        assert!(
            sql.contains("FROM \"_resolved_contact\""),
            "should select from resolved view"
        );
        assert!(
            sql.contains("_entity_id AS _cluster_id"),
            "should alias _entity_id"
        );
    }

    #[test]
    fn analytics_only_user_fields() {
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
        let sql = render_analytics_view("contact", target).unwrap();
        assert!(sql.contains("\"email\""), "should include email field");
        assert!(sql.contains("\"name\""), "should include name field");
        assert!(!sql.contains("_src_id"), "should not include _src_id");
        assert!(!sql.contains("_mapping"), "should not include _mapping");
        assert!(!sql.contains("_base"), "should not include _base");
    }
}
