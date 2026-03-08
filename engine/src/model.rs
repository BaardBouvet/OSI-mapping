use indexmap::IndexMap;
use serde::Deserialize;

/// Top-level mapping document.
#[derive(Debug, Deserialize)]
pub struct MappingDocument {
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub targets: IndexMap<String, Target>,
    #[serde(default)]
    pub mappings: Vec<Mapping>,
    #[serde(default)]
    pub tests: Vec<TestCase>,
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
