# Soft-delete (tombstone) support

**Status:** Implemented

First-class support for source-provided tombstones — rows that remain in the
source but are semantically deleted (soft delete).  When a source provides a
deletion signal, the engine treats the entity as disappeared from that
source.  Independent of `reinsert` — always suppresses the row from the delta.

Complements [HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md)
(entity disappears because the row is gone) and
[PROPAGATED-DELETE-PLAN](PROPAGATED-DELETE-PLAN.md) (existing `bool_or` +
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

A `tombstone` property on the mapping declares "when this expression is true,
treat the entity as disappeared from this source":

```yaml
# First-class soft delete (proposed)
mappings:
  - name: crm
    source: crm
    target: customer
    tombstone: "deleted_at IS NOT NULL"
    fields:
      - source: email
        target: email
```

This integrates with the delta CASE independently of `reinsert`:
- The delta CASE sees the row as "disappeared" — same as a hard delete
- Always emits NULL (suppress) — the row is excluded from the delta

## Design

### New mapping property: `tombstone`

A SQL boolean expression evaluated in the reverse view context.  When true,
the entity is treated as disappeared from this source.

```yaml
tombstone: "deleted_at IS NOT NULL"
```

| Type | Required | Default | Description |
|---|---|---|---|
| string | no | — | SQL boolean expression; when true, entity is treated as disappeared |

When `tombstone` is set, the delta CASE evaluates it before the normal
insert/update/delete/noop logic:

```sql
CASE
  -- Soft-delete: source row exists but is tombstoned
  WHEN _src_id IS NOT NULL AND (deleted_at IS NOT NULL) THEN NULL
  -- Hard-delete: source row gone but was previously synced (reinsert: false)
  WHEN _src_id IS NULL AND _cm_hd."_src_id" IS NOT NULL THEN NULL
  -- Normal insert/update/noop...
  WHEN _src_id IS NULL THEN 'insert'
  WHEN ... THEN 'noop'
  ELSE 'update'
END
```

The suppress branch always emits `NULL` (row excluded from delta).

### Independence from `reinsert`

`tombstone` is independent of the `reinsert` mechanism.  It does NOT
contribute to `suppress_reinsert()`.  The tombstone CASE branch is always
active when the expression is set — no detection mechanism needed:

```rust
// In action_case():
if let Some(ref expr) = mapping.tombstone {
    branches.push(format!("WHEN {src_id} IS NOT NULL AND ({expr}) THEN NULL"));
}
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

Add `tombstone: Option<String>` to `Mapping`:

```rust
/// SQL boolean expression — when true, the entity is treated as
/// disappeared from this source (soft delete).  Feeds into
/// `reinsert` mechanism.  Independent — always suppresses when set.
#[serde(default)]
pub tombstone: Option<String>,
```

`suppress_reinsert()` is NOT updated — tombstone is independent.

### 2. Delta render

In `action_case()` and `merged_action_case()`, add a branch before the
hard-delete detection:

```rust
// Soft-delete: source row exists but tombstone expression is true
// Independent of reinsert — always suppresses
if let Some(ref expr) = mapping.tombstone {
    branches.push(format!("WHEN {src_id} IS NOT NULL AND ({expr}) THEN NULL"));
}
```

This branch fires BEFORE the `_src_id IS NULL` checks, so a soft-deleted
row with a source row present is caught early.

### 3. Schema

Add to `mapping-schema.json`:

```json
"tombstone": {
  "type": "string",
  "description": "SQL boolean expression — when true, the entity is treated as disappeared from this source (soft delete). Always suppresses the row from the delta, independent of the reinsert setting."
}
```

### 4. Validation

- `tombstone` has no prerequisites — always active when set.
- No need for `reinsert: false` — tombstone suppression is independent.

### 5. Example

New example: `soft-delete/`

```yaml
version: "1.0"
description: >
  Soft-delete detection via tombstone expression.
  CRM has a deleted_at column — when set, the customer is treated as
  disappeared from CRM.  The engine applies reinsert policy.

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
    tombstone: "deleted_at IS NOT NULL"
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

### Why `tombstone` and not `soft_delete`?

Consistency with `derive_tombstones` — both deal with tombstones (markers
indicating something is dead).  `derive_tombstones` derives synthetic
tombstones from written state; `tombstone` declares that the source provides
its own tombstone signal.

### Why a SQL expression and not a column name?

Sources express soft deletes differently:
- `deleted_at IS NOT NULL` (timestamp column)
- `is_deleted = true` (boolean flag)
- `status = 'archived'` (enum value)
- `active = false` (inverted boolean)

A SQL expression handles all cases without needing multiple configuration
knobs.

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
  still works.  A future `on_disappear: propagate` value could automate
  this.
- **Element-level soft deletes:** Array elements with a tombstone flag.
  Out of scope for now — `derive_tombstones` handles element-level deletion
  via written state comparison.
