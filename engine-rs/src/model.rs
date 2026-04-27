//! v2 mapping model.
//!
//! This is the minimum viable schema for the hello-world slice. As each
//! conformance example comes online we extend this in lockstep with the
//! parser and validators.

use indexmap::IndexMap;
use serde::Deserialize;

/// Top-level mapping document.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Doc {
    /// Schema version. Must be `"2.0"`.
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    pub sources: IndexMap<String, Source>,
    pub targets: IndexMap<String, Target>,
    pub mappings: Vec<Mapping>,
    #[serde(default)]
    pub tests: Vec<Test>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Primary key field name. Single field for hello-world; later slices
    /// will accept a list for composite keys.
    pub primary_key: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// OR-list of identity components. Each component is either a single
    /// field name or an AND-tuple (list of field names). For hello-world we
    /// only emit single-field identities.
    pub identity: Vec<IdentityGroup>,
    pub fields: IndexMap<String, Field>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum IdentityGroup {
    /// Single field identity: `- email`.
    Single(String),
    /// AND-tuple identity: `- [first_name, last_name, dob]`.
    Tuple(Vec<String>),
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub strategy: Strategy,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Pick the highest-priority non-null value (priority ascending,
    /// then declaration order).
    Coalesce,
    /// Pick the value from the row with the highest `last_modified`
    /// timestamp; ties broken by declaration order. Mappings
    /// contributing to a `last_modified` field must declare a
    /// `last_modified:` source-field on the mapping itself; mappings
    /// without one contribute as if their timestamp were NULL (and
    /// therefore lose to any timestamped candidate).
    LastModified,
    // Future: MultiValue, AnyTrue, AllTrue, Expression
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub name: String,
    /// Source dataset name. Slice 3-foundation: still required even for
    /// nested mappings (the v2 spec lets `source:` be inherited from
    /// `parent:` but slice 3a will lift that restriction).
    pub source: String,
    pub target: String,
    /// Source-field name carrying the row's last-modified timestamp.
    /// Required if the target has any field with `strategy: last_modified`
    /// for which this mapping contributes; mappings without it contribute
    /// as NULL-timestamp losers.
    #[serde(default)]
    pub last_modified: Option<String>,
    /// Slice 3: nested mappings. Names another mapping that is this
    /// mapping's parent in the lift chain. Accepted by the parser but
    /// not yet rendered by either backend.
    #[serde(default)]
    pub parent: Option<String>,
    /// Slice 3: source-path expression naming an array column / dotted
    /// path to expand. One element of that array becomes one logical
    /// row of this mapping. Accepted by the parser but not yet rendered.
    #[serde(default)]
    pub array: Option<String>,
    /// Slice 3: aliases that bring parent-row columns into element scope.
    /// Map element-side alias → parent-side source column.
    #[serde(default)]
    pub parent_fields: IndexMap<String, String>,
    pub fields: Vec<FieldMap>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct FieldMap {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub priority: Option<i32>,
    /// Slice 4: name of another target whose canonical IRI should resolve
    /// this field on reverse (foreign-key-style cross-entity reference).
    /// Accepted by the parser but not yet rendered.
    #[serde(default)]
    pub references: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests block (conformance contract)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Test {
    pub description: String,
    /// Source name → list of input rows (each row is an arbitrary YAML map).
    pub input: IndexMap<String, Vec<serde_yaml::Value>>,
    /// Source name → expected outcomes.
    pub expected: IndexMap<String, ExpectedOutcomes>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ExpectedOutcomes {
    #[serde(default)]
    pub updates: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub inserts: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub deletes: Vec<serde_yaml::Value>,
}
