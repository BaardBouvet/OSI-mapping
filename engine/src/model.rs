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
        let col_ref = |col: &str| match row_alias {
            Some(alias) => format!("{alias}.{col}"),
            None => col.to_string(),
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
                vec![format!("{src_alias}._src_id AS {col}")]
            }
            PrimaryKey::Composite(cols) => {
                let mut sorted: Vec<&str> = cols.iter().map(|c| c.as_str()).collect();
                sorted.sort();
                sorted
                    .iter()
                    .map(|col| format!("({src_alias}._src_id::jsonb->>'{col}') AS {col}"))
                    .collect()
            }
        }
    }

    pub fn src_missing_predicate(&self, row_alias: Option<&str>) -> String {
        let col_ref = |col: &str| match row_alias {
            Some(alias) => format!("{alias}.{col}"),
            None => col.to_string(),
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
    pub fields: IndexMap<String, TargetField>,
}

/// A target field — either a shorthand string or a full definition.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum TargetField {
    Shorthand(Strategy),
    Full(TargetFieldDef),
}

impl TargetField {
    pub fn strategy(&self) -> Strategy {
        match self {
            TargetField::Shorthand(s) => *s,
            TargetField::Full(f) => f.strategy,
        }
    }

    pub fn references(&self) -> Option<&str> {
        match self {
            TargetField::Shorthand(_) => None,
            TargetField::Full(f) => f.references.as_deref(),
        }
    }

    pub fn group(&self) -> Option<&str> {
        match self {
            TargetField::Shorthand(_) => None,
            TargetField::Full(f) => f.group.as_deref(),
        }
    }

    pub fn link_group(&self) -> Option<&str> {
        match self {
            TargetField::Shorthand(_) => None,
            TargetField::Full(f) => f.link_group.as_deref(),
        }
    }

    pub fn expression(&self) -> Option<&str> {
        match self {
            TargetField::Shorthand(_) => None,
            TargetField::Full(f) => f.expression.as_deref(),
        }
    }

    pub fn default_value(&self) -> Option<&serde_yaml::Value> {
        match self {
            TargetField::Shorthand(_) => None,
            TargetField::Full(f) => f.default.as_ref(),
        }
    }

    pub fn default_expression(&self) -> Option<&str> {
        match self {
            TargetField::Shorthand(_) => None,
            TargetField::Full(f) => f.default_expression.as_deref(),
        }
    }
}

/// Full target field definition.
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
    pub include_base: bool,
    pub fields: Vec<FieldMapping>,
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
    #[serde(default)]
    pub description: Option<String>,
}

impl FieldMapping {
    /// Effective direction considering defaults.
    pub fn effective_direction(&self) -> Direction {
        self.direction.unwrap_or_else(|| {
            if self.source.is_some() {
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
