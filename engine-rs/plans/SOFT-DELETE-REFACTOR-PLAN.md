# Soft-delete refactor plan

**Status:** Proposed

Replace the overengineered `tombstone` object (5 properties, custom serde, mutual
exclusion validation) with a simple `soft_delete` that captures the only 3
strategies seen in practice.

## Current problems

The current surface (`tombstone` with `undelete_value`, `undelete_expression`,
`detect`, `undelete_columns`) was designed for maximum flexibility.  In practice:

1. **Only 3 real-world patterns exist** — nullable timestamp, boolean flag
   (deleted = true), inverted boolean flag (active = true).
2. **Wrong name** — "tombstone" is usually an internal term for durable delete
   markers.  The industry standard term is **soft delete**.
3. **Too many properties** — `undelete_value` vs `undelete_expression` with
   mutual exclusion, `detect` as an optional override, `undelete_columns` for
   multi-column undelete — all to support hypothetical edge cases.
4. **Hard for agents** — an LLM reading the schema must understand 5 interacting
   properties and their derivation rules.
5. **Bug: soft-deleted rows win resolution** — the forward view includes
   soft-deleted rows with all field values intact.  Tombstone detection only
   acts in the delta CASE, so a soft-deleted source still participates in
   field resolution and can push stale data to other systems.

## Proposed design

### YAML surface

```yaml
# Nullable timestamp (most common) — string shorthand
soft_delete: deleted_at

# Same thing, object form
soft_delete: { field: deleted_at }

# Boolean flag: deleted = true means deleted
soft_delete: { field: is_deleted, strategy: flag }

# Inverted boolean: active = true means NOT deleted
soft_delete: { field: is_active, strategy: active_flag }
```

### Strategy table

| Strategy       | Detection                     | Undelete value | Common fields            |
|----------------|-------------------------------|----------------|--------------------------|
| `timestamp`    | `"field" IS NOT NULL`         | `NULL`         | `deleted_at`, `removed_at` |
| `flag`         | `"field" IS NOT FALSE`        | `FALSE`        | `is_deleted`, `deleted`  |
| `active_flag`  | `"field" IS NOT TRUE`         | `TRUE`         | `is_active`, `active`    |

- `strategy` defaults to `timestamp` when omitted.
- Detection and undelete values are fully determined by the strategy — no
  overrides needed, no `detect`, no `undelete_expression`, no `undelete_columns`.

### Interaction with `resurrect`

Unchanged — behavior depends on `resurrect`:

| `resurrect` | Soft-delete detected | Action |
|---|---|---|
| `false` (default) | yes | Suppress (NULL action) |
| `true` | yes | Undelete (`'update'` action, project undelete value) |

### Serde

Two accepted forms via `#[serde(untagged)]` enum:

```rust
#[serde(untagged)]
enum SoftDeleteRaw {
    Short(String),                    // "deleted_at" → field + timestamp
    Full { field: String, strategy: Option<SoftDeleteStrategy> },
}
```

Converts to:

```rust
pub struct SoftDelete {
    pub field: String,
    pub strategy: SoftDeleteStrategy,
}
```

### `derive_tombstones`

Unchanged name — it refers to element-level deletion via written state, not to
the soft-delete concept.  Different mechanism, different purpose.

## What gets removed

| Removed | Reason |
|---|---|
| `Tombstone` struct | Replaced by `SoftDelete` |
| `TombstoneDefault` enum | Strategy subsumes value selection |
| `deser_some_tombstone_default` | No longer needed |
| `undelete_value` property | Derived from strategy |
| `undelete_expression` property | No real-world use case |
| `detect` property | Derived from strategy |
| `undelete_columns` property | No real-world use case |
| Exactly-one-of validation | No mutual exclusion to validate |

Net: ~160 lines removed from model.rs, ~40 lines removed from validate.rs.

## Implementation

### 1. model.rs

```rust
// ── Soft-delete detection ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoftDeleteStrategy {
    Timestamp,
    Flag,
    ActiveFlag,
}

pub struct SoftDelete {
    pub field: String,
    pub strategy: SoftDeleteStrategy,
}

impl SoftDelete {
    pub fn detection_expr(&self) -> String {
        let f = qi(&self.field);
        match self.strategy {
            SoftDeleteStrategy::Timestamp  => format!("{f} IS NOT NULL"),
            SoftDeleteStrategy::Flag       => format!("{f} IS NOT FALSE"),
            SoftDeleteStrategy::ActiveFlag => format!("{f} IS NOT TRUE"),
        }
    }

    pub fn undelete_value(&self) -> &'static str {
        match self.strategy {
            SoftDeleteStrategy::Timestamp  => "NULL",
            SoftDeleteStrategy::Flag       => "FALSE",
            SoftDeleteStrategy::ActiveFlag => "TRUE",
        }
    }

    /// Single-entry map: field → undelete value.
    pub fn undelete_overrides(&self) -> IndexMap<String, String> {
        let mut m = IndexMap::new();
        m.insert(self.field.clone(), self.undelete_value().to_string());
        m
    }

    pub fn passthrough_columns(&self) -> Vec<&str> {
        vec![self.field.as_str()]
    }
}
```

Custom serde via `SoftDeleteRaw` intermediate:

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum SoftDeleteRaw {
    Short(String),
    Full {
        field: String,
        #[serde(default)]
        strategy: Option<SoftDeleteStrategy>,
    },
}

impl From<SoftDeleteRaw> for SoftDelete {
    fn from(raw: SoftDeleteRaw) -> Self {
        match raw {
            SoftDeleteRaw::Short(f) => SoftDelete {
                field: f,
                strategy: SoftDeleteStrategy::Timestamp,
            },
            SoftDeleteRaw::Full { field, strategy } => SoftDelete {
                field,
                strategy: strategy.unwrap_or(SoftDeleteStrategy::Timestamp),
            },
        }
    }
}
```

On the Mapping struct:

```rust
// Rename field:
pub tombstone: Option<Tombstone>   →   pub soft_delete: Option<SoftDelete>
```

All `m.tombstone` references become `m.soft_delete`.

### 2. validate.rs

Remove the tombstone validation block (exactly-one-of, detect-required).
Keep the column-existence check, update the field reference:

```rust
if let Some(ref sd) = m.soft_delete {
    if !source_cols.is_empty() && !source_cols.contains(sd.field.as_str()) {
        result.warning("Column", format!(
            "mapping '{}' soft_delete.field: unknown source column '{}'",
            m.name, sd.field
        ));
    }
}
```

~30 lines removed net.

### 3. render/forward.rs — fix resolution bug

Soft-deleted rows must not win field resolution.  In the forward view,
wrap non-identity field projections in a CASE that NULLs them when the
soft-delete condition is true.  Identity fields keep their values (so
entities still link).

Before (broken — soft-deleted name wins priority):

```sql
"name"::text AS "name",
1 AS "_priority_name",
```

After (soft-deleted fields can't win):

```sql
CASE WHEN "deleted_at" IS NOT NULL THEN NULL ELSE "name"::text END AS "name",
CASE WHEN "deleted_at" IS NOT NULL THEN NULL ELSE 1 END AS "_priority_name",
```

The detection expression comes from `SoftDelete::detection_expr()`.  Apply
to every non-identity field projection and its associated metadata columns
(`_priority_*`, `_ts_*`).

The `_base` JSONB should still include all original values (for correct noop
detection in the delta — the source row hasn't changed, only its
participation changed).

### 4. render/delta.rs

All rendering code references `mapping.tombstone` — rename to `mapping.soft_delete`.
The `detection_expr()`, `undelete_overrides()`, and `passthrough_columns()` method
signatures are identical, so the rendering code needs only identifier renames.

In the comment marker:

```
// ── Tombstone (soft-delete detection) ──
→
// ── Soft-delete detection ──
```

### 5. Tests (delta.rs)

Update all ~15 tombstone tests:

- YAML property: `tombstone:` → `soft_delete:`
- Remove properties: `undelete_value:`, `undelete_expression:`, `detect:`
- String shorthand tests: `soft_delete: deleted_at`
- Object form: `soft_delete: { field: deleted_at }` (defaults to timestamp)
- Boolean flag: `soft_delete: { field: is_deleted, strategy: flag }`
- Active flag: `soft_delete: { field: is_active, strategy: active_flag }`
- Assertion messages: "tombstone" → "soft_delete"

Example test YAML before → after:

```yaml
# Before
tombstone: { field: deleted_at, undelete_value: null }

# After
soft_delete: deleted_at

# Before
tombstone:
  field: is_deleted
  undelete_value: false

# After
soft_delete: { field: is_deleted, strategy: flag }

# Before
tombstone:
  field: status
  detect: "status IN ('deleted', 'archived')"
  undelete_expression: "'active'"
  undelete_columns:
    deleted_at: "NULL"

# After — this test gets removed (no real-world use case).
# Or simplified to: soft_delete: { field: status, strategy: flag }
```

### 5. Schema (mapping-schema.json)

Replace the `Tombstone` definition:

```json
"SoftDelete": {
  "oneOf": [
    {
      "type": "string",
      "description": "Shorthand: field name. Strategy defaults to timestamp."
    },
    {
      "type": "object",
      "required": ["field"],
      "additionalProperties": false,
      "properties": {
        "field": {
          "type": "string",
          "description": "Source column carrying the deletion signal."
        },
        "strategy": {
          "type": "string",
          "enum": ["timestamp", "flag", "active_flag"],
          "default": "timestamp",
          "description": "Detection strategy."
        }
      }
    }
  ]
}
```

Mapping property: `"tombstone": { "$ref": "#/$defs/Tombstone" }` →
`"soft_delete": { "$ref": "#/$defs/SoftDelete" }`.

### 6. Docs (schema-reference.md)

Replace the `### tombstone` section with `### soft_delete`.

New property table:

| Property | Type | Required | Description |
|---|---|---|---|
| `field` | string | **yes** | Source column carrying the deletion signal |
| `strategy` | enum | no | `timestamp` (default), `flag`, or `active_flag` |

New strategy derivation table (same as above).

Update all YAML examples.

### 7. Example (soft-delete/)

Update `mapping.yaml` to use `soft_delete:` syntax and update `README.md`.

The example test scenario was fixed separately — see
[FIX-SOFT-DELETE-EXAMPLE-PLAN.md](FIX-SOFT-DELETE-EXAMPLE-PLAN.md).  Only the
property rename (`tombstone:` → `soft_delete:`) needs to happen here.

### 8. Example (soft-delete/)

Update `mapping.yaml` to use `soft_delete:` syntax and update `README.md`.

The example test scenario was fixed separately — see
[FIX-SOFT-DELETE-EXAMPLE-PLAN.md](FIX-SOFT-DELETE-EXAMPLE-PLAN.md).  Only the
property rename (`tombstone:` → `soft_delete:`) needs to happen here.

With the forward-view bug fixed (step 3), also add a test that exercises it:
soft-deleted CRM Alice with priority 1 name, ERP Alice with a different name —
ERP should NOT get an update because the soft-deleted source's fields are
excluded from resolution.

### 9. Plan (SOFT-DELETE-PLAN.md)

Rewrite to reflect simplified design (or delete and supersede with this plan).

### 10. Other docs with cosmetic references

These files mention "tombstone" in passing as a concept name.  Update text
only — no code changes:

- `examples/hard-delete/README.md` — "no tombstone" → "no soft_delete"
- `examples/hard-delete/mapping.yaml` — comment update
- `docs/reference/schema-reference.md` — resurrect description
- `engine-rs/plans/HARD-DELETE-PROPAGATION-PLAN.md` — passing references
- `engine-rs/plans/COMBINED-ETL-REVERSE-ETL-ANALYSIS.md` — passing references

## Execution order

1. **model.rs** — new `SoftDelete` struct, delete `Tombstone` + `TombstoneDefault`
   + custom deserializers.  Rename `Mapping.tombstone` → `Mapping.soft_delete`.
2. **validate.rs** — simplify tombstone validation block.
3. **render/forward.rs** — NULL out non-identity fields when soft-delete detected.
4. **render/delta.rs** — rename `mapping.tombstone` → `mapping.soft_delete`.
5. **cargo check** — verify compilation.
6. **Tests** — rewrite all tombstone test YAML; add forward-view resolution test.
7. **Schema** — update mapping-schema.json.
8. **Docs** — update schema-reference.md.
9. **Example** — update soft-delete/mapping.yaml + README.md + resolution test.
10. **Plan** — update or replace SOFT-DELETE-PLAN.md.
11. **Cosmetic** — grep for remaining "tombstone" outside `derive_tombstones`.
12. **cargo fmt --check && cargo clippy --tests -- -D warnings && cargo test**

## Scope exclusions

- **`derive_tombstones`** — unchanged.  Different mechanism (element-level
  deletion via written state), different name is appropriate.
- **`TombstoneDetection`** — the struct in `render/delta.rs` that handles
  entity-level hard-delete detection.  Rename to `HardDeleteDetection` for
  clarity since it has nothing to do with soft deletes, but this is a separate
  cosmetic cleanup.
