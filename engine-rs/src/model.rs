use indexmap::IndexMap;
use serde::{Deserialize, Deserializer};

use crate::qi;

/// Top-level mapping document.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
            PrimaryKeyRaw::List(v) if v.len() == 1 => {
                PrimaryKey::Single(v.into_iter().next().unwrap())
            }
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
                    .map(|col| {
                        format!(
                            "({src_alias}._src_id::jsonb->>'{col}') AS {}",
                            crate::qi(col)
                        )
                    })
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
#[serde(deny_unknown_fields)]
pub struct Target {
    #[serde(default)]
    pub description: Option<String>,
    pub fields: IndexMap<String, TargetFieldDef>,
}

/// Target field definition.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    BoolOr,
}

/// A source-to-target mapping.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub source: SourceRef,
    pub target: TargetRef,
    /// Name of the parent mapping. Child inherits source from parent.
    #[serde(default)]
    pub parent: Option<String>,
    /// JSONB array column to expand into rows (single segment). Requires parent.
    #[serde(default)]
    pub array: Option<String>,
    /// Dotted path to a JSONB array to expand. Requires parent.
    #[serde(default)]
    pub array_path: Option<String>,
    /// Map of local field aliases to parent column names. Promoted from source.
    #[serde(default)]
    pub parent_fields: IndexMap<String, ParentFieldRef>,
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
    /// ETL written-state table for target-centric noop detection.
    /// `true` uses defaults; an object overrides table/column names.
    #[serde(default)]
    pub written_state: Option<WrittenState>,
    /// When true and `written_state` is declared, the delta CASE compares
    /// resolved fields against the last-written values for noop detection.
    /// Off by default — opt-in because it assumes the ETL is the sole
    /// writer to the target.
    #[serde(default)]
    pub derive_noop: bool,
    /// When true and `written_state` is declared, the engine derives
    /// tombstones by comparing written state against current state.  This
    /// enriches the source data with deletion information the source
    /// system doesn't provide natively:
    /// - **Entity level:** entities in `_written` but absent from the
    ///   source are treated as hard-deleted (suppressed when `resurrect`
    ///   is false).
    /// - **Element level:** elements in the written JSONB array but absent
    ///   from the forward view are excluded from all sources' arrays.
    ///
    /// Off by default — opt-in because the written JSONB stores the merged
    /// output, not per-source contributions, which can cause false
    /// tombstones when sources contribute different elements.
    #[serde(default)]
    pub derive_tombstones: bool,
    /// When true and `written_state` is declared, the forward view derives
    /// per-field `_ts_{field}` timestamps by comparing current source values
    /// against `_written` JSONB. Fields that changed get `_written_at`;
    /// unchanged fields carry forward their timestamp from `_written_ts`.
    /// On bootstrap (no `_written_ts` entry), timestamps are NULL.
    #[serde(default)]
    pub derive_timestamps: bool,
    /// Source columns to carry through to delta output without mapping to a
    /// target field. Included in `_base` and reverse/delta but excluded from
    /// noop detection and resolution.
    #[serde(default)]
    pub passthrough: Vec<String>,
    /// Whether to resurrect entities that disappeared from this source.
    /// When `false` (default) and a detection mechanism is available
    /// (`cluster_members` or `derive_tombstones` + `written_state`), entities
    /// that were previously synced but are now absent are excluded from the
    /// delta instead of being re-inserted.  Set to `true` to allow
    /// re-insertion (opt out of hard-delete detection).
    ///
    /// For soft deletes (`tombstone`), `resurrect: true` enables
    /// undelete: the delta emits `'update'` with the undelete values, telling
    /// the ETL to clear the soft-delete marker.
    #[serde(default)]
    pub resurrect: bool,
    /// Soft-delete detection configuration.
    #[serde(default)]
    pub tombstone: Option<Tombstone>,
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

    /// Whether this mapping is a child of another mapping.
    pub fn is_child(&self) -> bool {
        self.parent.is_some()
    }

    /// Whether this mapping expands a nested array.
    pub fn is_nested(&self) -> bool {
        self.array.is_some() || self.array_path.is_some()
    }

    /// The effective array path (from `array` or `array_path`).
    pub fn effective_array(&self) -> Option<&str> {
        self.array.as_deref().or(self.array_path.as_deref())
    }

    /// Whether to suppress resurrection of disappeared entities.
    ///
    /// Returns `true` when entity-level hard-delete detection is active
    /// AND `resurrect` is `false`.  Detection requires a persistence table:
    /// - `cluster_members` — ETL feedback table.
    /// - `derive_tombstones` + `written_state` — written-state table.
    ///
    /// Note: `tombstone` does NOT contribute here — tombstone
    /// suppression is independent and always active when set.
    pub fn suppress_resurrect(&self) -> bool {
        let has_detection = self.cluster_members.is_some()
            || (self.derive_tombstones && self.written_state.is_some());
        has_detection && !self.resurrect
    }

    /// Effective passthrough columns — explicit passthrough plus columns
    /// auto-included by tombstone (`field` + any `undelete` map keys).
    pub fn effective_passthrough(&self) -> Vec<&str> {
        let mut cols: Vec<&str> = self.passthrough.iter().map(|s| s.as_str()).collect();
        if let Some(ref ts) = self.tombstone {
            for col in ts.passthrough_columns() {
                if !cols.contains(&col) {
                    cols.push(col);
                }
            }
        }
        cols
    }
}

/// A link reference — connects a field in a linking table to a source mapping.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
            LinkField::Map(map) => map.iter().map(|(l, p)| (l.clone(), p.clone())).collect(),
        }
    }
}

// ── Tombstone (soft-delete detection) ──────────────────────────────────

/// Soft-delete detection configuration.
///
/// Declares a source column that signals deletion and how to detect /
/// reverse it.  `detect` and `undelete` are derived from `field` +
/// `default` when omitted, but can be overridden with raw SQL.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tombstone {
    /// Source column carrying the deletion signal.
    pub field: String,
    /// The default (non-deleted) value.  Used to derive `detect` and
    /// `undelete` when they are not specified explicitly.
    /// Defaults to null — i.e. `field IS NOT NULL` means deleted.
    #[serde(default)]
    pub default: TombstoneDefault,
    /// SQL boolean expression that evaluates to true when the entity is
    /// soft-deleted.  When omitted, derived from `field` + `default`.
    #[serde(default)]
    pub detect: Option<String>,
    /// SQL expression(s) to project when undeleting (resurrect: true).
    /// When omitted, the tombstone field is projected with the `default`
    /// value.  A string applies to the tombstone field only; a map
    /// overrides arbitrary columns (map keys auto-included as passthrough).
    #[serde(default)]
    pub undelete: Option<UndeleteProjection>,
}

impl Tombstone {
    /// SQL boolean expression: true when the entity is soft-deleted.
    pub fn detection_expr(&self) -> String {
        if let Some(ref expr) = self.detect {
            return expr.clone();
        }
        let field = qi(&self.field);
        match &self.default {
            TombstoneDefault::Null => format!("{field} IS NOT NULL"),
            other => format!("{field} IS DISTINCT FROM {}", other.to_sql()),
        }
    }

    /// Column → SQL expression pairs for undelete projection.
    /// Always includes the tombstone field itself; may include extras
    /// from the map form.
    pub fn undelete_overrides(&self) -> IndexMap<String, String> {
        match &self.undelete {
            None => {
                let mut m = IndexMap::new();
                m.insert(self.field.clone(), self.default.to_sql());
                m
            }
            Some(UndeleteProjection::Single(expr)) => {
                let mut m = IndexMap::new();
                m.insert(self.field.clone(), expr.clone());
                m
            }
            Some(UndeleteProjection::Multi(map)) => {
                let mut result = IndexMap::new();
                // Ensure the tombstone field itself is included (from default
                // if not explicitly in the map).
                if !map.contains_key(&self.field) {
                    result.insert(self.field.clone(), self.default.to_sql());
                }
                for (k, v) in map {
                    result.insert(k.clone(), v.clone());
                }
                result
            }
        }
    }

    /// Columns that must be auto-included as passthrough:
    /// the tombstone field + any extra map keys from `undelete`.
    pub fn passthrough_columns(&self) -> Vec<&str> {
        let mut cols = vec![self.field.as_str()];
        if let Some(UndeleteProjection::Multi(map)) = &self.undelete {
            for k in map.keys() {
                if k != &self.field && !cols.contains(&k.as_str()) {
                    cols.push(k.as_str());
                }
            }
        }
        cols
    }
}

/// Undelete projection — what value(s) to write back when undeleting.
#[derive(Debug, Clone)]
pub enum UndeleteProjection {
    /// Single SQL expression applied to the tombstone field.
    Single(String),
    /// Map of column → SQL expression for multi-column undelete.
    Multi(IndexMap<String, String>),
}

impl<'de> serde::Deserialize<'de> for UndeleteProjection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct UndeleteVisitor;

        impl<'de> serde::de::Visitor<'de> for UndeleteVisitor {
            type Value = UndeleteProjection;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string (SQL expression) or a map of column: expression")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<UndeleteProjection, E> {
                Ok(UndeleteProjection::Single(v.to_string()))
            }

            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<UndeleteProjection, E> {
                Ok(UndeleteProjection::Single(v))
            }

            fn visit_map<M>(self, mut access: M) -> Result<UndeleteProjection, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let mut map = IndexMap::new();
                while let Some((key, value)) = access.next_entry::<String, String>()? {
                    map.insert(key, value);
                }
                Ok(UndeleteProjection::Multi(map))
            }
        }

        deserializer.deserialize_any(UndeleteVisitor)
    }
}

// ── TombstoneDefault (soft-delete default value) ──────────────────────

/// The default (non-deleted) value for a tombstone field.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum TombstoneDefault {
    /// SQL NULL — the most common case (`deleted_at IS NOT NULL`).
    #[default]
    Null,
    /// SQL boolean literal (`is_deleted = false`).
    Bool(bool),
    /// SQL string literal (`status = 'active'`).
    String(String),
}

impl TombstoneDefault {
    /// Render as a SQL literal.
    pub fn to_sql(&self) -> String {
        match self {
            TombstoneDefault::Null => "NULL".to_string(),
            TombstoneDefault::Bool(true) => "TRUE".to_string(),
            TombstoneDefault::Bool(false) => "FALSE".to_string(),
            TombstoneDefault::String(s) => format!("'{}'", s.replace('\'', "''")),
        }
    }
}

impl<'de> serde::Deserialize<'de> for TombstoneDefault {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct TombstoneDefaultVisitor;

        impl<'de> serde::de::Visitor<'de> for TombstoneDefaultVisitor {
            type Value = TombstoneDefault;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("null, a boolean, or a string")
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<TombstoneDefault, E> {
                Ok(TombstoneDefault::Null)
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<TombstoneDefault, E> {
                Ok(TombstoneDefault::Null)
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<TombstoneDefault, E> {
                Ok(TombstoneDefault::Bool(v))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<TombstoneDefault, E> {
                Ok(TombstoneDefault::String(v.to_string()))
            }

            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<TombstoneDefault, E> {
                Ok(TombstoneDefault::String(v))
            }
        }

        deserializer.deserialize_any(TombstoneDefaultVisitor)
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

fn default_cluster_id() -> String {
    "_cluster_id".to_string()
}
fn default_src_id() -> String {
    "_src_id".to_string()
}

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
            ClusterMembersRaw::Full {
                table,
                cluster_id,
                source_key,
            } => ClusterMembers {
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
        self.table
            .clone()
            .unwrap_or_else(|| format!("_cluster_members_{mapping_name}"))
    }
}

/// ETL written-state table — stores field values the ETL last wrote to the target.
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "WrittenStateRaw")]
pub struct WrittenState {
    /// Table name. Default: `_written_{mapping_name}`.
    pub table: Option<String>,
    /// Cluster ID column. Default: `_cluster_id`.
    pub cluster_id: String,
    /// Written-state JSONB column. Default: `_written`.
    pub written: String,
    /// Row-level write timestamp column. Default: `_written_at`.
    pub written_at: String,
    /// Per-field timestamps JSONB column. Default: `_written_ts`.
    pub written_ts: String,
}

/// Raw deserialization target for `written_state: true | { ... }`.
#[derive(Deserialize)]
#[serde(untagged)]
enum WrittenStateRaw {
    Bool(bool),
    Full {
        #[serde(default)]
        table: Option<String>,
        #[serde(default = "default_cluster_id")]
        cluster_id: String,
        #[serde(default = "default_written")]
        written: String,
        #[serde(default = "default_written_at")]
        written_at: String,
        #[serde(default = "default_written_ts")]
        written_ts: String,
    },
}

fn default_written() -> String {
    "_written".to_string()
}

fn default_written_at() -> String {
    "_written_at".to_string()
}

fn default_written_ts() -> String {
    "_written_ts".to_string()
}

impl From<WrittenStateRaw> for WrittenState {
    fn from(raw: WrittenStateRaw) -> Self {
        match raw {
            WrittenStateRaw::Bool(true) => WrittenState {
                table: None,
                cluster_id: "_cluster_id".to_string(),
                written: "_written".to_string(),
                written_at: "_written_at".to_string(),
                written_ts: "_written_ts".to_string(),
            },
            WrittenStateRaw::Bool(false) => WrittenState {
                table: None,
                cluster_id: "_cluster_id".to_string(),
                written: "_written".to_string(),
                written_at: "_written_at".to_string(),
                written_ts: "_written_ts".to_string(),
            },
            WrittenStateRaw::Full {
                table,
                cluster_id,
                written,
                written_at,
                written_ts,
            } => WrittenState {
                table,
                cluster_id,
                written,
                written_at,
                written_ts,
            },
        }
    }
}

impl WrittenState {
    /// Resolved table name — uses the default if not specified.
    pub fn table_name(&self, mapping_name: &str) -> String {
        self.table
            .clone()
            .unwrap_or_else(|| format!("_written_{mapping_name}"))
    }
}

/// Source reference — deserialised from a plain string (the source name).
/// Internal fields `path` and `parent_fields` are populated by the parser.
#[derive(Debug, Default)]
pub struct SourceRef {
    pub dataset: String,
    pub path: Option<String>,
    pub parent_fields: IndexMap<String, ParentFieldRef>,
}

impl<'de> Deserialize<'de> for SourceRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        Ok(SourceRef {
            dataset: name,
            path: None,
            parent_fields: IndexMap::new(),
        })
    }
}

/// Target reference — plain string name.
#[derive(Debug)]
pub struct TargetRef(String);

impl TargetRef {
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TargetRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(TargetRef(String::deserialize(deserializer)?))
    }
}

/// A single field mapping.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// When true, auto-populates this field with a sortable position key
    /// derived from the array element's index (Tier 1 ordinal ordering).
    /// Mutually exclusive with `source`, `source_path`, and `expression`.
    /// Only valid on nested mappings (those with `parent:`/`source.path`).
    #[serde(default)]
    pub order: bool,
    /// When true, auto-populates with the identity of the previous sibling
    /// (Tier 2 linked-list CRDT ordering). Requires `order: true` and
    /// `order_next: true` on the same mapping.
    #[serde(default)]
    pub order_prev: bool,
    /// When true, auto-populates with the identity of the next sibling
    /// (Tier 2 linked-list CRDT ordering). Requires `order: true` and
    /// `order_prev: true` on the same mapping.
    #[serde(default)]
    pub order_next: bool,
    /// SQL expression with `%s` placeholder applied to both sides of the
    /// delta noop comparison.  Handles precision loss (e.g. numeric rounding,
    /// string truncation, case folding) so that expected lossy differences
    /// are not flagged as changes.
    #[serde(default)]
    pub normalize: Option<String>,
}

impl FieldMapping {
    /// Effective direction considering defaults.
    /// Order fields default to Bidirectional so they flow through to reverse/delta
    /// for array reconstruction ORDER BY.
    pub fn effective_direction(&self) -> Direction {
        self.direction.unwrap_or_else(|| {
            if self.order
                || self.order_prev
                || self.order_next
                || self.source.is_some()
                || self.source_path.is_some()
            {
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
#[derive(Debug, Clone, Deserialize)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct TestExpected {
    #[serde(default)]
    pub updates: Vec<serde_json::Value>,
    #[serde(default)]
    pub inserts: Vec<serde_json::Value>,
    #[serde(default)]
    pub deletes: Vec<serde_json::Value>,
    /// Rows expected to be noops — listed for documentation/assertion.
    /// Not yet consumed by the test harness (implicit noops are verified
    /// by checking that unlisted rows appear in the delta as noops).
    #[serde(default)]
    pub noops: Vec<serde_json::Value>,
}
