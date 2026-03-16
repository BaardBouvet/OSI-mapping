use indexmap::IndexMap;
use serde::Deserialize;

/// Top-level mapping document.
#[derive(Debug, Deserialize)]
pub struct MappingDocument {
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sources: IndexMap<String, Source>,
    #[serde(default)]
    pub targets: IndexMap<String, Target>,
    #[serde(default)]
    pub mappings: Vec<Mapping>,
    #[serde(default)]
    pub tests: Vec<TestCase>,
}

/// Source dataset metadata.
#[derive(Debug, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub table: Option<String>,
    pub primary_key: PrimaryKey,
    /// Optional per-column metadata (e.g., type declarations).
    #[serde(default)]
    pub fields: IndexMap<String, SourceFieldDef>,
}

/// Per-column metadata on a source dataset.
#[derive(Debug, Deserialize)]
pub struct SourceFieldDef {
    /// SQL type for this column (e.g., "integer", "numeric").
    /// Used to cast PK columns in the reverse view when no target field type
    /// can be inferred.
    #[serde(default, rename = "type")]
    pub field_type: Option<String>,
}

/// Primary key representation: single column or composite key.
///
/// Deserialization: a bare string yields `Single`; a list yields `Composite`.
/// A single-element list `["id"]` is normalized to `Single("id")` so that
/// `primary_key: id` and `primary_key: [id]` produce identical SQL.
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "PrimaryKeyRaw")]
pub enum PrimaryKey {
    Single(String),
    Composite(Vec<String>),
}

/// Raw deserialization target — normalized into `PrimaryKey` via `From`.
#[derive(Deserialize)]
#[serde(untagged)]
enum PrimaryKeyRaw {
    Single(String),
    List(Vec<String>),
}

impl From<PrimaryKeyRaw> for PrimaryKey {
    fn from(raw: PrimaryKeyRaw) -> Self {
        match raw {
            PrimaryKeyRaw::Single(s) => PrimaryKey::Single(s),
            PrimaryKeyRaw::List(v) if v.len() == 1 => PrimaryKey::Single(v.into_iter().next().unwrap()),
            PrimaryKeyRaw::List(v) => PrimaryKey::Composite(v),
        }
    }
}

impl PrimaryKey {
    pub fn columns(&self) -> Vec<&str> {
        match self {
            PrimaryKey::Single(col) => vec![col.as_str()],
            PrimaryKey::Composite(cols) => cols.iter().map(|c| c.as_str()).collect(),
        }
    }

    pub fn src_id_expr(&self, row_alias: Option<&str>) -> String {
        let col_ref = |col: &str| {
            let qc = crate::qi(col);
            match row_alias {
                Some(alias) => format!("{alias}.{qc}"),
                None => qc,
            }
        };

        match self {
            PrimaryKey::Single(col) => format!("{}::text", col_ref(col)),
            PrimaryKey::Composite(cols) => {
                // Sort alphabetically for deterministic JSONB key order
                let mut sorted: Vec<&str> = cols.iter().map(|c| c.as_str()).collect();
                sorted.sort();
                let mut parts = Vec::new();
                for col in sorted {
                    parts.push(format!("'{}'", col.replace('\'', "''")));
                    parts.push(col_ref(col));
                }
                format!("jsonb_build_object({})::text", parts.join(", "))
            }
        }
    }

    /// Generate SELECT expressions that restore original PK columns from `_src_id`.
    ///
    /// For a single PK `contact_id`:  `id._src_id AS contact_id`
    /// For composite PK `[order_id, line_no]`:
    ///   `(id._src_id::jsonb->>'line_no') AS line_no`,
    ///   `(id._src_id::jsonb->>'order_id') AS order_id`
    pub fn reverse_select_exprs(&self, src_alias: &str) -> Vec<String> {
        match self {
            PrimaryKey::Single(col) => {
                vec![format!("{src_alias}._src_id AS {}", crate::qi(col))]
            }
            PrimaryKey::Composite(cols) => {
                let mut sorted: Vec<&str> = cols.iter().map(|c| c.as_str()).collect();
                sorted.sort();
                sorted
                    .iter()
                    .map(|col| format!("({src_alias}._src_id::jsonb->>'{col}') AS {}", crate::qi(col)))
                    .collect()
            }
        }
    }

    pub fn src_missing_predicate(&self, row_alias: Option<&str>) -> String {
        let col_ref = |col: &str| {
            let qc = crate::qi(col);
            match row_alias {
                Some(alias) => format!("{alias}.{qc}"),
                None => qc,
            }
        };

        let cols = self.columns();
        if cols.len() == 1 {
            format!("{} IS NULL", col_ref(cols[0]))
        } else {
            cols.iter()
                .map(|c| format!("{} IS NULL", col_ref(c)))
                .collect::<Vec<_>>()
                .join(" AND ")
        }
    }
}

impl Source {
    pub fn table_name<'a>(&'a self, key: &'a str) -> &'a str {
        self.table.as_deref().unwrap_or(key)
    }
}

/// A target entity definition.
#[derive(Debug, Deserialize)]
pub struct Target {
    #[serde(default)]
    pub description: Option<String>,
    pub fields: IndexMap<String, TargetFieldDef>,
}

/// Target field definition.
#[derive(Debug, Deserialize)]
pub struct TargetFieldDef {
    pub strategy: Strategy,
    #[serde(default)]
    pub expression: Option<String>,
    #[serde(default)]
    pub references: Option<String>,
    #[serde(default)]
    pub default: Option<serde_yaml::Value>,
    #[serde(default)]
    pub default_expression: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub link_group: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "type")]
    pub field_type: Option<String>,
}

impl TargetFieldDef {
    pub fn strategy(&self) -> Strategy {
        self.strategy
    }

    pub fn references(&self) -> Option<&str> {
        self.references.as_deref()
    }

    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    pub fn link_group(&self) -> Option<&str> {
        self.link_group.as_deref()
    }

    pub fn expression(&self) -> Option<&str> {
        self.expression.as_deref()
    }

    pub fn default_value(&self) -> Option<&serde_yaml::Value> {
        self.default.as_ref()
    }

    pub fn default_expression(&self) -> Option<&str> {
        self.default_expression.as_deref()
    }

    /// Optional SQL type for this field (e.g. "numeric", "boolean", "date").
    /// When set, forward views cast to this type instead of text.
    pub fn field_type(&self) -> Option<&str> {
        self.field_type.as_deref()
    }
}

/// Resolution strategy enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    Identity,
    Collect,
    Coalesce,
    LastModified,
    Expression,
}

/// A source-to-target mapping.
#[derive(Debug, Deserialize)]
pub struct Mapping {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub source: SourceRef,
    pub target: TargetRef,
    #[serde(default)]
    pub embedded: bool,
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub last_modified: Option<TimestampRef>,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub reverse_filter: Option<String>,
    #[serde(default)]
    pub fields: Vec<FieldMapping>,
    /// Column in the linking table whose value serves as cluster identity.
    /// Enables the IVM-safe path (forward-view LEFT JOIN on cluster members).
    #[serde(default)]
    pub link_key: Option<String>,
    /// External identity edges — links to other source mappings.
    #[serde(default)]
    pub links: Vec<LinkRef>,
    /// ETL feedback via a per-mapping table. `true` uses defaults;
    /// an object overrides table/column names.
    #[serde(default)]
    pub cluster_members: Option<ClusterMembers>,
    /// Column in the source table holding a pre-populated cluster ID.
    #[serde(default)]
    pub cluster_field: Option<String>,
}

impl Mapping {
    /// Whether this mapping contributes forward data (fields) to the target.
    pub fn has_fields(&self) -> bool {
        !self.fields.is_empty()
    }

    /// Whether this mapping has any field that participates in reverse mapping.
    /// True when at least one field is Bidirectional or ReverseOnly.
    pub fn has_reverse_fields(&self) -> bool {
        self.fields.iter().any(|f| f.is_reverse())
    }

    /// Whether to generate sync (reverse + delta) views for this mapping.
    /// True when any field is Bidirectional or ReverseOnly.
    pub fn needs_sync(&self) -> bool {
        self.has_reverse_fields()
    }

    /// Whether this mapping contributes identity edges via links.
    pub fn has_links(&self) -> bool {
        !self.links.is_empty()
    }

    /// Whether this mapping is linkage-only (links but no fields).
    pub fn is_linkage_only(&self) -> bool {
        self.has_links() && !self.has_fields()
    }
}

/// A link reference — connects a field in a linking table to a source mapping.
#[derive(Debug, Deserialize)]
pub struct LinkRef {
    /// Column(s) in the linking table referencing the target source's PK.
    pub field: LinkField,
    /// Name of the source mapping being referenced.
    pub references: String,
}

/// Link field — single column name or composite key mapping.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum LinkField {
    /// Single column: `field: crm_id`
    Single(String),
    /// Composite, same-name columns: `field: [order_id, line_no]`
    List(Vec<String>),
    /// Composite, renamed columns: `field: { src_col: pk_col, ... }`
    Map(IndexMap<String, String>),
}

impl LinkField {
    /// Pairs of (link_column, pk_column).
    pub fn column_pairs(&self, referenced_pk: &PrimaryKey) -> Vec<(String, String)> {
        match self {
            LinkField::Single(col) => {
                vec![(col.clone(), referenced_pk.columns()[0].to_string())]
            }
            LinkField::List(cols) => {
                let pk_cols = referenced_pk.columns();
                cols.iter()
                    .zip(pk_cols.iter())
                    .map(|(l, p)| (l.clone(), p.to_string()))
                    .collect()
            }
            LinkField::Map(map) => {
                map.iter().map(|(l, p)| (l.clone(), p.clone())).collect()
            }
        }
    }
}

/// ETL feedback configuration — per-mapping cluster membership table.
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "ClusterMembersRaw")]
pub struct ClusterMembers {
    /// Table name. Default: `_cluster_members_{mapping_name}`.
    pub table: Option<String>,
    /// Cluster ID column. Default: `_cluster_id`.
    pub cluster_id: String,
    /// Source key column. Default: `_src_id`.
    pub source_key: String,
}

/// Raw deserialization target for `cluster_members: true | { ... }`.
#[derive(Deserialize)]
#[serde(untagged)]
enum ClusterMembersRaw {
    Bool(bool),
    Full {
        #[serde(default)]
        table: Option<String>,
        #[serde(default = "default_cluster_id")]
        cluster_id: String,
        #[serde(default = "default_src_id")]
        source_key: String,
    },
}

fn default_cluster_id() -> String { "_cluster_id".to_string() }
fn default_src_id() -> String { "_src_id".to_string() }

impl From<ClusterMembersRaw> for ClusterMembers {
    fn from(raw: ClusterMembersRaw) -> Self {
        match raw {
            ClusterMembersRaw::Bool(true) => ClusterMembers {
                table: None,
                cluster_id: "_cluster_id".to_string(),
                source_key: "_src_id".to_string(),
            },
            ClusterMembersRaw::Bool(false) => ClusterMembers {
                table: None,
                cluster_id: "_cluster_id".to_string(),
                source_key: "_src_id".to_string(),
            },
            ClusterMembersRaw::Full { table, cluster_id, source_key } => ClusterMembers {
                table,
                cluster_id,
                source_key,
            },
        }
    }
}

impl ClusterMembers {
    /// Resolved table name — uses the default if not specified.
    pub fn table_name(&self, mapping_name: &str) -> String {
        self.table.clone().unwrap_or_else(|| format!("_cluster_members_{mapping_name}"))
    }
}

/// Source dataset reference.
#[derive(Debug, Deserialize)]
pub struct SourceRef {
    pub dataset: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub parent_fields: IndexMap<String, ParentFieldRef>,
}

/// Target reference — string name or dataset ref.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum TargetRef {
    Name(String),
    Dataset { dataset: String },
}

impl TargetRef {
    pub fn name(&self) -> &str {
        match self {
            TargetRef::Name(n) => n,
            TargetRef::Dataset { dataset } => dataset,
        }
    }
}

/// A single field mapping.
#[derive(Debug, Deserialize)]
pub struct FieldMapping {
    #[serde(default)]
    pub source: Option<String>,
    /// Dotted path into a JSONB column (e.g. `metadata.tier`).
    /// First segment is the column name, rest navigates JSON keys.
    /// Mutually exclusive with `source`.
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub direction: Option<Direction>,
    #[serde(default)]
    pub expression: Option<String>,
    #[serde(default)]
    pub reverse_expression: Option<String>,
    #[serde(default)]
    pub reverse_required: bool,
    #[serde(default)]
    pub last_modified: Option<TimestampRef>,
    #[serde(default)]
    pub priority: Option<i64>,
    /// Name of the mapping whose source identities should be used when
    /// translating an entity reference back to a source FK in the reverse view.
    #[serde(default)]
    pub references: Option<String>,
    /// When set, the reverse view returns this field's value from the referenced
    /// target instead of `_src_id`. Used for vocabulary-style references where
    /// the source FK stores a specific field (e.g., `iso_code`) rather than the
    /// referenced entity's primary key.
    #[serde(default)]
    pub references_field: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

impl FieldMapping {
    /// Effective direction considering defaults.
    pub fn effective_direction(&self) -> Direction {
        self.direction.unwrap_or_else(|| {
            if self.source.is_some() || self.source_path.is_some() {
                Direction::Bidirectional
            } else {
                Direction::ForwardOnly
            }
        })
    }

    /// Whether this field participates in forward mapping.
    pub fn is_forward(&self) -> bool {
        matches!(
            self.effective_direction(),
            Direction::Bidirectional | Direction::ForwardOnly
        )
    }

    /// Whether this field participates in reverse mapping.
    pub fn is_reverse(&self) -> bool {
        matches!(
            self.effective_direction(),
            Direction::Bidirectional | Direction::ReverseOnly
        )
    }

    /// Logical source identity — used as `_base` key and reverse view column alias.
    /// Returns the full dotted `source_path` if set, else `source`.
    pub fn source_name(&self) -> Option<&str> {
        self.source_path.as_deref().or(self.source.as_deref())
    }

    /// Physical source column in the source table.
    /// For `source_path`, this is the first segment (the JSONB column),
    /// stripping any bracket suffix (e.g. `contacts[0].email` → `contacts`).
    /// For `source`, this is the column name itself.
    pub fn source_column(&self) -> Option<&str> {
        if let Some(ref sp) = self.source_path {
            let first = sp.split('.').next().unwrap_or(sp);
            // Strip bracket suffix: "contacts[0]" → "contacts"
            Some(first.split('[').next().unwrap_or(first))
        } else {
            self.source.as_deref()
        }
    }
}

/// Mapping direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Bidirectional,
    ForwardOnly,
    ReverseOnly,
}

/// Timestamp reference — string field name or structured.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum TimestampRef {
    Field(String),
    Structured {
        #[serde(default)]
        field: Option<String>,
        #[serde(default)]
        expression: Option<String>,
    },
}

impl TimestampRef {
    pub fn field_name(&self) -> Option<&str> {
        match self {
            TimestampRef::Field(f) => Some(f),
            TimestampRef::Structured { field, .. } => field.as_deref(),
        }
    }

    pub fn expression(&self) -> Option<&str> {
        match self {
            TimestampRef::Field(_) => None,
            TimestampRef::Structured { expression, .. } => expression.as_deref(),
        }
    }
}

/// Parent field reference for nested arrays.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ParentFieldRef {
    Simple(String),
    Qualified {
        #[serde(default)]
        path: Option<String>,
        field: String,
    },
}

/// A test case.
#[derive(Debug, Deserialize)]
pub struct TestCase {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input: IndexMap<String, Vec<serde_json::Value>>,
    #[serde(default)]
    pub expected: IndexMap<String, TestExpected>,
    /// Expected analytics (target) view contents, keyed by target name.
    #[serde(default)]
    pub analytics: IndexMap<String, Vec<serde_json::Value>>,
}

/// Expected output for a single source dataset.
#[derive(Debug, Deserialize)]
pub struct TestExpected {
    #[serde(default)]
    pub updates: Vec<serde_json::Value>,
    #[serde(default)]
    pub inserts: Vec<serde_json::Value>,
    #[serde(default)]
    pub deletes: Vec<serde_json::Value>,
}
