//! Mapping document validation.
//!
//! Pass 0 — JSON Schema validation against `spec/mapping-schema.json`.
//! Runs *before* serde deserialization so the user sees every structural
//! error at once instead of the first serde failure.
//!
//! Semantic validation passes (cross-references, strategy compatibility,
//! etc.) live alongside the renderers and are not expressible in JSON
//! Schema; they continue to bail on the first error.

use anyhow::{Context, Result};
use jsonschema::Validator;
use std::sync::OnceLock;

/// Embedded copy of the v2 schema. Bundled at compile time so the engine
/// stays single-binary and never reads the schema file at runtime.
const SCHEMA_JSON: &str = include_str!("../../spec/mapping-schema.json");

fn validator() -> &'static Validator {
    static V: OnceLock<Validator> = OnceLock::new();
    V.get_or_init(|| {
        let schema: serde_json::Value =
            serde_json::from_str(SCHEMA_JSON).expect("embedded schema is valid JSON");
        jsonschema::validator_for(&schema).expect("embedded schema compiles")
    })
}

/// One structural error reported against the input document.
///
/// `path` is a JSON-Pointer-ish path to the offending node (e.g.
/// `/mappings/0/fields/2/source`). `message` is a human-readable
/// description from the JSON Schema validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// Validates `value` against the embedded v2 mapping schema, collecting
/// **all** structural errors. Returns `Ok(())` if the document is
/// schema-valid; otherwise returns the full list.
pub fn validate_schema(value: &serde_json::Value) -> std::result::Result<(), Vec<SchemaError>> {
    let v = validator();
    let errors: Vec<SchemaError> = v
        .iter_errors(value)
        .map(|e| SchemaError {
            path: e.instance_path().to_string(),
            message: e.to_string(),
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Convenience: parse YAML text → JSON value → schema-validate.
///
/// YAML parse failures are surfaced as a single anyhow error. Schema
/// failures are returned via the inner `Result` so callers can
/// pretty-print all of them.
pub fn validate_schema_yaml(yaml: &str) -> Result<std::result::Result<(), Vec<SchemaError>>> {
    let value: serde_json::Value = serde_yaml::from_str(yaml).context("YAML parse failed")?;
    Ok(validate_schema(&value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn examples_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples")
    }

    fn read_example(name: &str) -> String {
        std::fs::read_to_string(examples_dir().join(name).join("mapping.yaml"))
            .unwrap_or_else(|e| panic!("read examples/{name}: {e}"))
    }

    #[test]
    fn all_v2_examples_pass_schema() {
        // Every example whose `version: "2.0"` should validate cleanly.
        let v2_examples = [
            "hello-world",
            "composite-identity",
            "last-modified",
            "nested-arrays-shallow",
            "nested-arrays-v2",
        ];
        for name in v2_examples {
            let yaml = read_example(name);
            let value: serde_json::Value =
                serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("YAML parse {name}: {e}"));
            match validate_schema(&value) {
                Ok(()) => {}
                Err(errs) => panic!(
                    "schema validation of examples/{name} failed:\n{}",
                    errs.iter()
                        .map(|e| format!("  - {e}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            }
        }
    }

    #[test]
    fn rejects_v1_version() {
        let yaml = r#"
version: "1.0"
sources: { s: { primary_key: id } }
targets: { t: { identity: [x], fields: { x: { strategy: coalesce } } } }
mappings: [ { name: m, source: s, target: t, fields: [{ source: x, target: x }] } ]
"#;
        let result = validate_schema_yaml(yaml).unwrap();
        let errs = result.expect_err("v1 must be rejected");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("2.0") || e.path.contains("version")),
            "expected version error, got: {errs:?}"
        );
    }

    #[test]
    fn collects_multiple_errors_at_once() {
        // Three separate structural errors — typo in a key, wrong strategy
        // value, and `additionalProperties` violation. All three must
        // appear in the report instead of failing fast on the first one.
        let yaml = r#"
version: "2.0"
sources:
  crm:
    primary_key: id
targets:
  contact:
    identity:
      - email
    fields:
      email:
        strategy: not_a_strategy
mappings:
  - name: m
    source: crm
    target: contact
    fields:
      - source: email
        target: email
        bogus_field: oops
extra_top_level_key: nope
"#;
        let result = validate_schema_yaml(yaml).unwrap();
        let errs = result.expect_err("must fail");
        assert!(
            errs.len() >= 2,
            "expected ≥2 errors at once, got {}: {errs:?}",
            errs.len()
        );
        let blob = errs
            .iter()
            .map(|e| format!("{e}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            blob.contains("not_a_strategy") || blob.contains("strategy"),
            "expected strategy error in:\n{blob}"
        );
        assert!(
            blob.contains("extra_top_level_key") || blob.contains("additional"),
            "expected additional-property error in:\n{blob}"
        );
    }

    #[test]
    fn rejects_uppercase_identifier() {
        // Identifier pattern is `^[a-z][a-z0-9_]*$`. Uppercase source name
        // must be flagged — protects downstream SQL/SPARQL emit.
        let yaml = r#"
version: "2.0"
sources:
  CRM:
    primary_key: id
targets:
  contact:
    identity: [email]
    fields:
      email: { strategy: coalesce }
mappings:
  - name: m
    source: CRM
    target: contact
    fields:
      - { source: email, target: email }
"#;
        let result = validate_schema_yaml(yaml).unwrap();
        result.expect_err("uppercase source key must be rejected");
    }

    #[test]
    fn rejects_missing_required_keys() {
        // Source missing primary_key, mapping missing fields.
        let yaml = r#"
version: "2.0"
sources:
  crm: {}
targets:
  contact:
    identity: [email]
    fields:
      email: { strategy: coalesce }
mappings:
  - name: m
    source: crm
    target: contact
"#;
        let result = validate_schema_yaml(yaml).unwrap();
        let errs = result.expect_err("must fail");
        let blob = errs
            .iter()
            .map(|e| format!("{e}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            blob.contains("primary_key") || blob.contains("required"),
            "expected primary_key required error, got:\n{blob}"
        );
        assert!(
            blob.contains("fields") || blob.contains("required"),
            "expected fields required error, got:\n{blob}"
        );
    }
}
