# Deletion as a target field — anti-corruption layer design

**Status:** Proposed

Unify entity and element deletion handling by treating deletion as a
**regular target field** synthesized via anti-corruption adapters, not as
engine-internal side-channel exclusions.

## The insight

The engine already has three anti-corruption adapters that synthesize what
imperfect sources can't provide natively:

| Ideal source has      | Adapter                | Synthesizes                    | Status   |
|-----------------------|------------------------|--------------------------------|----------|
| Timestamps            | `derive_timestamps`    | `_updated_at` from ETL clock   | **Done** |
| Change detection      | `derive_noop`          | Noop action from `_written` diff | **Done** |
| Soft-delete markers   | ???                    | Deletion signal from absence   | **Gap**  |

The first two follow a clean pattern: detect a missing capability →
synthesize the equivalent → feed it into the normal pipeline. The third
is missing. Instead, deletion detection today **fuses detection with
reaction** in one step — it detects absence and immediately excludes the
entity/element, bypassing resolution entirely.

This means:
- A hard-deleted entity is suppressed — other sources can't "see" the
  deletion and react to it
- A hard-deleted element is removed from ALL arrays — other sources have
  no per-source control over whether to keep or remove their copy
- Soft-delete via `tombstone` is also fused: it detects and excludes,
  never producing a target field that resolution could process

Meanwhile, the `propagated-delete` example already demonstrates the
principled approach for entity-level soft-delete: map the deletion marker
to a regular `is_deleted` field → `bool_or` strategy → `reverse_filter`
per consumer. This works because the source still has the row with its
marker. But hard-deleted rows/elements are **gone** — there's nothing to
map.

## Problem statement

**Absence cannot become a field value.** The engine can detect absence
(via `cluster_members` / `_written` diffing) but can only act on it
internally (suppress entity, exclude element). It cannot inject a
synthetic field value into the target model for resolution to process.

This creates an asymmetry:

| Deletion type    | Signal type | Can propagate via field? | Current behavior      |
|-----------------|-------------|-------------------------|-----------------------|
| Entity soft-del | Source data | Yes (expression mapping) | `propagated-delete` pattern |
| Entity hard-del | Absence     | **No**                  | Implicit suppression  |
| Element soft-del| Source data | Partially (tombstone excludes) | `element-soft-delete` |
| Element hard-del| Absence     | **No**                  | `derive_element_tombstones` excludes |

In all four cases, the **ideal** behavior is the same: deletion becomes a
target field → resolution combines signals → each consumer reacts
independently. Only the first case achieves this today.

## Design: the perfect model + adapters

### The perfect source model

Imagine every source system has perfect soft-delete semantics:
- Entities are never physically deleted; they get an `is_deleted` marker
- Array elements are never removed; they get an `is_removed` marker
- CRDT ordering means no conflicts

In this ideal world, the mapping is trivial:

```yaml
targets:
  customer:
    fields:
      email: { strategy: identity }
      name: { strategy: coalesce }
      is_deleted: { strategy: bool_or }

  task:
    fields:
      project_ref: { strategy: identity, link_group: task_id }
      title: { strategy: identity, link_group: task_id }
      is_removed: { strategy: bool_or }

mappings:
  - name: crm_customers
    source: crm
    target: customer
    fields:
      - source: email
        target: email
      - source: name
        target: name
      - source: deleted
        target: is_deleted

  - name: erp_customers
    source: erp
    target: customer
    reverse_filter: "is_deleted IS NOT TRUE"
    fields:
      - source: email
        target: email
      - source: tier
        target: tier
```

No special deletion machinery. Resolution handles it. Each consumer
decides. This is exactly what `propagated-delete` already demonstrates.

### The imperfect reality

Sources aren't ideal. They hard-delete, or soft-delete but the element
is gone from the array, or don't track deletion at all. The engine needs
adapters:

```
Detection              →  Adapter synthesizes  →  Resolution   →  Per-consumer
                          a target field                          reaction
────────────────────────────────────────────────────────────────────────────────
Source has soft-delete     (no adapter needed,     bool_or /       reverse_filter
  marker                   just map the field)     coalesce        element_filter

Source hard-deletes        derive_tombstones:       ↑               ↑
  entity (row gone,        synthesize is_deleted
  detected via             from _written /
  cluster_members /        cluster_members
  written_state)           absence

Source hard-deletes        derive_element_           ↑               ↑
  element (element         tombstones:
  gone from array,         synthesize is_removed
  detected via             from _written array
  written_state diff)      absence
```

### What changes

Today `derive_element_tombstones` detects absence and produces a
`DeletionFilter` (an exclusion list). The new design would instead:

1. **Detect** absence (same `_written` diff logic)
2. **Inject** a synthetic `is_removed = true` into the element's resolved
   fields (or `is_deleted = true` for entities)
3. **Auto-nullify** all non-identity field contributions from that source
   (a deleted source shouldn't poison resolution with stale values)
4. **Let resolution handle it** via `bool_or` (or whatever strategy the
   user chose)
5. **Let each consumer react** via `reverse_filter` / element filter

### Automatic field nullification

A deleted source must not contribute field values to resolution. If CRM
soft-deletes a customer, its `name: "Alice"` should not win `coalesce`
against other sources. Only the identity fields (for matching) and the
deletion field itself should be contributed.

Without this, users would have to wrap every field mapping in a CASE:

```yaml
# BAD: user manually guards every field
fields:
  - expression: "CASE WHEN deleted_at IS NOT NULL THEN NULL ELSE name END"
    target: name
  - expression: "CASE WHEN deleted_at IS NOT NULL THEN NULL ELSE tier END"
    target: tier
  # ... repeated for every field
```

The engine should do this automatically. When `tombstone.target` is set,
the forward view wraps non-identity fields:

```sql
-- Engine-generated forward view with tombstone.target: is_deleted
SELECT
  email,                                                    -- identity: kept
  CASE WHEN deleted_at IS NOT NULL THEN NULL ELSE name END AS name,
  CASE WHEN deleted_at IS NOT NULL THEN NULL ELSE tier END AS tier,
  (deleted_at IS NOT NULL)::boolean AS is_deleted           -- deletion field
FROM crm_source
```

The user writes only:

```yaml
tombstone:
  field: deleted_at
  target: is_deleted      # engine auto-nullifies non-identity fields
```

For `derive_tombstones` (hard-delete, source row gone), auto-nullification
is implicit — there's no source row, so the only thing to contribute is
the synthetic `is_deleted = TRUE`. Everything else is already NULL.

For element-level `derive_element_tombstones`, same: an absent element
re-injected into resolution has `is_removed = TRUE` and NULLs for all
non-identity fields.

**The principle:** A source that says "this is deleted" yields the floor
on every field except the deletion signal itself. The deletion field
participates in resolution (`bool_or`). All other fields become NULL for
this source, letting surviving sources' values win naturally.

## Proposed API

### Entity-level: `derive_tombstones`

```yaml
# Dev tracker doesn't soft-delete — when a customer disappears from
# its feed, synthesize is_deleted = true for that source contribution.
mappings:
  - name: dev_tracker_customers
    source: dev_tracker
    target: customer
    written_state: true
    derive_tombstones: is_deleted      # ← field to synthesize
    fields:
      - source: email
        target: email
      - source: name
        target: name

  # ERP opts in to deletion propagation
  - name: erp_customers
    source: erp
    target: customer
    reverse_filter: "is_deleted IS NOT TRUE"
    fields: [...]

  # CRM does NOT have reverse_filter → keeps the record,
  # sees is_deleted = true, shows it as archived
```

When the engine detects that a dev_tracker entity disappeared (was in
`cluster_members` or `_written` but is no longer in the forward view),
it synthesizes `is_deleted = true` for that source's contribution to
the target field. Resolution combines it via `bool_or` (or whatever
strategy `is_deleted` uses). Each consumer's `reverse_filter` then
determines what to do.

### Element-level: `derive_element_tombstones`

```yaml
# Dev tracker hard-deletes tasks (removes from array).
# Synthesize is_removed = true for disappeared elements.
mappings:
  - name: dev_projects
    source: dev_tracker
    target: project
    written_state: true
    derive_element_tombstones: is_removed   # ← field to synthesize
    fields: [...]

  - name: dev_tasks
    parent: dev_projects
    array: tasks
    target: task
    fields: [...]

  # PM tool wants to keep removed tasks but marked
  - name: pm_tasks
    parent: pm_projects
    array: tasks
    target: task
    # no element filter → sees is_removed in the data
    fields: [...]

  # Reporting tool wants clean arrays
  - name: report_tasks
    parent: report_projects
    array: tasks
    target: task
    element_filter: "is_removed IS NOT TRUE"
    fields: [...]
```

### Tombstone config: `target` property

The existing `tombstone` config currently detects-and-excludes. To fit
the new model, it would gain a `target` property:

```yaml
# Current: detect and exclude (backwards compatible)
tombstone:
  field: cancelled_at
  undelete_value: null

# New: detect and produce a target field
tombstone:
  field: cancelled_at
  target: is_removed        # ← maps detection into a target field
```

When `target` is set, the tombstone detection expression feeds into the
target field as a boolean value rather than producing an exclusion NULL.
This makes `tombstone` composable with resolution — the detection becomes
data, not a side effect.

If `SOFT-DELETE-REFACTOR-PLAN` proceeds, this would be:

```yaml
soft_delete:
  field: cancelled_at
  target: is_removed
```

### Element filter: `element_filter`

A new property on child mappings, analogous to `reverse_filter` on root
mappings:

```yaml
- name: dev_tasks
  parent: dev_projects
  array: tasks
  target: task
  element_filter: "is_removed IS NOT TRUE"
  fields: [...]
```

The `element_filter` would apply inside the `jsonb_agg`:

```sql
-- Without element_filter (all elements)
jsonb_agg(jsonb_build_object(...) ORDER BY ...)

-- With element_filter (exclude filtered elements)
jsonb_agg(jsonb_build_object(...) ORDER BY ...)
  FILTER (WHERE is_removed IS NOT TRUE)
```

This is the per-consumer control mechanism for elements, mirroring
`reverse_filter` for entities.

## Implementation phases

### Phase 1: `element_filter` on child mappings

Add `element_filter` as a SQL expression property on child mappings that
controls which elements are included in the reconstructed JSONB array.
This is the consumption-side mechanism that enables per-source control.

Applies a `FILTER (WHERE ...)` clause on the `jsonb_agg()` in the nested
CTE pipeline. Validated against target fields the same way `reverse_filter`
is today.

This can be shipped independently — even without the synthesis adapters,
users with soft-delete sources can map markers to target fields and use
`element_filter` to control per-source inclusion.

### Phase 2: `tombstone.target` — detection as a field + auto-nullification

Extend `tombstone` (or `soft_delete` if that refactor proceeds) with a
`target` property. When set:

- The detection expression produces a boolean value mapped to the target
  field instead of a NULL action
- **All non-identity fields are auto-nullified** when the detection fires
  — the source contributes only identity fields and the deletion signal,
  preventing stale values from poisoning resolution
- The row/element participates in resolution normally (with NULLed fields)
- The target field is resolved via its declared strategy (`bool_or`, etc.)

The auto-nullification is essential: without it, a deleted source would
still win `coalesce` or `last_modified` for fields it contributed before
deletion. Wrapping every field in a manual `CASE WHEN ... THEN NULL`
would be unacceptable ergonomics.

This makes `tombstone` work with the field-based propagation model.
The existing detect-and-exclude behavior is preserved when `target` is
not set (backwards compatible).

### Phase 3: `derive_tombstones` — synthesis from absence (entity-level)

When the engine detects entity absence (was in `cluster_members` or
`_written` but no longer in forward view), instead of emitting a NULL
action (suppress), it synthesizes a contribution to the named target
field.

SQL sketch for the entity case:

```sql
-- In the delta CASE, instead of:
WHEN _src_id IS NULL AND _ws._cluster_id IS NOT NULL THEN NULL  -- suppress

-- Produce:
WHEN _src_id IS NULL AND _ws._cluster_id IS NOT NULL THEN 'update'
-- And in the SELECT list:
CASE
  WHEN _src_id IS NULL AND _ws._cluster_id IS NOT NULL
  THEN TRUE
  ELSE FALSE
END AS is_deleted
```

The synthetic `is_deleted = TRUE` participates in resolution. Other
sources that still have the entity contribute `is_deleted = FALSE`
(or NULL). The resolution strategy combines them.

### Phase 4: `derive_element_tombstones` refactor — synthesis from absence (element-level)

The current `_del_` CTE pipeline detects absent elements. Instead of
producing an exclusion filter (LEFT JOIN + WHERE IS NULL), it would
inject the absent elements back into the resolved set with
`is_removed = TRUE`:

```sql
-- Current: UNION of deleted identities for exclusion
_del_tasks AS (
  SELECT _parent_key, title FROM _del_src_tasks_0
  UNION ALL
  SELECT _parent_key, title FROM _del_src_tasks_1
)

-- New: re-inject deleted elements with is_removed = TRUE
-- These participate in resolution alongside live elements
```

The resolved element set then includes both live elements
(`is_removed = NULL/FALSE`) and synthesized-absent elements
(`is_removed = TRUE`). Each consumer's `element_filter` decides
whether to include or exclude them in its reconstructed array.

## Interaction with existing features

### `reverse_filter` (entity-level)

Unchanged. `reverse_filter` already works with the field-based model
(see `propagated-delete` example). The `derive_tombstones` adapter
just feeds the field that `reverse_filter` can reference.

### `resurrect`

`resurrect: true` means "if an entity was previously synced and the
source row reappears, re-sync it." In the new model:

- `resurrect: false` (default): absent entity → `is_deleted = TRUE`
  is synthesized and sticky (entity stays "deleted" in this source's
  contribution until it reappears)
- `resurrect: true`: absent entity → `is_deleted = TRUE` is synthesized,
  but if the row returns, `is_deleted = FALSE` replaces it

This is more nuanced than the current binary suppress/allow — the
deletion signal is data, so reappearance naturally overwrites it.

### `derive_noop`

No conflict. `derive_noop` compares field values against `_written` to
detect noop. `derive_tombstones` adds one more synthetic field
(`is_deleted`) to the comparison set.

### `derive_timestamps`

No conflict. Orthogonal adapter.

### `tombstone` without `target` (current behavior)

Backwards compatible. When `tombstone` has no `target` property, it
continues to detect-and-exclude as today. This is the "local soft-delete"
behavior — the source's own soft-delete handling, not propagation.

Mapping authors who want propagation explicitly add `target:` to route
the detection into a resolved field. This opt-in mirrors how entity-level
propagation works today (you explicitly map `deleted_at IS NOT NULL` to
`is_deleted`).

## What this replaces

- **Implicit entity suppression** (current: detect absence → emit NULL
  action) → becomes `derive_tombstones` synthesizing a field
- **`derive_element_tombstones: true`** (current: detect absence →
  exclude from all arrays) → becomes `derive_element_tombstones: field_name`
  synthesizing a field + `element_filter` per consumer
- **`tombstone` detect-and-exclude** (for propagation scenarios) →
  becomes `tombstone.target` producing a field

The current behaviors remain available as defaults (no `target`, no
`element_filter` → same exclusion semantics). The new properties extend
the model for users who need per-consumer control.

## Example: mixed element deletion (the motivating scenario)

Two sources manage tasks as array elements. PM tool soft-deletes
(cancelled_at marker). Dev tracker hard-deletes (element vanishes).
Different consumers want different reactions.

```yaml
version: "1.0"

targets:
  project:
    fields:
      name: { strategy: identity }

  task:
    fields:
      project_ref: { strategy: identity, link_group: task_id }
      title: { strategy: identity, link_group: task_id }
      is_removed: { strategy: bool_or }       # ← resolved deletion signal
      task_order: { strategy: coalesce }

mappings:
  # PM tool: has soft-delete markers (perfect source)
  - name: pm_projects
    source: pm_tool
    target: project
    fields:
      - source: name
        target: name

  - name: pm_tasks
    parent: pm_projects
    array: tasks
    target: task
    tombstone:
      field: cancelled_at
      target: is_removed              # ← detection becomes a field
    fields:
      - source: parent_name
        target: project_ref
        references: pm_projects
      - source: title
        target: title
      - target: task_order
        order: true

  # Dev tracker: hard-deletes tasks (imperfect source, needs adapter)
  - name: dev_projects
    source: dev_tracker
    target: project
    written_state: true
    derive_element_tombstones: is_removed   # ← absence → field synthesis
    fields:
      - source: name
        target: name

  - name: dev_tasks
    parent: dev_projects
    array: tasks
    target: task
    element_filter: "is_removed IS NOT TRUE"  # ← dev tracker wants clean arrays
    fields:
      - source: parent_name
        target: project_ref
        references: dev_projects
      - source: title
        target: title
      - target: task_order
        order: true

  # PM tool child: keeps removed tasks visible (no element_filter)
  # PM tool sees is_removed = true and renders with strikethrough
```

### Behavior

1. PM tool cancels "Write docs" → `cancelled_at` set →
   `tombstone.target` maps to `is_removed = TRUE`
2. Resolution: `is_removed` via `bool_or` → TRUE (PM tool says removed)
3. Dev tracker's `element_filter` excludes it → "Write docs" removed
   from dev tracker's array
4. PM tool has no `element_filter` → "Write docs" stays in PM tool's
   array, with `is_removed = TRUE` available if the ETL wants to render
   it differently

And symmetrically:

1. Dev tracker removes "Fix bug" from array → `derive_element_tombstones`
   synthesizes `is_removed = TRUE` for that element
2. Resolution: `bool_or` → TRUE
3. Dev tracker's `element_filter` excludes it
4. PM tool keeps it, sees `is_removed = TRUE`

Each consumer independently decides its reaction. Deletion is data, not
an engine side effect.

## Open questions

1. **Should `element_filter` live on the child mapping or the parent?**
   The parent's `reverse_filter` controls entity inclusion. The child's
   `element_filter` would control element inclusion within the parent's
   array. Putting it on the child mapping feels right since it's
   per-array, not per-entity.

2. **How does `derive_tombstones` (entity-level) interact with
   `cluster_members`?** Currently `cluster_members` is used for insert
   feedback. If the entity disappears from the forward view but is still
   in `cluster_members`, we synthesize `is_deleted = TRUE`. But the
   entity might have been removed from `cluster_members` too (the ETL
   cleaned up). Need to define the exact detection semantics.

3. **Default field name.** Should `derive_tombstones` / `derive_element_
   tombstones` require an explicit field name (as proposed), or default
   to a conventional name like `_is_deleted` / `_is_removed`? Explicit
   is clearer but verbose. Given the pre-1.0 simplicity principle,
   requiring explicit is better.

4. **Phase 1 standalone value.** `element_filter` is useful even without
   the synthesis adapters — users with soft-delete sources can already
   map markers to fields and filter per consumer. This argues for
   shipping Phase 1 first.

5. **Interaction with `soft_delete` refactor.** If SOFT-DELETE-REFACTOR-
   PLAN proceeds (renaming `tombstone` to `soft_delete`), the `target`
   property would be on `soft_delete` instead. The design is the same
   regardless of naming.
