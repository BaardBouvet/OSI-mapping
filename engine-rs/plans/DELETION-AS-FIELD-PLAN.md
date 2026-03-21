# Deletion as a target field — anti-corruption layer design

**Status:** Done

Unify entity and element deletion handling by treating deletion as a
**regular target field** synthesized via anti-corruption adapters, not as
engine-internal side-channel exclusions.

## Lifecycle building blocks — complete picture

This section summarizes every building block for entity and element
lifecycles: what exists today, what's planned, and how they compose.

### The anti-corruption adapter pattern

The engine assumes an ideal source model (soft-delete markers, CRDT
ordering, change timestamps) and provides adapters for imperfect
sources. Each adapter synthesizes what the source can't provide:

| Ideal source has        | Adapter                        | Synthesizes                  | Status       |
|-------------------------|--------------------------------|------------------------------|--------------|
| Change timestamps       | `derive_timestamps`            | `_updated_at` from ETL clock | **Done**     |
| Change detection        | `derive_noop`                  | Noop action from written diff| **Done**     |
| Element ordering        | CRDT ordering (`order: true`)  | Deterministic position       | **Done**     |
| Soft-delete markers     | `soft_delete.target`           | Deletion signal as field     | **Planned**  |
| Soft-delete markers (elements) | `tombstone.target`      | Element deletion as field    | **Planned**  |
| Hard-delete detection   | `derive_tombstones`            | Absence → field synthesis    | **Planned**  |
| Element hard-delete     | `derive_element_tombstones`    | Absence → field synthesis    | **Planned** (currently: exclusion only) |

### Entity lifecycle — building blocks

```
SOURCE SIDE                    TARGET MODEL                   CONSUMER SIDE
─────────────                  ────────────                   ─────────────

Detection adapters:            Resolution:                    Reaction:

┌─────────────────┐            ┌───────────┐                 ┌───────────────┐
│ Source has row   │──field────▶│           │                 │               │
│ with soft-delete │  mapping   │ is_deleted│──reverse_filter─▶│ action=delete │
│ marker           │           │           │                 │ (ETL handles) │
│                  │           │ strategy: │                 │               │
│ soft_delete:     │──target──▶│ bool_or   │                 │ OR:           │
│   field: del_at  │  (auto)   │           │  no filter ────▶│ sees field,   │
│   target: is_del │           │           │                 │ renders as    │
│                  │           │           │                 │ archived      │
└─────────────────┘            └───────────┘                 └───────────────┘

┌─────────────────┐                 ▲
│ Source hard-     │                 │
│ deletes row      │                 │
│ (row gone)       │──synthesize────┘
│                  │  is_deleted
│ derive_tombstones│  = TRUE
│  : is_deleted    │  (+ auto-null
└─────────────────┘   other fields)
```

| Building block | Type | Status | What it does |
|---|---|---|---|
| ~~`tombstone`~~ | Detection | **Superseded** | Replaced by `soft_delete` (SOFT-DELETE-REFACTOR) |
| `soft_delete` | Detection | **Done** (SOFT-DELETE-REFACTOR) | 3-strategy API (timestamp/deleted_flag/active_flag) |
| `soft_delete.target` | Detection → field | **Done** (this plan) | Routes detection into a target field instead of excluding |
| Auto-nullification | Forward view | **Done** (this plan) | When detection fires, NULLs all non-identity fields automatically |
| `reverse_filter` | Reaction | **Done** | Per-consumer entity exclusion (`action = 'delete'`) |
| `derive_tombstones` | Synthesis | **Done** (this plan) | Hard-delete → synthesize target field from absence |
| `cluster_members` | ETL feedback | **Done** | Tracks which entities were synced (insert dedup + resurrection) |
| `written_state` | ETL feedback | **Done** | Tracks written entity state (noop + hard-delete detection) |
| ~~`resurrect`~~ | Policy | **Superseded** | Replaced by `derive_tombstones` + `soft_delete.target` |
| `derive_noop` | Synthesis | **Done** | Synthesizes noop from `_written` diff |
| `derive_timestamps` | Synthesis | **Done** | Synthesizes `_updated_at` from ETL clock |

**How they compose (entity):**

```
soft_delete.target ──▶ is_deleted (bool_or) ──▶ reverse_filter ──▶ delete
          │                                           │
          │ (auto-null fields)                        │ (no filter)
          ▼                                           ▼
   source yields floor                        consumer keeps record,
   in resolution                              sees is_deleted = true

derive_tombstones ───▶ is_deleted (bool_or) ──▶ same path
   (hard-delete)          ▲
                          │
   other sources ─────────┘ is_deleted = false (still alive there)

   entity reappears ──▶ source row returns ──▶ is_deleted = false
                                               (natural, no special logic)
```

### Element lifecycle — building blocks

```
SOURCE SIDE                    TARGET MODEL                   CONSUMER SIDE
─────────────                  ────────────                   ─────────────

┌─────────────────┐            ┌───────────┐                 ┌───────────────┐
│ Source has       │            │           │                 │               │
│ element with     │──target──▶│is_removed │──reverse_filter──▶│ excluded from │
│ soft-delete      │  (auto)   │           │                 │ this source's │
│ marker           │           │ strategy: │                 │ array         │
│                  │           │ bool_or   │                 │               │
│ tombstone:       │           │           │  no filter ────▶│ kept in array │
│   field: rm_at   │           │           │                 │ with marker   │
│   target: is_rm  │           │           │                 │               │
└─────────────────┘            └───────────┘                 └───────────────┘

┌─────────────────┐                 ▲
│ Source removes   │                 │
│ element from     │──synthesize────┘
│ array (gone)     │  is_removed
│                  │  = TRUE
│ derive_element_  │
│  tombstones:     │
│  is_removed      │
└─────────────────┘
```

| Building block | Type | Status | What it does |
|---|---|---|---|
| `tombstone` (child) | Detection | **Done** | Detects element soft-delete marker, excludes from array |
| `tombstone.target` | Detection → field | **Planned** (this plan) | Routes detection into target field instead of excluding |
| `reverse_filter` (child) | Reaction | **Planned** (this plan, extend existing) | Per-consumer element exclusion in `jsonb_agg` FILTER |
| `derive_element_tombstones` | Synthesis (current) | **Done** | Detects absent elements via `_written` diff, excludes from all arrays |
| `derive_element_tombstones` | Synthesis (planned) | **Planned** (this plan) | Detects absent elements, synthesizes target field instead |
| CRDT ordering | Ordering | **Done** | `order: true` provides deterministic element positions |
| `written_state` (parent) | ETL feedback | **Done** | Parent's `_written` JSONB stores arrays for element diff |

**How they compose (element):**

```
tombstone.target ──▶ is_removed (bool_or) ──▶ reverse_filter ──▶ excluded
         │                                          │
         │ (auto-null fields)                       │ (no filter)
         ▼                                          ▼
  element yields floor                       kept in array with
  in resolution                              is_removed = true

derive_element_tombstones ──▶ is_removed (bool_or) ──▶ same path
   (hard-delete)                   ▲
                                   │
   other sources ──────────────────┘ is_removed = false
```

### TARGET-ARRAYS-PLAN: structural unification

The current system requires separate child targets for element
entities. TARGET-ARRAYS-PLAN (Proposed) adds array-typed fields
directly on targets:

| Building block | Status | What it does |
|---|---|---|
| Array field (`type: jsonb[]`) | **Planned** | Array as a first-class target field |
| `element_identity` | **Planned** | Declares which sub-fields identify unique elements |
| `strategy: coalesce` on arrays | **Planned** | Highest-priority source's element set wins |
| `strategy: collect` on arrays | **Planned** | Merge elements from all sources (deduplicated) |
| `array_field` child mapping | **Planned** | Auto-generates FK→array aggregation in forward view |

This subsumes the element set authority problem: `coalesce` on an
array field means the highest-priority source owns which elements
exist. No new concepts — reuses existing resolution strategies.

### Full composition table

| Scenario | Detection | Target field | Strategy | Consumer reaction | Status |
|----------|-----------|-------------|----------|-------------------|--------|
| Entity soft-delete (local) | `tombstone`/`soft_delete` | — | — | Excluded from delta | **Done** |
| Entity soft-delete (propagated) | Expression mapping | `is_deleted` | `bool_or` | `reverse_filter` | **Done** |
| Entity soft-delete (propagated, ergonomic) | `soft_delete.target` | `is_deleted` | `bool_or` | `reverse_filter` | **Planned** |
| Entity hard-delete (local) | Implicit from `written_state` | — | — | Suppressed | **Done** |
| Entity hard-delete (propagated) | `derive_tombstones` | `is_deleted` | `bool_or` | `reverse_filter` | **Planned** |
| Element soft-delete (local) | `tombstone` on child | — | — | Excluded from all arrays | **Done** |
| Element soft-delete (propagated) | `tombstone.target` | `is_removed` | `bool_or` | `reverse_filter` (child) | **Planned** |
| Element hard-delete (local) | `derive_element_tombstones` | — | — | Excluded from all arrays | **Done** |
| Element hard-delete (propagated) | `derive_element_tombstones` | `is_removed` | `bool_or` | `reverse_filter` (child) | **Planned** |
| Element set authority | `strategy: coalesce` on array field | — | `coalesce` | Priority-based | **Planned** (TARGET-ARRAYS) |
| Resurrect (entity) | Source reappears | — | — | Undelete value written back | **Done** |

### Reverse direction summary

| Event | Engine output | ETL connector responsibility |
|---|---|---|
| Entity deleted | `action = 'delete'` (via `reverse_filter`) | Connector issues source-native delete/archive |
| Entity resurrected | `action = 'update'` + undelete value | Connector writes the undelete value |
| Element excluded | Absent from reconstructed array | Connector writes updated array |
| Element included | Present in reconstructed array | Connector writes updated array |
| Entity noop | `action = 'noop'` | Connector skips |

The engine never writes non-deterministic values (no `NOW()` in views).
All reverse values are stable: undelete values are constants derived
from `soft_delete` strategy (`NULL`/`FALSE`/`TRUE`).

### Implementation phases

| Phase | Plan | Delivers | Depends on |
|---|---|---|---|
| 1 | DELETION-AS-FIELD | `reverse_filter` on array child mappings | — |
| 2 | SOFT-DELETE-REFACTOR | `soft_delete` replaces `tombstone` | — |
| 3 | DELETION-AS-FIELD | `soft_delete.target` + auto-nullification | Phase 2 |
| 4 | DELETION-AS-FIELD | `derive_tombstones: field_name` (entity) | Phase 3 |
| 5 | DELETION-AS-FIELD | `derive_element_tombstones` refactor (element) | Phase 3 |
| 6 | TARGET-ARRAYS | Scalar array fields (`text[]`, `collect`, `coalesce`) | — |
| 7 | TARGET-ARRAYS | Object array fields (`jsonb[]`, `element_identity`, `array_field`) | Phase 6 |

Phases 1-5 and 6-7 are independent tracks that can progress in parallel.

### Existing examples

| Example | Lifecycle | Level | Status |
|---|---|---|---|
| `soft-delete/` | Entity soft-delete (local) | Entity | Done |
| `hard-delete/` | Entity hard-delete (local) | Entity | Done |
| `propagated-delete/` | Entity soft-delete (propagated) | Entity | Done |
| `element-soft-delete/` | Element soft-delete (cross-source) | Element | Done |
| `element-hard-delete/` | Element hard-delete (cross-source) | Element | Done |
| `derive-noop/` | Change detection synthesis | Entity | Done |
| `derive-timestamps/` | Timestamp synthesis | Entity | Done |

---

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
  marker                   just map the field)     coalesce        reverse_filter

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
    derive_tombstones: is_deleted      # ← field to synthesize (value: TRUE)
    fields:
      - source: email
        target: email
      - source: name
        target: name

  # Or with a custom value (string shorthand defaults to TRUE):
  - name: dev_tracker_customers
    source: dev_tracker
    target: customer
    written_state: true
    derive_tombstones:
      target: deleted_by
      expression: "'dev_tracker'"      # ← custom literal value
    fields: [...]

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
it synthesizes the value into the named target field. For the string
shorthand (`derive_tombstones: is_deleted`), the value is `TRUE`. For
the object form, the `expression` is used.

Note: the `_written` JSONB does contain the entity's previous field
values, so the expression *could* technically reference them via
`_ws._written->>'field'`. However this is intentionally not exposed —
the written schema uses target field names (not source), depends on
prior resolution, and is NULL on first sync. Literal values (`TRUE`,
`'erp'`) cover the real use cases cleanly. Users needing previous-state
access can use a regular expression field mapping against the written
state table directly.

Resolution combines it via the target field's strategy. Each consumer's
`reverse_filter` then determines what to do.

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
    reverse_filter: "is_removed IS NOT TRUE"
    fields: [...]
```

### Tombstone config: `target` + `expression`

The existing `tombstone` config currently detects-and-excludes. To fit
the new model, it gains `target` and optionally `expression`:

```yaml
# Current: detect and exclude (backwards compatible)
tombstone:
  field: cancelled_at
  undelete_value: null

# New: detect and produce a boolean target field (default expression: TRUE)
tombstone:
  field: cancelled_at
  target: is_removed

# New: detect and produce a custom value
tombstone:
  field: cancelled_at
  target: deleted_by
  expression: "'pm_tool'"

# New: propagate the original timestamp
tombstone:
  field: cancelled_at
  target: deleted_at_resolved
  expression: "cancelled_at"
```

When `target` is set, the tombstone detection injects a value into the
named target field instead of producing an exclusion NULL. The injected
value is `expression` if provided, otherwise `TRUE`. All non-identity
fields are auto-nullified (see above).

This makes `tombstone` composable with any resolution strategy:

| Target field         | Strategy        | Expression       | Semantics                         |
|----------------------|-----------------|------------------|-----------------------------------|
| `is_deleted`         | `bool_or`       | (default: TRUE)  | Deleted if any source says so     |
| `deleted_by`         | `collect`       | `'erp'`          | Which systems flagged deletion    |
| `deleted_at`         | `last_modified` | `cancelled_at`   | Most recent deletion timestamp    |
| `deletion_source`    | `coalesce`      | `'pm_tool'`      | Highest-priority deleter wins     |

Generated SQL:

```sql
-- Default (no expression)
CASE WHEN cancelled_at IS NOT NULL THEN TRUE ELSE NULL END AS is_removed

-- With expression
CASE WHEN cancelled_at IS NOT NULL THEN 'pm_tool' ELSE NULL END AS deleted_by
```

The detection becomes data, not a side effect. The `expression` is
evaluated in the same SQL context as the forward view, so it can
reference any source column.

If `SOFT-DELETE-REFACTOR-PLAN` proceeds, this would be:

```yaml
soft_delete:
  field: cancelled_at
  target: is_removed
  expression: "'pm_tool'"    # optional
```

### Reverse direction: delete action, not field write-back

The forward direction maps source markers into a resolved target field.
The reverse direction is simpler than you'd expect: **it just emits a
delete action.**

The engine doesn't need to know how each source physically represents
deletion. When `reverse_filter: "is_deleted IS NOT TRUE"` evaluates to
false, the delta emits `action = 'delete'`. The ETL connector for each
source then handles the physical representation:

- Connector for a system with `deleted_at`: writes `deleted_at = NOW()`
- Connector for a system with `is_active`: writes `is_active = FALSE`
- Connector for a system with hard-delete: issues a DELETE
- Connector for a system with an API: calls the archive/delete endpoint

This is the anti-corruption layer at the ETL connector level — not in
the SQL views. The engine's responsibility ends at `action = 'delete'`.

On the next sync cycle, the source's input reflects the deletion however
that source represents it (soft-delete marker, missing row, etc.), and
the `soft_delete` config on the forward side detects it normally.

```yaml
targets:
  customer:
    fields:
      email: { strategy: identity }
      is_deleted: { strategy: bool_or }

mappings:
  # CRM: soft-deletes via deleted_at
  - name: crm_customers
    source: crm
    target: customer
    reverse_filter: "is_deleted IS NOT TRUE"
    soft_delete:
      field: deleted_at
      target: is_deleted
    fields:
      - source: email
        target: email

  # ERP: soft-deletes via is_active flag
  - name: erp_customers
    source: erp
    target: customer
    reverse_filter: "is_deleted IS NOT TRUE"
    soft_delete:
      field: is_active
      strategy: active_flag
      target: is_deleted
    fields:
      - source: email
        target: email
```

When CRM sets `deleted_at = '2026-03-15'`:
1. Forward: `is_deleted = TRUE` (CRM's contribution)
2. Resolution: `bool_or → TRUE`
3. Reverse to CRM: `reverse_filter` → `action = 'delete'` → CRM connector
   handles it (already deleted, noop or re-confirms)
4. Reverse to ERP: `reverse_filter` → `action = 'delete'` → ERP connector
   sets `is_active = FALSE` (or calls archive API, etc.)
5. Next sync: ERP's `is_active = FALSE` → `soft_delete` detects it →
   `is_deleted = TRUE` from ERP too → stable

This eliminates:
- `reverse_expression` on `soft_delete` (not needed)
- Auto-derived boolean write-back logic (not needed)
- IVM/`NOW()` problem (nothing non-deterministic in the view)
- Strategy-specific reverse logic (the engine doesn't care how sources
  represent deletion physically)

The `reverse_filter` + `action = 'delete'` pattern already exists and
works today (see `propagated-delete` example). The only new piece is
`soft_delete.target` on the forward side to feed the deletion signal
into resolution.

### Resurrect: the one reverse write-back

Resurrect is the exception: when a deleted entity comes back (source
row reappears or soft-delete marker is cleared by the source), the
engine needs to clear the soft-delete marker so it doesn't immediately
re-trigger deletion on the next sync.

The `soft_delete` strategy fully determines the undelete value:

| Strategy      | Undelete value |
|---------------|----------------|
| `timestamp`   | `NULL`         |
| `flag`        | `FALSE`        |
| `active_flag` | `TRUE`         |

This is what the current `undelete_value` does, but derived
automatically from the strategy — no user config needed.

The flow for resurrect:

1. CRM sets `deleted_at = '2026-03-15'` → `is_deleted = TRUE` → propagates
2. Later, CRM clears `deleted_at` → `is_deleted = FALSE` from CRM
3. Resolution: `bool_or → FALSE` (no source says deleted anymore)
4. `reverse_filter` passes → entity re-included in delta
5. Delta: `action = 'update'`, writes undelete value to soft-delete
   column (`deleted_at = NULL` for CRM, `is_active = TRUE` for ERP)
6. Stable — entity is alive everywhere again

The resurrect write-back is deterministic (constant values, no `NOW()`)
and already implemented via the current `undelete_value` mechanism. The
`soft_delete` strategy just makes it implicit.

### `reverse_filter` on array child mappings

`reverse_filter` already exists on root mappings and controls entity
inclusion via the delta action CASE. The same property on array child
mappings would control **element** inclusion during array reconstruction.
No new property needed — just extend `reverse_filter` to work in the
nested CTE pipeline.

```yaml
- name: dev_tasks
  parent: dev_projects
  array: tasks
  target: task
  reverse_filter: "is_removed IS NOT TRUE"
  fields: [...]
```

In the nested CTE pipeline, `reverse_filter` applies as a WHERE
condition on the leaf CTE that feeds `jsonb_agg`:

```sql
-- Without reverse_filter
SELECT n._parent_key,
  jsonb_agg(obj ORDER BY ...) AS tasks
FROM _rev_dev_tasks AS n
WHERE n._parent_key IS NOT NULL
GROUP BY n._parent_key

-- With reverse_filter: "is_removed IS NOT TRUE"
SELECT n._parent_key,
  jsonb_agg(obj ORDER BY ...) AS tasks
FROM _rev_dev_tasks AS n
WHERE n._parent_key IS NOT NULL
  AND (is_removed IS NOT TRUE)          -- ← injected from reverse_filter
GROUP BY n._parent_key
```

This reuses the existing property with consistent semantics: on root
mappings, `reverse_filter` controls entity inclusion; on array child
mappings, it controls element inclusion.

## Implementation phases

### Phase 1: `reverse_filter` on array child mappings

Extend `reverse_filter` to work in the nested CTE pipeline for child
mappings with `array:`. When present, the filter expression is added
as a WHERE condition on the leaf CTE that feeds `jsonb_agg()`.

Validation: same rules as `reverse_filter` on root mappings — the
expression must reference target field names.

This can be shipped independently — even without the synthesis adapters,
users with soft-delete sources can map markers to target fields and use
`reverse_filter` on child mappings to control per-source inclusion.

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
(`is_removed = TRUE`). Each consumer's `reverse_filter` decides
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
  synthesizing a field + `reverse_filter` per consumer
- **`tombstone` detect-and-exclude** (for propagation scenarios) →
  becomes `tombstone.target` producing a field

The current behaviors remain available as defaults (no `target`, no
`reverse_filter` → same exclusion semantics). The new properties extend
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
    reverse_filter: "is_removed IS NOT TRUE"  # ← dev tracker wants clean arrays
    fields:
      - source: parent_name
        target: project_ref
        references: dev_projects
      - source: title
        target: title
      - target: task_order
        order: true

  # PM tool child: keeps removed tasks visible (no reverse_filter)
  # PM tool sees is_removed = true and renders with strikethrough
```

### Behavior

1. PM tool cancels "Write docs" → `cancelled_at` set →
   `tombstone.target` maps to `is_removed = TRUE`
2. Resolution: `is_removed` via `bool_or` → TRUE (PM tool says removed)
3. Dev tracker's `reverse_filter` excludes it → "Write docs" removed
   from dev tracker's array
4. PM tool has no `reverse_filter` → "Write docs" stays in PM tool's
   array, with `is_removed = TRUE` available if the ETL wants to render
   it differently

And symmetrically:

1. Dev tracker removes "Fix bug" from array → `derive_element_tombstones`
   synthesizes `is_removed = TRUE` for that element
2. Resolution: `bool_or` → TRUE
3. Dev tracker's `reverse_filter` excludes it
4. PM tool keeps it, sees `is_removed = TRUE`

Each consumer independently decides its reaction. Deletion is data, not
an engine side effect.

## Open questions

1. ~~**Should `reverse_filter` live on the child mapping or the parent?**~~
   Resolved: it's just `reverse_filter` on the child mapping. Same
   property, same semantics — on root mappings it controls entity
   inclusion, on array child mappings it controls element inclusion.
   No new property needed.

2. ~~**How does `derive_tombstones` (entity-level) interact with
   `cluster_members`?**~~ Resolved: `derive_tombstones` requires
   `cluster_members` (or `written_state`) to detect absence. If neither
   is configured, the engine has no record of what was previously present
   and cannot detect disappearance — `derive_tombstones` is a validation
   error without a presence-tracking mechanism.

3. ~~**Default field name.**~~ Resolved: explicit field name required.
   No conventional default — keeps configs self-documenting.

4. **Phase 1 standalone value.** `reverse_filter` on child mappings is
   useful even without the synthesis adapters — users with soft-delete
   sources can already map markers to fields and filter per consumer.
   This argues for shipping Phase 1 first.

5. **Interaction with `soft_delete` refactor.** If SOFT-DELETE-REFACTOR-
   PLAN proceeds (renaming `tombstone` to `soft_delete`), the `target`
   property would be on `soft_delete` instead. The design is the same
   regardless of naming.
