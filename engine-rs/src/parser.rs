use anyhow::{Context, Result};
use std::path::Path;

use crate::model::MappingDocument;

/// Parse a mapping YAML file into a MappingDocument.
pub fn parse_file(path: &Path) -> Result<MappingDocument> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse_str(&content)
}

/// Parse a YAML string into a MappingDocument.
pub fn parse_str(yaml: &str) -> Result<MappingDocument> {
    let doc: MappingDocument =
        serde_yaml::from_str(yaml).context("Failed to parse mapping YAML")?;
    Ok(doc)
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

    #[test]
    fn parse_all_examples() {
        let examples = examples_dir();
        let mut count = 0;
        let mut failures = Vec::new();

        for entry in std::fs::read_dir(&examples).expect("examples dir") {
            let entry = entry.unwrap();
            if !entry.file_type().unwrap().is_dir() {
                continue;
            }
            let mapping = entry.path().join("mapping.yaml");
            if !mapping.exists() {
                continue;
            }
            count += 1;
            let name = entry.file_name().to_string_lossy().to_string();
            match parse_file(&mapping) {
                Ok(doc) => {
                    assert_eq!(doc.version, "1.0", "{name}: version should be 1.0");
                }
                Err(e) => {
                    failures.push(format!("{name}: {e:#}"));
                }
            }
        }

        assert!(count > 0, "No examples found in {}", examples.display());
        if !failures.is_empty() {
            panic!(
                "Failed to parse {} of {} examples:\n{}",
                failures.len(),
                count,
                failures.join("\n")
            );
        }
    }
}
