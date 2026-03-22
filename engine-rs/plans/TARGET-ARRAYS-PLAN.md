# Target arrays

**Status:** Planned

Native array-typed target fields (`text[]`, `integer[]`) with per-element
identity, cross-source deduplication, and element-level resolution.

Note: the simple "one source wins for a whole array" case is already solved
by opaque JSON fields (`type: jsonb` + `strategy: coalesce`) — see
[`json-opaque`](../../examples/json-opaque/README.md). This plan covers
what opaque JSON cannot: per-element dedup across sources, scalar ↔ array
wrapping, `element_identity`, and `array_field` child mappings.

## Problem

Today every "list of values" requires a separate child target:

```yaml
targets:
  contact:
    fields:
      email: { strategy: identity }
      name:  { strategy: coalesce }

  phone_entry:                         # separate target just for a list
    fields:
      contact_ref: { strategy: coalesce, references: contact }
      phone:       { strategy: identity }
```

This works, but has consequences:

1. **Target model bloat.** Simple lists (phone numbers, tags, email addresses)
   each need their own target with a FK reference and identity strategy. A
   contact with phones, emails, and tags = 4 targets.

2. **Scalar + list duplication.** When a system only deals with one phone, the
   [MULTI-VALUE-PLAN](MULTI-VALUE-PLAN.md) adds a `primary_phone` coalesce
   field alongside the `phone_entry` child target. With array fields, a single
   `phones` field covers both: scalar consumers map one element, list consumers
   map all of them.

3. **No way to coalesce lists.** The `collect` strategy produces an array of
   all distinct contributions, but it operates on a single scalar column — it
   can't merge multiple arrays from different sources into one deduplicated
   list while preserving per-element identity.

4. **Child target overhead.** Each child target generates a full pipeline
   (forward → identity → resolution → reverse → delta). For simple value
   lists, this is heavy machinery.

## Proposed syntax

```yaml
targets:
  contact:
    fields:
      email:
        strategy: identity
      name:
        strategy: coalesce
      phones:
        type: text[]
        element_identity: [value]          # what makes two phone entries "the same"
        strategy: collect                  # merge arrays from all sources
```

### New field properties

| Property | Type | Description |
|----------|------|-------------|
| `type: text[]` (or `X[]`) | string | Marks the field as an array. The base type (`text`, `integer`, etc.) defines element type. |
| `element_identity` | string[] | Which sub-properties identify a unique element. For scalar arrays, use `[value]` (the element itself). For object arrays (future), list the key fields. |

### How `collect` works on arrays

Today `collect` on a scalar field generates:
```sql
array_agg(DISTINCT "field") FILTER (WHERE "field" IS NOT NULL)
```

On an array field, `collect` should:
1. Unnest each source's contributed array
2. Deduplicate by `element_identity`
3. Re-aggregate into a single array

```sql
-- Resolution for phones (array field, collect strategy)
(SELECT array_agg(DISTINCT val ORDER BY val)
 FROM unnest_contributions AS val
 WHERE val IS NOT NULL) AS "phones"
```

The "unnest_contributions" part comes from gathering all contributing
mappings' forward-view arrays into one pool.

## Forward view: array contribution

A mapping contributing to an array field can come from two shapes:

### Scalar source → array target

```yaml
- name: crm_contacts
  source: { dataset: crm }
  target: contact
  fields:
    - source: phone
      target: phones
```

The forward view wraps the scalar in `ARRAY[...]`:
```sql
ARRAY["phone"] FILTER (WHERE "phone" IS NOT NULL) AS "phones"
```

### Array source → array target

```yaml
- name: cc_contacts
  source: { dataset: contact_center }
  target: contact
  fields:
    - source: phone_numbers    # JSONB array in source
      target: phones
```

The forward view converts the JSONB array:
```sql
(SELECT array_agg(el::text) FROM jsonb_array_elements_text("phone_numbers") el) AS "phones"
```

### Nested source → array target

```yaml
- name: cc_contacts
  source: { dataset: contact_center }
  target: contact
  fields:
    - source: phones[].number       # path into nested array
      target: phones
```

The forward view extracts and collects:
```sql
(SELECT array_agg(el->>'number') FROM jsonb_array_elements("phones") el) AS "phones"
```

### FK table → array target (first-class child mapping)

When a source stores elements in a **separate table** with a foreign key
to the parent, the user shouldn't have to write raw subquery SQL. The
engine can generate the aggregation using an `array_field` child mapping:

```yaml
targets:
  project:
    fields:
      name: { strategy: identity }
      tasks:
        type: jsonb[]
        element_identity: [title]
        strategy: coalesce

mappings:
  # PM tool: tasks embedded in source JSONB — direct mapping
  - name: pm_projects
    source: pm_tool
    target: project
    priority: 1
    fields:
      - source: name
        target: name
      - source: tasks
        target: tasks

  # Dev tracker: tasks in a separate FK table
  - name: dev_projects
    source: dev_tracker
    target: project
    priority: 2
    fields:
      - source: name
        target: name

  # Child mapping: aggregates FK rows into parent's array field
  - name: dev_tasks
    source: dev_task_table             # separate source table
    parent: dev_projects
    array_field: tasks                 # ← targets parent's array FIELD
    parent_fields:
      project_id: id                   # FK join: dev_tasks.project_id = dev_tracker.id
    fields:
      - source: title
      - source: estimate
```

`array_field: tasks` (as opposed to `array: tasks` which means JSONB
array path within the same source) tells the engine: "aggregate these
FK rows into the parent mapping's `tasks` array field." The engine
auto-generates the subquery aggregation in the parent's forward view:

```sql
-- Auto-generated: aggregate dev_task_table rows into tasks array
(SELECT jsonb_agg(jsonb_build_object(
    'title', _child."title",
    'estimate', _child."estimate"
  ) ORDER BY _child."title")
 FROM "dev_task_table" AS _child
 WHERE _child."project_id" = "dev_tracker"."id"
) AS "tasks"
```

This is the same `jsonb_agg(jsonb_build_object(...))` pattern the engine
already generates for nested array reconstruction in the delta pipeline.
The difference is it runs in the **forward** view instead of the delta.

#### Why `array_field` vs `array`

| Property | Meaning | Source shape |
|----------|---------|-------------|
| `array: tasks` | JSONB path within same source row | Embedded: `{tasks: [{...}]}` |
| `array_field: tasks` | Aggregate FK rows into parent's array field | Separate table with FK |

Both use `parent:` and `parent_fields:` to define the parent-child
relationship. The difference is where the child data lives.

#### What the engine generates

The `array_field` child mapping doesn't generate its own full view
pipeline. Instead, it contributes a subquery to the parent mapping's
forward view. The parent's forward view includes the aggregation as a
column expression.

The child mapping's `fields` list defines which columns from the FK
table appear in the JSONB objects. The `parent_fields` defines the
join condition. The `element_identity` on the target field defines
the ORDER BY (for deterministic aggregation).

#### Symmetry with embedded sources

From the target's perspective, both sources contribute the same thing:
a JSONB array. Resolution doesn't care whether PM tool's array came
from an embedded JSONB column or dev tracker's came from an aggregated
FK table. The `strategy: coalesce` picks the highest-priority array;
`strategy: collect` merges elements by `element_identity`.

This means the "structural mismatch" problem from ARRAY-RESOLUTION-PLAN
vanishes — both embedded and FK sources produce the same shape, just
via different forward-view mechanisms. The engine generates the
aggregation for FK sources automatically.

## Resolution view: merging arrays

The resolution view needs to merge arrays from multiple forward views.
With `strategy: collect` on an array field:

```sql
-- Inner: each forward view contributes its phones array
-- Resolution CTE unnests and re-aggregates:
(SELECT array_agg(DISTINCT phone ORDER BY phone)
 FROM (
   SELECT unnest("phones") AS phone
   FROM _contributions
   WHERE "phones" IS NOT NULL
 ) sub
) AS "phones"
```

For `strategy: coalesce` on an array field, the highest-priority non-null
array wins (no merging — same as scalar coalesce but the value is an array).

For `strategy: identity` on an array field, all contributions must be
identical (same as scalar identity but compared as sorted arrays).

## Reverse view: array → source reconstruction

The reverse view needs to turn the resolved array back into the source's
expected shape.

### Array target → scalar source

CRM has a single `phone` column. The reverse view picks one element:

```yaml
- source: phone
  target: phones
  reverse_expression: "phones[1]"    # first element
```

Or the engine could auto-generate this when it detects array → scalar
mapping (take element at index 1, or `NULL` if empty).

### Array target → array source

The JSONB array reconstruction already exists for nested arrays in
`delta.rs` (`JsonNode` tree). Extend it to handle array-typed target fields:

```sql
-- Reverse: resolved phones[] → source JSONB array
(SELECT jsonb_agg(el) FROM unnest("phones") el) AS "phone_numbers"
```

### Array target → nested source

For sources with nested object arrays (e.g., `phones: [{number: "..."}]`),
the reverse wraps each element:

```sql
(SELECT jsonb_agg(jsonb_build_object('number', el))
 FROM unnest("phones") el) AS "phones"
```

## Delta view: noop comparison for arrays

The noop check needs to compare the `_base` snapshot against the resolved
array. Since `_base` stores the raw source value as text:

**Scalar source:**
```sql
_base->>'phone' IS NOT DISTINCT FROM "phones"[1]::text
```

**Array source:** Compare sorted, like `_osi_text_norm` does for nested JSONB:
```sql
_osi_array_norm(_base->'phone_numbers') IS NOT DISTINCT FROM _osi_array_norm(to_jsonb("phones"))
```

A new `_osi_array_norm` helper (or reuse `_osi_text_norm` extended for arrays)
sorts array elements and normalizes types for stable comparison.

## Multi-value revisited

With array fields, the [MULTI-VALUE-PLAN](MULTI-VALUE-PLAN.md) simplifies:

```yaml
targets:
  contact:
    fields:
      email: { strategy: identity }
      name:  { strategy: coalesce }
      phones:
        type: text[]
        element_identity: [value]
        strategy: collect

mappings:
  - name: crm_contacts
    source: { dataset: crm }
    target: contact
    fields:
      - source: email
        target: email
      - source: name
        target: name
      - source: phone
        target: phones              # scalar → array, auto-wrapped

  - name: cc_contacts
    source: { dataset: contact_center }
    target: contact
    fields:
      - source: email
        target: email
      - source: full_name
        target: name
      - source: phone_numbers
        target: phones              # JSONB array → array, auto-converted
```

No `primary_phone`. No `phone_entry` child target. No dual mappings. CRM
contributes its single phone to the array; CC contributes its full list.
The `collect` strategy merges them. CRM's reverse gets `phones[1]` back.

## Scope of changes

### Model
- `model.rs`: Add array detection to `FieldType` (parse `text[]` → base
  type `text`, is_array = true). Add `element_identity` to `TargetFieldDef`.
  Add `array_field` as alternative to `array` on child mappings.
- `mapping-schema.json`: Allow `type: "text[]"` pattern, add
  `element_identity` property. Add `array_field` property to mappings.

### Forward view
- `forward.rs`: When source is scalar and target is array, wrap in
  `ARRAY[...]`. When source is JSONB array, convert via
  `jsonb_array_elements_text`.
- When a mapping has `array_field` child mappings, generate
  `jsonb_agg(jsonb_build_object(...))` subqueries for each child and
  include them as column expressions in the parent's forward view.

### Resolution view
- `resolution.rs`: For `collect` on array fields, generate
  unnest-dedup-reaggregate SQL instead of simple `array_agg(DISTINCT ...)`.
  For `coalesce`/`identity` on array fields, compare/select as arrays.

### Reverse view
- `reverse.rs`: When reversing array → scalar, generate `field[1]` (or
  custom `reverse_expression`). When reversing array → JSONB array,
  generate `to_jsonb(field)`.
- `array_field` child mappings: reverse decomposes the resolved array
  back into individual rows for the FK table.

### Delta view
- `delta.rs`: Array noop comparison using sorted normalization.
- Possibly a new `_osi_array_norm` SQL helper function.

### Validation
- `validate.rs`: Validate `element_identity` fields exist or are `[value]`.
  Validate `strategy: identity` on array fields requires sorted comparison.
  Validate `array_field` targets an array-typed field on the parent's target.
  Validate `array_field` child mapping's fields match element sub-properties.

## Open questions

1. **Object arrays.** Should `element_identity` support compound keys for
   future object-typed array elements? e.g.:
   ```yaml
   addresses:
     type: jsonb[]
     element_identity: [type, zip]
   ```
   This would enable address lists where (type + zip) identifies unique
   entries. Proposal: support it in the schema now, implement scalar-only
   (`[value]`) first.

2. **Ordering.** Should the resolved array preserve insertion order, or
   always sort? Sorting is simpler for noop comparison. If ordering matters,
   we'd need an `order_by` property or positional identity.

3. **Array of objects vs. array of scalars.** Scalar arrays (`text[]`) are
   straightforward. Object arrays (`jsonb[]`) would need per-element field
   mapping — essentially what child targets do today. Proposal: start with
   scalar arrays only.

4. **Coexistence with child targets.** Array fields don't replace child
   targets — child targets remain the right choice for complex entities with
   their own identity, multiple fields, and independent resolution strategies.
   Array fields are for simple value lists.

## Phasing

### Phase 1 — Scalar array fields
- `type: text[]` / `integer[]` etc.
- `element_identity: [value]`
- `strategy: collect` (unnest + dedup + reaggregate)
- `strategy: coalesce` (pick highest-priority array)
- Scalar ↔ array forward/reverse wrapping
- Array noop comparison

### Phase 2 — Object array fields + `array_field` child mappings

> **Re-evaluation note:** Phase 2 was originally motivated by the "atomic
> array" problem. This is now solved by `elements:` on child targets —
> see "Atomic element resolution" below. Phase 2 remains useful for
> reducing target model bloat (inlining simple object arrays into the
> parent target), but is **not required** for element set authority.

- `type: jsonb[]`
- `element_identity: [field1, field2]`
- Per-element field decomposition in forward/reverse
- `array_field` child mappings for FK table → array aggregation
- Essentially inlines child target behavior into the parent
- Subsumes ARRAY-RESOLUTION-PLAN Options 1-3

## Atomic element resolution

The "atomic array" problem: PM tool says tasks are [A, B, C], dev tracker
says [A, D]. With current child targets, both contribute to a `task`
child target and the resolved set is the union [A, B, C, D]. Element D
(only from dev tracker) leaks into PM tool's array.

The root cause: element **set membership** is resolved as a union today.
The `elements` property on a child target controls how element set
membership is resolved — reusing the same vocabulary as field strategies.

### The `elements` property

```yaml
task:
  elements: collect         # union of all sources' elements (default)
  elements: coalesce        # highest-priority source's set wins per parent
  elements: last_modified   # most recently modified source's set wins per parent
```

| `elements` value | Winner signal | Resolved set |
|---|---|---|
| `collect` (default) | No winner — union | All elements from all sources |
| `coalesce` | Child mapping `priority:` | Highest-priority source's elements per parent |
| `last_modified` | `MAX(last_modified field)` per parent per mapping | Most recently active source's elements per parent |

### `elements: coalesce` — priority wins

```yaml
targets:
  project:
    fields:
      name: { strategy: identity }
  task:
    elements: coalesce
    fields:
      project_ref: { strategy: identity, references: project }
      title:       { strategy: identity }
      estimate:    { strategy: coalesce }

mappings:
  - name: pm_tasks
    target: task
    priority: 1                      # ← wins element set
    parent: pm_projects
    array: tasks
    fields:
      - source: title
        target: title
      - source: estimate
        target: estimate

  - name: dev_tasks
    target: task
    priority: 2                      # ← falls back if PM has no elements
    parent: dev_projects
    array: tasks
    fields:
      - source: title
        target: title
      - source: hours
        target: estimate
```

Per parent entity, the engine picks the highest-priority child mapping
that contributed at least one element. Only that mapping's elements
survive in the resolved view:

```sql
-- Per parent, pick highest-priority mapping with elements
_element_winner AS (
  SELECT DISTINCT ON (parent_ref_resolved)
    parent_ref_resolved, _mapping
  FROM (
    SELECT parent_ref_resolved, _mapping, _priority
    FROM _id_task
    GROUP BY parent_ref_resolved, _mapping, _priority
  ) sub
  ORDER BY parent_ref_resolved, _priority
)

-- Only entities contributed by the winning mapping survive
_surviving_elements AS (
  SELECT id._entity_id_resolved
  FROM _id_task id
  JOIN _element_winner w
    ON w.parent_ref_resolved = id.parent_ref_resolved
    AND w._mapping = id._mapping
)
```

| Element | Contributed by | Survives? | Why |
|---|---|---|---|
| Task A | PM (1), Dev (2) | ✓ | PM contributed it, PM wins |
| Task B | PM (1) | ✓ | PM contributed it |
| Task C | PM (1) | ✓ | PM contributed it |
| Task D | Dev (2) | ✗ | PM has elements for this parent, so PM wins; D is not in PM's set |

Field values within surviving elements still resolve normally —
Task A's `estimate` comes from PM (coalesce, priority 1) even though
dev tracker also contributed it. Atomic resolution controls **which
entities exist**, not how their fields resolve.

If PM has zero tasks for a parent but dev tracker has some, dev tracker's
set wins for that parent — same fallthrough as scalar coalesce.

### `elements: last_modified` — timestamp wins

The child target must have a field with `strategy: last_modified`. The
engine uses it to pick the winner: per parent, compute
`MAX(timestamp)` across each mapping's elements, highest wins.

```yaml
targets:
  task:
    elements: last_modified
    fields:
      project_ref: { strategy: identity, references: project }
      title:       { strategy: identity }
      updated_at:  { strategy: last_modified }   # ← drives winner
      estimate:    { strategy: coalesce }
```

Each source maps its native timestamp to this field:

```yaml
  - name: pm_tasks
    target: task
    parent: pm_projects
    array: tasks
    fields:
      - source: modified_at           # PM's native timestamp
        target: updated_at

  - name: dev_tasks
    target: task
    parent: dev_projects
    array: tasks
    fields:
      - source: last_changed          # Dev tracker's native timestamp
        target: updated_at
```

```sql
-- Per parent, pick mapping with most recent element activity
_element_winner AS (
  SELECT DISTINCT ON (parent_ref_resolved)
    parent_ref_resolved, _mapping
  FROM (
    SELECT parent_ref_resolved, _mapping,
           MAX(updated_at) AS last_touch
    FROM _id_task
    GROUP BY parent_ref_resolved, _mapping
  ) sub
  ORDER BY parent_ref_resolved, last_touch DESC NULLS LAST
)
```

Where the timestamp **value** comes from is an ordinary mapping
concern — element-native or propagated from the parent via
`parent_fields`. The winner query is the same either way.

#### Parent-level timestamp via `parent_fields`

When the source only tracks modification at the parent level (common
for embedded elements), propagate it with `parent_fields`:

```yaml
mappings:
  - name: pm_tasks
    parent: pm_projects
    array: tasks
    parent_fields:
      project_id: id
      parent_updated: updated_at     # ← propagate parent source column
    target: task
    fields:
      - source: title
        target: title
      - source: parent_updated        # ← maps to child target field
        target: updated_at
      - source: estimate
        target: estimate
```

All elements from the same parent source row get the same timestamp,
so `MAX(updated_at)` degenerates to a scalar lookup — but the SQL is
identical. The engine doesn't need to distinguish the two cases.

Sources without native timestamps use `derive_timestamps` (which
reads from `written_state`) — same anti-corruption pattern as scalar
`last_modified` strategy.

Validation: `elements: last_modified` requires at least one field on the
child target with `strategy: last_modified`. Otherwise it's a validation
error.

### Resolution vs. reverse cleanup

Atomic element resolution operates at the **resolution** layer. The
`_resolved_task` view itself only contains surviving elements. This
means:

- **Analytics queries** on `_resolved_task` see the authoritative set
- **Reverse views** only produce rows for surviving elements
- **Delta views** detect elements that previously survived but no longer
  do (because the winner changed) and emit deletes — using the existing
  deletion detection CTEs that compare written state against current

No special reverse-layer filtering needed. The resolved view is the
single source of truth; reverse and delta consume it naturally.

### Implementation

```rust
// model.rs
pub struct TargetDef {
    pub fields: IndexMap<String, TargetFieldDef>,
    pub elements: Option<ElementStrategy>,
    // ...
}

/// How element set membership is resolved for child targets.
/// Reuses strategy vocabulary: collect (union), coalesce (priority),
/// last_modified (timestamp).
pub enum ElementStrategy {
    Collect,       // union of all sources' elements (default)
    Coalesce,      // highest-priority mapping's set wins per parent
    LastModified,  // most recently active mapping's set wins per parent
}
```

In `resolution.rs`, when the child target has `elements: coalesce` or
`elements: last_modified`, generate the `_element_winner` and
`_surviving_elements` CTEs. The main resolution query adds
`WHERE _entity_id_resolved IN (SELECT * FROM _surviving_elements)`.

## Interaction with other plans

- **MULTI-VALUE-PLAN**: Array fields eliminate the need for `primary_phone` +
  separate `phone_entry` child target.
- **EXPRESSION-SAFETY-PLAN**: Array field expressions are column-level
  snippets — no change to validation rules.
- **PRECISION-LOSS-PLAN**: `normalize` applies per-element when comparing
  arrays.
- **DELETION-AS-FIELD-PLAN**: `reverse_filter` on child mappings composes
  with `elements:` — `elements:` determines which entities survive
  resolution, `reverse_filter` further controls which appear in each
  source's reconstructed array.  `derive_element_tombstones` can
  synthesize deletion markers into elements.
- **ARRAY-RESOLUTION-PLAN**: The `elements:` property on child targets
  solves element set authority without Phase 2 or sentinel fields. Phase 2
  remains useful for target model reduction (inlining simple object arrays)
  but is no longer the critical path for authority semantics.
