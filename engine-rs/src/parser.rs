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
    let mut doc: MappingDocument =
        serde_yaml::from_str(yaml).context("Failed to parse mapping YAML")?;
    resolve_parents(&mut doc)?;
    Ok(doc)
}

/// Post-processing: resolve `parent:` references.
///
/// For each mapping with `parent`:
/// 1. Copy `source` from the parent mapping (inherit dataset).
/// 2. Populate `source.path` from `array`/`array_path` so the render pipeline
///    sees the same internal structure it uses today.
/// 3. Move mapping-level `parent_fields` into `source.parent_fields`.
fn resolve_parents(doc: &mut MappingDocument) -> Result<()> {
    // Multi-pass resolution: resolve parent references iteratively.
    // Each pass resolves mappings whose parent has already been resolved.
    // This handles chains like grandchild → child → parent.
    let max_passes = doc.mappings.len();
    let mut resolved: std::collections::HashSet<String> = doc
        .mappings
        .iter()
        .filter(|m| m.parent.is_none())
        .map(|m| m.name.clone())
        .collect();

    for _pass in 0..max_passes {
        let mut progress = false;
        for i in 0..doc.mappings.len() {
            let m = &doc.mappings[i];
            if resolved.contains(&m.name) {
                continue;
            }
            let parent_name = match m.parent.as_ref() {
                Some(p) => p.clone(),
                None => continue,
            };
            if !resolved.contains(&parent_name) {
                continue; // parent not yet resolved, try next pass
            }

            // Find parent's resolved dataset and path.
            let (parent_dataset, parent_path) = doc
                .mappings
                .iter()
                .find(|other| other.name == parent_name)
                .map(|other| (other.source.dataset.clone(), other.source.path.clone()))
                .ok_or_else(|| anyhow::anyhow!(
                    "mapping '{}': parent '{}' not found",
                    m.name,
                    parent_name
                ))?;

            let m = &mut doc.mappings[i];

            // Inherit source dataset from parent.
            if m.source.dataset.is_empty() {
                m.source.dataset = parent_dataset;
            }

            // Build compound source.path for the render pipeline.
            // parent_path (if any) + this mapping's array/array_path.
            let local_array = m.array.clone().or_else(|| m.array_path.clone());
            if let Some(ref arr) = local_array {
                let full_path = match parent_path {
                    Some(ref pp) => format!("{pp}.{arr}"),
                    None => arr.clone(),
                };
                m.source.path = Some(full_path);
            }

            // Move mapping-level parent_fields into source.parent_fields.
            if m.source.parent_fields.is_empty() && !m.parent_fields.is_empty() {
                m.source.parent_fields = m.parent_fields.clone();
            }

            resolved.insert(m.name.clone());
            progress = true;
        }
        if !progress {
            break;
        }
    }

    Ok(())
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
