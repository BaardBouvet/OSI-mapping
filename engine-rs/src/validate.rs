use std::collections::{HashMap, HashSet};

use crate::model::{MappingDocument, Strategy};

/// A validation diagnostic — either an error or a warning.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: Level,
    pub pass: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.pass, self.message)
    }
}

/// Result of validating a mapping document.
#[derive(Debug, Default)]
pub struct ValidationResult {
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidationResult {
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.level == Level::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.level == Level::Warning)
    }

    pub fn error_count(&self) -> usize {
        self.errors().count()
    }

    pub fn warning_count(&self) -> usize {
        self.warnings().count()
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.level == Level::Error)
    }

    fn error(&mut self, pass: &'static str, message: String) {
        self.diagnostics.push(Diagnostic {
            level: Level::Error,
            pass,
            message,
        });
    }

    fn warning(&mut self, pass: &'static str, message: String) {
        self.diagnostics.push(Diagnostic {
            level: Level::Warning,
            pass,
            message,
        });
    }
}

/// Run all validation passes on a parsed mapping document.
///
/// Pass 1 (structural) is handled by serde deserialization — if the document
/// parsed successfully, it already satisfies the JSON schema constraints.
/// Passes 2-7 perform semantic validation.
pub fn validate(doc: &MappingDocument) -> ValidationResult {
    let mut result = ValidationResult::default();

    // Pass 1: Schema / structural (covered by serde deserialization)
    pass_structural(doc, &mut result);

    // Pass 2: Unique names
    pass_unique_names(doc, &mut result);

    // Pass 3: Target references
    pass_target_refs(doc, &mut result);

    // Pass 4: Strategy consistency
    pass_strategy_consistency(doc, &mut result);

    // Pass 5: Field coverage
    pass_field_coverage(doc, &mut result);

    // Pass 6: Test dataset consistency
    pass_test_datasets(doc, &mut result);

    // Pass 6b: Source primary-key consistency
    pass_source_primary_keys(doc, &mut result);

    // Pass 7: SQL expression syntax
    pass_sql_syntax(doc, &mut result);

    // Pass 8: Origin/cluster rules
    pass_origin_cluster(doc, &mut result);

    result
}

// ──────────────────────────────────────────────────────────────────────
// Pass 1 — Structural checks beyond what serde catches
// ──────────────────────────────────────────────────────────────────────

fn pass_structural(doc: &MappingDocument, result: &mut ValidationResult) {
    if doc.version != "1.0" {
        result.error(
            "Schema",
            format!("version must be '1.0', got '{}'", doc.version),
        );
    }

    if doc.targets.is_empty() && doc.mappings.is_empty() {
        result.error(
            "Schema",
            "document must have at least 'targets' or 'mappings'".into(),
        );
    }

    // Validate naming conventions
    let name_re = regex::Regex::new(r"^[a-z][a-z0-9_]*$").unwrap();

    for name in doc.targets.keys() {
        if !name_re.is_match(name) {
            result.error(
                "Schema",
                format!(
                    "target name '{name}' must match ^[a-z][a-z0-9_]*$"
                ),
            );
        }
    }

    for mapping in &doc.mappings {
        if !name_re.is_match(&mapping.name) {
            result.error(
                "Schema",
                format!(
                    "mapping name '{}' must match ^[a-z][a-z0-9_]*$",
                    mapping.name
                ),
            );
        }

        if mapping.fields.is_empty() && mapping.links.is_empty() {
            result.error(
                "Schema",
                format!("mapping '{}': must have fields or links", mapping.name),
            );
        }

        // Each field mapping must have at least source or target
        for (i, fm) in mapping.fields.iter().enumerate() {
            if fm.source.is_none() && fm.target.is_none() {
                result.error(
                    "Schema",
                    format!(
                        "mapping '{}' field[{}]: must have at least 'source' or 'target'",
                        mapping.name, i
                    ),
                );
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Pass 2 — Unique names
// ──────────────────────────────────────────────────────────────────────

fn pass_unique_names(doc: &MappingDocument, result: &mut ValidationResult) {
    // 2a: mapping names must be unique
    let mut name_counts: HashMap<&str, usize> = HashMap::new();
    for m in &doc.mappings {
        *name_counts.entry(&m.name).or_insert(0) += 1;
    }
    for (name, count) in &name_counts {
        if *count > 1 {
            result.error(
                "Unique",
                format!("Mapping name '{name}' appears {count} times"),
            );
        }
    }

    // 2b: within each mapping, field targets should be unique
    for m in &doc.mappings {
        let mut target_counts: HashMap<(&str, &str), usize> = HashMap::new();
        for fm in &m.fields {
            let src = fm.source.as_deref().unwrap_or("<none>");
            let tgt = fm.target.as_deref().unwrap_or("<none>");
            *target_counts.entry((src, tgt)).or_insert(0) += 1;
        }
        for ((src, tgt), count) in &target_counts {
            if *count > 1 && *tgt != "<none>" {
                result.error(
                    "Unique",
                    format!(
                        "Mapping '{}': field target '{tgt}' (source '{src}') appears {count} times",
                        m.name
                    ),
                );
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Pass 3 — Target references
// ──────────────────────────────────────────────────────────────────────

fn pass_target_refs(doc: &MappingDocument, result: &mut ValidationResult) {
    let target_names: HashSet<&str> = doc.targets.keys().map(|s| s.as_str()).collect();
    let sorted_targets: Vec<&str> = {
        let mut v: Vec<&str> = target_names.iter().copied().collect();
        v.sort();
        v
    };

    // 3a: mapping.target must reference a defined target (when string name)
    for m in &doc.mappings {
        let tname = m.target.name();
        if !target_names.contains(tname) {
            result.error(
                "Reference",
                format!(
                    "Mapping '{}': target '{}' not found in targets ({})",
                    m.name,
                    tname,
                    if sorted_targets.is_empty() {
                        "none".to_string()
                    } else {
                        sorted_targets.join(", ")
                    }
                ),
            );
        }
    }

    // 3b: target field references must point to other targets
    for (tname, tdef) in &doc.targets {
        for (fname, fdef) in &tdef.fields {
            if let Some(ref_target) = fdef.references() {
                if !target_names.contains(ref_target) {
                    result.error(
                        "Reference",
                        format!(
                            "Target '{tname}.{fname}': references '{ref_target}' not found in targets"
                        ),
                    );
                }
            }
        }
    }

    // 3c: field mapping references must point to an existing mapping name
    let mapping_names: HashSet<&str> = doc.mappings.iter().map(|m| m.name.as_str()).collect();
    for m in &doc.mappings {
        for (i, fm) in m.fields.iter().enumerate() {
            if let Some(ref ref_mapping) = fm.references {
                if !mapping_names.contains(ref_mapping.as_str()) {
                    let src = fm.source.as_deref().unwrap_or("?");
                    result.error(
                        "Reference",
                        format!(
                            "Mapping '{}' field[{i}] ({src}): references mapping '{ref_mapping}' not found",
                            m.name
                        ),
                    );
                }
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Pass 4 — Strategy consistency
// ──────────────────────────────────────────────────────────────────────

/// Contribution: which mappings contribute to which target fields.
struct Contribution<'a> {
    mapping_name: &'a str,
    mapping_priority: Option<i64>,
    mapping_last_modified: bool,
    field_priority: Option<i64>,
    field_last_modified: bool,
}

fn pass_strategy_consistency(doc: &MappingDocument, result: &mut ValidationResult) {
    // Build contribution index: (target_name, field_name) → [Contribution]
    let mut contributions: HashMap<(&str, &str), Vec<Contribution>> = HashMap::new();

    for m in &doc.mappings {
        let tname = m.target.name();
        for fm in &m.fields {
            if let Some(ref ftarget) = fm.target {
                contributions
                    .entry((tname, ftarget))
                    .or_default()
                    .push(Contribution {
                        mapping_name: &m.name,
                        mapping_priority: m.priority,
                        mapping_last_modified: m.last_modified.is_some(),
                        field_priority: fm.priority,
                        field_last_modified: fm.last_modified.is_some(),
                    });
            }
        }
    }

    for (tname, tdef) in &doc.targets {
        for (fname, fdef) in &tdef.fields {
            let strategy = fdef.strategy();

            // 4a: expression strategy must have expression on target field
            if strategy == Strategy::Expression {
                if fdef.expression().is_none() {
                    result.error(
                        "Strategy",
                        format!(
                            "Target '{tname}.{fname}': strategy 'expression' requires an 'expression'"
                        ),
                    );
                }
            }

            // 4b: link_group requires identity strategy
            if fdef.link_group().is_some() && strategy != Strategy::Identity {
                result.error(
                    "Strategy",
                    format!(
                        "Target '{tname}.{fname}': link_group requires strategy 'identity', got '{:?}'",
                        strategy
                    ),
                );
            }

            // 4c: group is typically used with last_modified or coalesce
            if fdef.group().is_some()
                && strategy != Strategy::LastModified
                && strategy != Strategy::Coalesce
            {
                result.warning(
                    "Strategy",
                    format!(
                        "Target '{tname}.{fname}': group is typically used with 'last_modified' strategy, got '{:?}'",
                        strategy
                    ),
                );
            }

            let contribs = contributions.get(&(tname.as_str(), fname.as_str()));
            let contrib_count = contribs.map_or(0, |c| c.len());

            // 4d: coalesce — contributing mappings should have priority
            if strategy == Strategy::Coalesce && contrib_count > 1 {
                if let Some(contribs) = contribs {
                    for c in contribs {
                        let has_priority =
                            c.field_priority.is_some() || c.mapping_priority.is_some();
                        if !has_priority {
                            result.warning(
                                "Strategy",
                                format!(
                                    "Mapping '{}' → '{tname}.{fname}': coalesce strategy but no priority set",
                                    c.mapping_name
                                ),
                            );
                        }
                    }
                }
            }

            // 4e: last_modified — contributing mappings should have timestamp
            if strategy == Strategy::LastModified && contrib_count > 1 {
                if let Some(contribs) = contribs {
                    for c in contribs {
                        let has_ts = c.field_last_modified || c.mapping_last_modified;
                        if !has_ts {
                            result.warning(
                                "Strategy",
                                format!(
                                    "Mapping '{}' → '{tname}.{fname}': last_modified strategy but no timestamp source",
                                    c.mapping_name
                                ),
                            );
                        }
                    }
                }
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Pass 5 — Field coverage
// ──────────────────────────────────────────────────────────────────────

fn pass_field_coverage(doc: &MappingDocument, result: &mut ValidationResult) {
    let mut contributed: HashSet<(&str, &str)> = HashSet::new();

    for m in &doc.mappings {
        let tname = m.target.name();
        if !doc.targets.contains_key(tname) {
            continue; // caught in pass 3
        }
        let target_fields: HashSet<&str> = doc.targets[tname]
            .fields
            .keys()
            .map(|s| s.as_str())
            .collect();

        for fm in &m.fields {
            if let Some(ref ftarget) = fm.target {
                contributed.insert((tname, ftarget));
                if !target_fields.contains(ftarget.as_str()) {
                    let sorted: Vec<&str> = {
                        let mut v: Vec<&str> = target_fields.iter().copied().collect();
                        v.sort();
                        v
                    };
                    result.error(
                        "Field",
                        format!(
                            "Mapping '{}': field target '{}' not found in target '{}' fields ({})",
                            m.name,
                            ftarget,
                            tname,
                            sorted.join(", ")
                        ),
                    );
                }
            }
        }
    }

    // Warn about orphan target fields
    for (tname, tdef) in &doc.targets {
        for fname in tdef.fields.keys() {
            if !contributed.contains(&(tname.as_str(), fname.as_str())) {
                result.warning(
                    "Field",
                    format!(
                        "Target '{tname}.{fname}': no mapping contributes to this field"
                    ),
                );
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Pass 6 — Test dataset consistency
// ──────────────────────────────────────────────────────────────────────

fn pass_test_datasets(doc: &MappingDocument, result: &mut ValidationResult) {
    let source_datasets: HashSet<&str> = doc
        .mappings
        .iter()
        .map(|m| m.source.dataset.as_str())
        .collect();

    let sorted_sources: Vec<&str> = {
        let mut v: Vec<&str> = source_datasets.iter().copied().collect();
        v.sort();
        v
    };

    for (i, tc) in doc.tests.iter().enumerate() {
        let default_desc = format!("test[{i}]");
        let desc = tc
            .description
            .as_deref()
            .unwrap_or(&default_desc);

        for ds in tc.input.keys() {
            if !source_datasets.contains(ds.as_str()) {
                result.warning(
                    "Test",
                    format!(
                        "'{desc}': input dataset '{ds}' not found in mapping sources ({})",
                        sorted_sources.join(", ")
                    ),
                );
            }
        }

        for ds in tc.expected.keys() {
            if !source_datasets.contains(ds.as_str()) {
                result.warning(
                    "Test",
                    format!(
                        "'{desc}': expected dataset '{ds}' not found in mapping sources ({})",
                        sorted_sources.join(", ")
                    ),
                );
            }
        }
    }
}

fn pass_source_primary_keys(doc: &MappingDocument, result: &mut ValidationResult) {
    if doc.sources.is_empty() {
        return;
    }

    let mapping_sources: HashSet<&str> = doc
        .mappings
        .iter()
        .map(|m| m.source.dataset.as_str())
        .collect();

    for (source_name, source_def) in &doc.sources {
        if !mapping_sources.contains(source_name.as_str()) {
            result.warning(
                "PrimaryKey",
                format!(
                    "source '{source_name}' is declared in sources but not used by any mapping"
                ),
            );
        }

        let pk_cols = source_def.primary_key.columns();
        if pk_cols.is_empty() {
            result.error(
                "PrimaryKey",
                format!("source '{source_name}': primary_key must include at least one column"),
            );
            continue;
        }

        // Warning: mapping PK columns as non-identity target fields is unusual.
        // Skip this warning if the target field has identity strategy — PKs
        // mapped to identity fields is the normal single-natural-key pattern.
        for m in doc
            .mappings
            .iter()
            .filter(|m| m.source.dataset == *source_name)
        {
            let target_def = doc.targets.get(m.target.name());
            for fm in &m.fields {
                if let Some(src_col) = fm.source.as_deref() {
                    if pk_cols.contains(&src_col) {
                        let is_identity = fm.target.as_deref().and_then(|tgt| {
                            target_def.and_then(|t| t.fields.get(tgt))
                        }).map(|f| f.strategy() == Strategy::Identity).unwrap_or(false);
                        if !is_identity {
                            result.warning(
                                "PrimaryKey",
                                format!(
                                    "mapping '{}': source PK column '{}' is mapped to a non-identity target field",
                                    m.name, src_col
                                ),
                            );
                        }
                    }
                }
            }
        }
    }

    for (i, tc) in doc.tests.iter().enumerate() {
        let default_desc = format!("test[{i}]");
        let desc = tc.description.as_deref().unwrap_or(&default_desc);

        for (dataset, rows) in &tc.input {
            let Some(source_def) = doc.sources.get(dataset) else {
                continue;
            };

            let pk_cols = source_def.primary_key.columns();
            let mut seen: HashSet<String> = HashSet::new();

            for (row_idx, row) in rows.iter().enumerate() {
                let Some(obj) = row.as_object() else {
                    result.error(
                        "PrimaryKey",
                        format!(
                            "'{desc}' dataset '{dataset}' row[{row_idx}] must be an object"
                        ),
                    );
                    continue;
                };

                let mut pk_parts = Vec::new();
                let mut missing_cols = Vec::new();

                for col in &pk_cols {
                    match obj.get(*col) {
                        Some(val) if !val.is_null() => pk_parts.push(val.to_string()),
                        _ => missing_cols.push((*col).to_string()),
                    }
                }

                if !missing_cols.is_empty() {
                    result.error(
                        "PrimaryKey",
                        format!(
                            "'{desc}' dataset '{dataset}' row[{row_idx}] missing PK column(s): {}",
                            missing_cols.join(", ")
                        ),
                    );
                    continue;
                }

                let key = pk_parts.join("|");
                if !seen.insert(key.clone()) {
                    result.error(
                        "PrimaryKey",
                        format!(
                            "'{desc}' dataset '{dataset}' has duplicate primary key value '{}'",
                            key
                        ),
                    );
                }
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Pass 7 — SQL expression syntax (basic parenthesis/quote balancing)
// ──────────────────────────────────────────────────────────────────────

fn pass_sql_syntax(doc: &MappingDocument, result: &mut ValidationResult) {
    // Check target-level expressions
    for (tname, tdef) in &doc.targets {
        for (fname, fdef) in &tdef.fields {
            if let Some(expr) = fdef.expression() {
                check_sql_expr(
                    expr,
                    &format!("target '{tname}.{fname}' expression"),
                    result,
                );
            }
            if let Some(expr) = fdef.default_expression() {
                check_sql_expr(
                    expr,
                    &format!("target '{tname}.{fname}' default_expression"),
                    result,
                );
            }
        }
    }

    // Check mapping-level expressions
    for m in &doc.mappings {
        if let Some(ref filter) = m.filter {
            check_sql_expr(filter, &format!("mapping '{}' filter", m.name), result);
        }
        if let Some(ref filter) = m.reverse_filter {
            check_sql_expr(
                filter,
                &format!("mapping '{}' reverse_filter", m.name),
                result,
            );
        }

        for fm in &m.fields {
            let label = fm
                .target
                .as_deref()
                .or(fm.source.as_deref())
                .unwrap_or("?");

            if let Some(ref expr) = fm.expression {
                check_sql_expr(
                    expr,
                    &format!("mapping '{}' field '{label}' expression", m.name),
                    result,
                );
            }
            if let Some(ref expr) = fm.reverse_expression {
                check_sql_expr(
                    expr,
                    &format!("mapping '{}' field '{label}' reverse_expression", m.name),
                    result,
                );
            }
        }
    }
}

/// Basic SQL expression syntax check: balanced parentheses and quotes.
fn check_sql_expr(expr: &str, location: &str, result: &mut ValidationResult) {
    let mut paren_depth: i32 = 0;
    let mut in_single_quote = false;
    let chars: Vec<char> = expr.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];
        if in_single_quote {
            if ch == '\'' {
                // Check for escaped quote ('')
                if i + 1 < len && chars[i + 1] == '\'' {
                    i += 2; // skip the escaped quote
                    continue;
                }
                in_single_quote = false;
            }
        } else {
            match ch {
                '\'' => in_single_quote = true,
                '(' => paren_depth += 1,
                ')' => {
                    paren_depth -= 1;
                    if paren_depth < 0 {
                        result.error(
                            "SQL",
                            format!("{location}: unmatched closing parenthesis"),
                        );
                        return;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }

    if paren_depth != 0 {
        result.error(
            "SQL",
            format!("{location}: unbalanced parentheses (depth {paren_depth})"),
        );
    }

    if in_single_quote {
        result.error(
            "SQL",
            format!("{location}: unterminated string literal"),
        );
    }

    if expr.trim().is_empty() {
        result.error("SQL", format!("{location}: empty expression"));
    }
}

// ──────────────────────────────────────────────────────────────────────
// Pass 8 — Origin/cluster rules
// ──────────────────────────────────────────────────────────────────────

fn pass_origin_cluster(doc: &MappingDocument, result: &mut ValidationResult) {
    // 8a: Error when a mapping declares both cluster_members and cluster_field.
    for m in &doc.mappings {
        if m.cluster_members.is_some() && m.cluster_field.is_some() {
            result.error(
                "Cluster",
                format!(
                    "mapping '{}': cannot declare both 'cluster_members' and 'cluster_field'",
                    m.name
                ),
            );
        }
    }

    // 8b: Warn when target has 2+ identity fields and insert-producing mappings
    // (multi-value hazard).
    for (tname, tdef) in &doc.targets {
        let identity_count = tdef
            .fields
            .values()
            .filter(|f| f.strategy() == Strategy::Identity)
            .count();
        if identity_count >= 2 {
            // Check if any mapping for this target could produce inserts
            // (i.e. another mapping targets the same target from a different source).
            let mapping_count = doc
                .mappings
                .iter()
                .filter(|m| m.target.name() == tname && m.has_fields())
                .count();
            if mapping_count >= 2 {
                result.warning(
                    "Cluster",
                    format!(
                        "target '{tname}': {identity_count} identity fields with {mapping_count} mappings — \
                         multi-value hazard: inserts may create synthetic composites. \
                         Consider using _cluster_id feedback instead."
                    ),
                );
            }
        }
    }

    // 8c: Info when links is present without link_key — batch-safe only.
    for m in &doc.mappings {
        if m.has_links() && m.link_key.is_none() {
            result.warning(
                "Cluster",
                format!(
                    "mapping '{}': links without link_key is batch-safe only; \
                     add link_key for IVM safety",
                    m.name
                ),
            );
        }
    }

    // 8d: Warn when links without link_key is used but no insert-producing
    // mapping for the same target has cluster_members or cluster_field.
    for m in &doc.mappings {
        if !m.has_links() || m.link_key.is_some() {
            continue;
        }
        let tname = m.target.name();
        let has_feedback = doc.mappings.iter().any(|other| {
            other.target.name() == tname
                && other.has_fields()
                && (other.cluster_members.is_some() || other.cluster_field.is_some())
        });
        if !has_feedback {
            result.warning(
                "Cluster",
                format!(
                    "mapping '{}': links without link_key targets '{tname}' but no mapping \
                     for that target declares cluster_members or cluster_field for insert feedback",
                    m.name
                ),
            );
        }
    }

    // 8e: Warn when a links mapping also has fields (unusual but allowed).
    for m in &doc.mappings {
        if m.has_links() && m.has_fields() {
            result.warning(
                "Cluster",
                format!(
                    "mapping '{}': has both links and fields — \
                     this is unusual; typically link mappings are linkage-only",
                    m.name
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use std::path::PathBuf;

    fn examples_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples")
    }

    #[test]
    fn validate_all_examples_pass() {
        let examples = examples_dir();
        let mut total_errors = 0;
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
            let name = entry.file_name().to_string_lossy().to_string();
            match parser::parse_file(&mapping) {
                Ok(doc) => {
                    let result = validate(&doc);
                    if result.has_errors() {
                        let errs: Vec<String> =
                            result.errors().map(|d| d.to_string()).collect();
                        failures.push(format!("{name}: {}", errs.join("; ")));
                        total_errors += result.error_count();
                    }
                }
                Err(e) => {
                    failures.push(format!("{name}: parse error: {e:#}"));
                    total_errors += 1;
                }
            }
        }

        if !failures.is_empty() {
            panic!(
                "{total_errors} validation error(s):\n{}",
                failures.join("\n")
            );
        }
    }

    #[test]
    fn validate_hello_world() {
        let yaml = std::fs::read_to_string(examples_dir().join("hello-world/mapping.yaml"))
            .unwrap();
        let doc = parser::parse_str(&yaml).unwrap();
        let result = validate(&doc);
        assert!(
            !result.has_errors(),
            "hello-world should have no errors: {:?}",
            result.errors().collect::<Vec<_>>()
        );
    }

    #[test]
    fn detect_duplicate_mapping_name() {
        let yaml = r#"
version: "1.0"
targets:
  contact:
    fields:
      email:
        strategy: identity
      name:
        strategy: coalesce
mappings:
  - name: crm
    source: { dataset: crm }
    target: contact
    fields:
      - { source: email, target: email }
  - name: crm
    source: { dataset: crm2 }
    target: contact
    fields:
      - { source: email, target: email }
"#;
        let doc = parser::parse_str(yaml).unwrap();
        let result = validate(&doc);
        assert!(result.has_errors());
        let msgs: Vec<String> = result.errors().map(|d| d.message.clone()).collect();
        assert!(
            msgs.iter().any(|m| m.contains("'crm'") && m.contains("2 times")),
            "expected duplicate name error, got: {msgs:?}"
        );
    }

    #[test]
    fn detect_invalid_target_ref() {
        let yaml = r#"
version: "1.0"
targets:
  contact:
    fields:
      email:
        strategy: identity
mappings:
  - name: crm
    source: { dataset: crm }
    target: nonexistent
    fields:
      - { source: email, target: email }
"#;
        let doc = parser::parse_str(yaml).unwrap();
        let result = validate(&doc);
        assert!(result.has_errors());
        let msgs: Vec<String> = result.errors().map(|d| d.message.clone()).collect();
        assert!(
            msgs.iter().any(|m| m.contains("nonexistent") && m.contains("not found")),
            "expected target ref error, got: {msgs:?}"
        );
    }

    #[test]
    fn detect_field_not_in_target() {
        let yaml = r#"
version: "1.0"
targets:
  contact:
    fields:
      email:
        strategy: identity
mappings:
  - name: crm
    source: { dataset: crm }
    target: contact
    fields:
      - { source: email, target: email }
      - { source: phone, target: phone_number }
"#;
        let doc = parser::parse_str(yaml).unwrap();
        let result = validate(&doc);
        assert!(result.has_errors());
        let msgs: Vec<String> = result.errors().map(|d| d.message.clone()).collect();
        assert!(
            msgs.iter().any(|m| m.contains("phone_number") && m.contains("not found")),
            "expected field coverage error, got: {msgs:?}"
        );
    }

    #[test]
    fn warn_on_orphan_target_field() {
        let yaml = r#"
version: "1.0"
targets:
  contact:
    fields:
      email:
        strategy: identity
      name:
        strategy: coalesce
      phone:
        strategy: coalesce
mappings:
  - name: crm
    source: { dataset: crm }
    target: contact
    fields:
      - { source: email, target: email }
      - { source: name, target: name, priority: 1 }
"#;
        let doc = parser::parse_str(yaml).unwrap();
        let result = validate(&doc);
        assert!(!result.has_errors());
        let warns: Vec<String> = result.warnings().map(|d| d.message.clone()).collect();
        assert!(
            warns.iter().any(|m| m.contains("phone") && m.contains("no mapping")),
            "expected orphan field warning, got: {warns:?}"
        );
    }

    #[test]
    fn detect_unbalanced_sql() {
        let yaml = r#"
version: "1.0"
targets:
  contact:
    fields:
      email:
        strategy: identity
      name:
        strategy: expression
        expression: "max(name"
mappings:
  - name: crm
    source: { dataset: crm }
    target: contact
    fields:
      - { source: email, target: email }
      - { source: name, target: name }
"#;
        let doc = parser::parse_str(yaml).unwrap();
        let result = validate(&doc);
        assert!(result.has_errors());
        let msgs: Vec<String> = result.errors().map(|d| d.message.clone()).collect();
        assert!(
            msgs.iter().any(|m| m.contains("parenthes")),
            "expected SQL syntax error, got: {msgs:?}"
        );
    }
}
