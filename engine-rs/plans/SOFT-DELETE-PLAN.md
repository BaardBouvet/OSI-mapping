# Soft-delete (tombstone) support

**Status:** Implemented

First-class support for source-provided tombstones — rows that remain in the
source but are semantically deleted (soft delete).  The engine knows the
tombstone field and its "alive" value, enabling both suppression and automatic
undelete.

- `reinsert: false` (default) + tombstone → suppress (NULL action)
- `reinsert: true` + tombstone → undelete ('update' action + alive value)

Complements [HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md)
(entity disappears because the row is gone) and
[PROPAGATED-DELETE-PLAN](PROPAGATED-DELETE-PLAN.md) (existing `bool_or` +
`reverse_filter` pattern).
`reverse_filter` pattern).

## Motivation

### Current state

Soft delete works today via the propagated-delete pattern:

```yaml
# User-wired soft delete (current)
targets:
  customer:
    fields:
      is_deleted:
        strategy: bool_or

mappings:
  - name: crm
    source: crm
    target: customer
    fields:
      - expression: "deleted_at IS NOT NULL"
        target: is_deleted

  - name: erp
    source: erp
    target: customer
    reverse_filter: "is_deleted IS NOT TRUE"
    fields: [...]
```

This works but requires:
1. A dedicated target field (`is_deleted`) and strategy (`bool_or`)
2. An expression mapping on every source that has a deletion signal
3. Manual `reverse_filter` on every mapping that should respond to deletions
4. No integration with `reinsert` — the delta still emits `'update'` for
   the deleted row (updating it to `is_deleted: true`) rather than `'delete'`

### What first-class support adds

A `tombstone` property on the mapping declares the field and its alive
value.  The engine derives the detection expression and knows how to
reverse the soft delete:

```yaml
# Shorthand — field with NULL alive value (most common)
mappings:
  - name: crm
    source: crm
    target: customer
    tombstone: deleted_at
    fields:
      - source: email
        target: email

# Object form — field with explicit alive value
mappings:
  - name: crm
    source: crm
    target: customer
    tombstone: { field: is_deleted, alive: false }
    fields:
      - source: email
        target: email
```

This integrates with `reinsert`:
- `reinsert: false` (default) → suppress (NULL action, row excluded)
- `reinsert: true` → undelete ('update' action, alive value projected)

## Design

### New mapping property: `tombstone`

A field name (shorthand) or `{ field, alive }` object.  The engine derives
the detection expression from the field and alive value:

| Shorthand | Object form | Detection expression |
|---|---|---|
| `tombstone: deleted_at` | `{ field: deleted_at }` | `"deleted_at" IS NOT NULL` |
| — | `{ field: is_deleted, alive: false }` | `"is_deleted" IS DISTINCT FROM FALSE` |
| — | `{ field: status, alive: active }` | `"status" IS DISTINCT FROM 'active'` |

The tombstone field is auto-included in the effective passthrough — no need
for manual `passthrough: [deleted_at]`.

### Interaction with `reinsert`

| `reinsert` | Tombstone detected | Action | Projection |
|---|---|---|---|
| `false` (default) | yes | NULL (suppress) | — |
| `true` | yes | `'update'` (undelete) | tombstone field → alive value |
| either | no | normal logic | normal |

When `reinsert: true` and tombstone is detected, the delta:
1. Emits `'update'` instead of NULL — the ETL will write back
2. Projects the alive value for the tombstone field — the ETL writes the
   alive value back to the source, clearing the soft delete

```sql
-- Action CASE (reinsert: true)
CASE
  WHEN _src_id IS NOT NULL AND ("deleted_at" IS NOT NULL) THEN 'update'
  ...
END AS _action

-- Tombstone field projection (reinsert: true)
CASE WHEN ("deleted_at" IS NOT NULL) THEN NULL
     ELSE "deleted_at" END AS "deleted_at"
```

### Vanished-entity UNION ALL

Soft-deleted rows already exist in the source — they don't need the
vanished-entity UNION ALL path (that's for hard deletes only, where the row
is gone from the reverse view entirely).  The tombstone branch in the CASE
handles soft deletes directly.

### Relationship to `reverse_filter`

`tombstone` does NOT replace `reverse_filter`.  They serve different purposes:

| Feature | Purpose | Scope |
|---|---|---|
| `tombstone` | "This source says this entity is deleted" | Per-mapping detection |
| `reverse_filter` | "This mapping only accepts rows matching this condition" | Per-mapping routing |

A mapping can have both:
```yaml
  - name: erp
    source: erp
    target: customer
    tombstone: "erp_deleted = true"
    reverse_filter: "tier IS NOT NULL"
    fields: [...]
```

### Difference from `propagated-delete` pattern

| Aspect | `propagated-delete` (current) | `tombstone` (proposed) |
|---|---|---|
| Detection | User wires `expression` + `bool_or` target field | Declared on mapping |
| Delta action | `'update'` (sets `is_deleted: true`) | suppress (excluded from delta) |
| Scope | Cross-system propagation via resolution | Per-source detection |
| Complexity | 3-4 properties across mapping + target | 1 property on mapping |
| Target schema | Requires `is_deleted` field on target | No target field needed |

They can coexist.  `tombstone` is the per-source detection signal.
`propagated-delete` is cross-source propagation via resolution.  A system
might use `tombstone` to detect soft deletes from one source, while using
`bool_or` to propagate a unified deletion signal to all sources.

## Implementation

### 1. Model

`Tombstone` struct with field-based config and serde shorthand:

```rust
/// Shorthand: `tombstone: deleted_at` (alive defaults to Null)
/// Object:    `tombstone: { field: is_deleted, alive: false }`
#[derive(Debug, Deserialize)]
#[serde(from = "TombstoneRaw")]
pub struct Tombstone { field: String, alive: AliveValue }

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TombstoneRaw {
    Field(String),
    Config { field: String, #[serde(default)] alive: AliveValue },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum AliveValue { #[default] Null, Bool(bool), String(String) }
```

Key methods:
- `Tombstone::detection_expr()` — derives SQL from field + alive
- `Tombstone::field()` — the source column name
- `AliveValue::to_sql()` — renders SQL literal (NULL, FALSE, 'value')
- `Mapping::effective_passthrough()` — `passthrough` + tombstone field

`suppress_reinsert()` is NOT updated — tombstone is independent.

### 2. Delta render

In `action_case()` and `merged_action_case()`, tombstone branches on
`reinsert`:

```rust
if let Some(ref ts) = mapping.tombstone {
    let det = ts.detection_expr();
    if mapping.reinsert {
        branches.push(format!("WHEN {src_id} IS NOT NULL AND ({det}) THEN 'update'"));
    } else {
        branches.push(format!("WHEN {src_id} IS NOT NULL AND ({det}) THEN NULL"));
    }
}
```

When `reinsert: true`, the delta also overrides the tombstone field
projection with the alive value:

```rust
// ts_override replaces the tombstone field column
let ts_override = format!(
    "CASE WHEN ({det}) THEN {alive} ELSE {field} END AS {field}",
    det = ts.detection_expr(),
    alive = ts.alive().to_sql(),
    field = qi(ts.field()),
);
```

Three rendering paths updated: single-mapping, merged-child, UNION ALL.

### 3. Schema

```json
"tombstone": {
  "oneOf": [
    { "type": "string", "description": "Field name (alive defaults to null)" },
    { "type": "object", "properties": {
        "field": { "type": "string" },
        "alive": { "description": "Value when not deleted (null, boolean, string)" }
      }, "required": ["field"] }
  ]
}
```

### 4. Validation

- Tombstone field must exist as a source column (checked via `source_cols`)
- No `check_expr` needed — not a SQL expression anymore

### 5. Example

`examples/soft-delete/` — `tombstone: deleted_at` with no explicit passthrough.

```yaml
version: "1.0"
description: >
  Soft-delete detection via tombstone field.
  CRM has a deleted_at column — when set, the customer is treated as
  disappeared from CRM.

sources:
  crm:
    primary_key: id
  erp:
    primary_key: cust_id

targets:
  customer:
    fields:
      email:
        strategy: identity
      name:
        strategy: coalesce

mappings:
  - name: crm_customers
    source: crm
    target: customer
    tombstone: deleted_at
    fields:
      - source: email
        target: email
      - source: name
        target: name
        priority: 1

  - name: erp_customers
    source: erp
    target: customer
    cluster_members: true
    fields:
      - source: email
        target: email
      - source: name
        target: name

tests:
  - description: >
      Alice soft-deleted in CRM (deleted_at set). ERP still has her.
      CRM's delta suppresses Alice — no re-insert to CRM.
      ERP's delta is unaffected (noop for Alice).
    input:
      crm:
        - { id: "C1", email: "alice@example.com", name: "Alice", deleted_at: "2026-03-15" }
        - { id: "C2", email: "bob@example.com", name: "Bob", deleted_at: null }
      erp:
        - { cust_id: "E1", email: "alice@example.com", name: "Alice" }
        - { cust_id: "E2", email: "bob@example.com", name: "Bob" }
    expected: {}
```

## Design decisions

### Why field-based and not a SQL expression?

A SQL expression (`"deleted_at IS NOT NULL"`) only answers "is this row
deleted?".  It can't answer "what value should the field have to undelete?"

A field + alive value gives the engine both:
- **Detection:** derive `field IS NOT NULL` or `field IS DISTINCT FROM value`
- **Reversal:** project the alive value when undeleting

This enables automatic undelete when `reinsert: true` — the engine knows
exactly what to write back to clear the soft-delete marker.

### Why two forms (string shorthand + object)?

The overwhelmingly common case is a nullable timestamp column (alive = NULL):
`tombstone: deleted_at`.  The object form handles less common cases:
`tombstone: { field: is_deleted, alive: false }`.

### Why not extend `filter` instead?

`filter` controls what rows enter the forward view — filtered rows don't
contribute identity or fields at all.  `tombstone` is different: the row
still exists and its identity should still link entities, but the entity is
treated as disappeared from this source's perspective in the delta.

If tombstoned rows were filtered out of the forward view, the identity
graph would lose edges, potentially unlinking entities that should remain
linked.  `tombstone` keeps the row in the forward view (preserving identity)
while the delta treats it as disappeared.

### Should tombstoned rows contribute to resolution?

Open question.  Two options:

1. **No contribution** — tombstoned rows are excluded from resolution
   entirely.  Other sources' values win.  This is simpler but means a
   soft-deleted source loses all influence.

2. **Normal contribution** — tombstoned rows still contribute to resolution
   but the delta classifies them as disappeared.  This preserves the
   source's influence on field values while suppressing/deleting the entity
   from this source's delta.

Option 2 is likely correct — it matches how the system works today.  The
forward view still has the row, resolution still considers it, but the
delta says "don't sync back to this source."

## Future considerations

- **Cross-source propagation:** `tombstone` detects per-source soft deletes.
  For cross-system propagation ("CRM deletes → delete from all systems"),
  the existing `propagated-delete` pattern (`bool_or` + `reverse_filter`)
  still works.
- **Element-level soft deletes:** Array elements with a tombstone flag.
  Out of scope for now — `derive_tombstones` handles element-level deletion
  via written state comparison.
