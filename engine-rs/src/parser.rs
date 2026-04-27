//! YAML loader for v2 mapping files.

use crate::model::Doc;
use anyhow::{Context, Result};
use std::path::Path;

pub fn parse_str(yaml: &str) -> Result<Doc> {
    let doc: Doc = serde_yaml::from_str(yaml).context("YAML parse failed")?;
    if doc.version != "2.0" {
        anyhow::bail!(
            "version must be \"2.0\" (got \"{}\"); v1 files do not load on v2",
            doc.version
        );
    }
    Ok(doc)
}

pub fn parse_file(path: &Path) -> Result<Doc> {
    let yaml =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_str(&yaml).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{IdentityGroup, Strategy};

    #[test]
    fn parses_hello_world() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples/hello-world/mapping.yaml");
        let doc = parse_file(&path).expect("parses");

        assert_eq!(doc.version, "2.0");
        assert_eq!(doc.sources.len(), 2);
        assert_eq!(doc.sources["crm"].primary_key, "id");

        let contact = &doc.targets["contact"];
        assert_eq!(contact.identity.len(), 1);
        assert!(matches!(&contact.identity[0], IdentityGroup::Single(s) if s == "email"));
        assert_eq!(contact.fields["email"].strategy, Strategy::Coalesce);
        assert_eq!(contact.fields["name"].strategy, Strategy::Coalesce);

        assert_eq!(doc.mappings.len(), 2);
        assert_eq!(doc.mappings[0].name, "crm");
        assert_eq!(doc.mappings[0].fields.len(), 2);
        assert_eq!(doc.mappings[0].fields[1].priority, Some(1));

        assert_eq!(doc.tests.len(), 3);
        assert!(doc.tests[0].description.contains("CRM name wins"));
    }

    #[test]
    fn rejects_v1_version() {
        let yaml = r#"
version: "1.0"
sources: {}
targets: {}
mappings: []
"#;
        let err = parse_str(yaml).unwrap_err();
        assert!(err.to_string().contains("v2"));
    }

    #[test]
    fn rejects_unknown_keys() {
        let yaml = r#"
version: "2.0"
sources: {}
targets: {}
mappings: []
extra_typo: "oops"
"#;
        let err = parse_str(yaml).unwrap_err();
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("unknown field") || msg.contains("extra_typo"),
            "got: {msg}"
        );
    }

    #[test]
    fn parses_slice3_nested_keywords() {
        // Slice 3 foundation: parser must accept `parent:`, `array:`,
        // `parent_fields:`, and `references:` even though the renderers
        // bail on them.
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples/nested-arrays-v2/mapping.yaml");
        let doc = parse_file(&path).expect("parses");

        let lines = doc
            .mappings
            .iter()
            .find(|m| m.name == "shop_lines")
            .expect("shop_lines mapping");
        assert_eq!(lines.parent.as_deref(), Some("shop_orders"));
        assert_eq!(lines.array.as_deref(), Some("lines"));
        assert_eq!(
            lines.parent_fields.get("parent_order_id"),
            Some(&"order_id".to_string())
        );
        assert_eq!(
            lines.fields[0].references.as_deref(),
            Some("purchase_order")
        );
    }
}
