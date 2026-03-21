# Unify derive_tombstones across entities and elements

**Status:** Planned

Three changes, all following from "deletion is data, not a side-channel":

1. Replace `derive_element_tombstones: bool` with `derive_tombstones`
   on child mappings — same property as root mappings, context
   determines scope
2. Remove `resurrect` — superseded by the field-based model
3. Remove implicit suppression from `cluster_members` / `written_state`

Pre-1.0 — all three are breaking changes without backwards compatibility.

## Principle: elements are entities

Elements (array children) and root entities are fundamentally the same
thing in the resolution model.  Both:

- Can merge contributions from multiple sources
- Can have one source win (coalesce, last_modified) or combine (bool_or)
- Can be identity-linked across sources
- Can be created, updated, and deleted

This applies equally to **object arrays** (`steps: [{instruction: "..."}]`)
and **scalar arrays** (`tags: ["a", "b"]`).  A scalar list item is just an
element whose single value doubles as its identity — same pipeline, same
semantics.

The delta already emits `action = 'delete'` for root entities when
`reverse_filter` excludes them.  The same mechanism should work for
elements: absence synthesizes a field, resolution combines it, each
consumer's `reverse_filter` decides whether the element survives —
and if not, the delta emits a delete for that element in that consumer's
array.

The current `derive_element_tombstones: true` violates this by:
1. Using a different property name for the same concept
2. Living on the parent instead of the child
3. Being a boolean side-channel instead of a field-based adapter

This plan covers object arrays (child mappings with `parent:` + `array:`).
Scalar arrays that go through the same child-mapping pathway get the
same behavior automatically.  See [SCALAR-ARRAY-DELETION-PLAN](SCALAR-ARRAY-DELETION-PLAN.md)
for the broader question of scalar arrays that don't yet use child
mappings.

## Motivation

After this change, one property works at every level:

```yaml
mappings:
  # Root mapping: absent entity → is_deleted = TRUE
  - name: erp_customers
    source: erp
    target: customer
    cluster_members: true
    derive_tombstones: is_deleted
    fields: [...]

  # Child mapping: absent element → is_removed = TRUE
  - name: blog_cms_steps
    parent: blog_cms_recipes
    array: steps
    target: recipe_step
    derive_tombstones: is_removed
    fields: [...]
```

Each consumer's `reverse_filter` independently decides whether to
include or exclude — same semantics at both levels.

## Current behavior

`derive_element_tombstones: true` on the **parent** mapping (wrong
property, wrong level):

1. Compare current child forward view against parent's `written_state`
   JSONB arrays to find absent elements (the `_del_prev → _del_curr →
   _del_src` CTE pipeline in delta.rs)
2. Merge absent-element identities with soft-delete-detected elements
   into a single `DeletionFilter` per segment
3. Apply the filter via LEFT JOIN + WHERE IS NULL in the nested
   reconstruction CTEs — absent elements excluded from ALL sources'
   arrays unconditionally

## New behavior

`derive_tombstones: is_removed` on the **child** mapping (same property
as entity-level, lives on the mapping of the thing being detected):

1. Same absence detection pipeline (reads parent's `written_state`)
2. Instead of producing a DeletionFilter exclusion, inject the absent
   elements back into the child forward view with `is_removed = TRUE`
   and all other fields NULL
3. Resolution combines via the target field's strategy (typically
   `bool_or`)
4. Each consumer's child mapping uses `reverse_filter` to decide
   whether to include or exclude

The engine resolves the dependency on the parent's `written_state`
internally — the child declares what it needs, the parent provides
the storage.

## Implementation phases

### Phase 1: Model + schema + validation

**Files:** model.rs, mapping-schema.json, schema-reference.md, validate.rs

1. Remove `derive_element_tombstones: bool` from `Mapping`
2. Remove `resurrect: bool` from `Mapping`
3. Remove `suppress_resurrect()` and all call sites
4. `derive_tombstones: Option<String>` already exists on `Mapping` —
   it now works on both root and child mappings
5. Update mapping-schema.json: remove `derive_element_tombstones`
   and `resurrect`
6. Update schema-reference.md: unify into one `derive_tombstones`
   section covering both root and child usage; remove `resurrect`
   section
7. Update validation:
   - Root mapping: requires `cluster_members` (existing)
   - Child mapping: requires parent with `written_state`
   - Both: named field must exist on the mapping's own target
8. Pre-1.0 breakage: `derive_element_tombstones` and `resurrect` gone

Failing existing tests at this point is expected (element-hard-delete
uses `derive_element_tombstones: true` on the parent; hard-delete uses
`resurrect: true`).

### Phase 2: Forward view synthesis for child mappings

**Files:** forward.rs

When `derive_tombstones` is set on a child mapping, add a UNION ALL
to that child's forward view (mirroring the entity-level pattern):

For each child mapping with `derive_tombstones`:
1. Extract previously-written elements from parent's `_written` JSONB
   (same `_del_prev` CTE logic currently in delta.rs)
2. Anti-join against the current child forward view (same `_del_curr`
   + `_del_src` pattern)
3. For absent elements, emit a synthetic row:
   - Identity fields from the written state
   - `{target_field} = TRUE` (cast to the field's declared type)
   - All other non-identity fields NULL
   - Parent FK field preserved for grouping

This means the element detection pipeline moves from delta.rs to
forward.rs — absence is detected at the forward stage and represented
as data, not as an exclusion filter at the delta stage.

### Phase 3: Delta changes — remove DeletionFilter for the field case

**Files:** delta.rs

When `derive_tombstones` is set on a child mapping:
1. Skip the `_del_prev → _del_curr → _del_src` CTE pipeline for this
   child's segment (the forward view already handles it)
2. Skip the DeletionFilter LEFT JOIN + WHERE IS NULL for this segment
3. The nested reconstruction sees the synthetic rows naturally — they
   have `is_removed = TRUE` and participate in resolution

When `derive_tombstones` is None on a child mapping:
- No change — the existing soft_delete DeletionFilter path continues
  to work for explicit soft-delete markers

Note: the existing `soft_delete` detect-and-exclude path (child
mappings with `soft_delete` and no `target`) should continue to produce
DeletionFilter entries.  Only the `derive_tombstones` path changes.

### Phase 4: Update examples

**Files:** examples/element-hard-delete/

Update the example to use `derive_tombstones` on the child mapping:

```yaml
targets:
  recipe_step:
    fields:
      instruction: { strategy: identity, link_group: step_identity }
      is_removed: { strategy: bool_or, type: boolean }
      step_order: { strategy: coalesce }
      duration: { strategy: coalesce }

mappings:
  - name: blog_cms_recipes
    source: blog_cms
    target: recipe
    written_state: true          # parent provides storage
    fields: [...]

  - name: blog_cms_steps
    parent: blog_cms_recipes
    array: steps
    target: recipe_step
    derive_tombstones: is_removed  # same property, element context
    reverse_filter: "is_removed IS NOT TRUE"
    fields: [...]
```

Update the README to describe the unified behavior.

### Phase 5: Update element-soft-delete example (if affected)

Check whether `examples/element-soft-delete/` uses
`derive_element_tombstones`.  If it uses only `soft_delete` on child
mappings (explicit markers, detect-and-exclude), no change needed.
If it uses `derive_element_tombstones: true`, update to
`derive_tombstones: <field>`.

### Phase 6: Verify and clean up

1. `cargo fmt --check`
2. `cargo clippy --tests -- -D warnings`
3. `cargo test` — all unit + integration tests pass
4. Verify the element-hard-delete integration test demonstrates
   per-consumer control (one consumer filters, another keeps)

## Interaction with existing features

### `soft_delete` on child mappings (detect-and-exclude)

Unchanged.  Child mappings with `soft_delete` and no `target` continue
to produce DeletionFilter entries via the existing code path.  This is
local suppression, not cross-source propagation.

### `soft_delete.target` on child mappings (detect-as-field)

Orthogonal.  `soft_delete.target` routes explicit source markers into
a field.  `derive_tombstones` synthesizes the same kind of field for
sources that lack markers.  Both can coexist on different child
mappings for the same target:

- Source A child: `soft_delete: { field: cancelled_at, target: is_removed }`
- Source B child: `derive_tombstones: is_removed` (synthesizes from absence)

Resolution combines them: `is_removed: { strategy: bool_or }`.

### `reverse_filter` on child mappings

Already works on child mappings (controls element inclusion in the
reconstructed array).  `derive_tombstones` just gives it something to
filter on.

### `derive_noop` / `derive_timestamps`

No conflict.  Orthogonal adapters operating on different concerns.

### `resurrect` — removed

`resurrect` only existed to counteract implicit suppression.  Today,
`cluster_members` or `written_state` automatically suppresses absent
entities from the delta (`suppress_resurrect()` in model.rs).
`resurrect: true` opts out of that suppression.

With the field-based model, there is no implicit suppression to opt
out of:

| Has `cluster_members` | Has `derive_tombstones` | What happens on absence |
|---|---|---|
| yes | no | Nothing — source stops contributing, other sources win |
| yes | `is_deleted` | `is_deleted = TRUE` synthesized → resolution → `reverse_filter` per consumer |
| no | n/a | Engine can't detect absence — no feedback table |

`cluster_members` becomes purely about insert feedback (linking new
rows to clusters).  If you want to react to absence, you explicitly
add `derive_tombstones`, which makes it a field that flows through
resolution.  No implicit suppression, no `resurrect` to toggle it.

The hard-delete example simplifies:

```yaml
# Today: CRM needs resurrect: true to opt out of implicit suppression
- name: crm_customers
  cluster_members: true
  resurrect: true              # just to prevent side-channel suppression

# New: CRM just has cluster_members for insert feedback
- name: crm_customers
  cluster_members: true        # no implicit suppression, no resurrect needed
```

Implementation:
- Remove `resurrect: bool` from `Mapping`
- Remove `suppress_resurrect()` from model.rs
- Remove all `suppress_resurrect()` call sites in delta.rs
- Remove `resurrect` from mapping-schema.json and schema-reference.md
- Update hard-delete example (remove `resurrect: true` from CRM)

## Migration

Pre-1.0 — three breaking changes:

1. `derive_element_tombstones: true` (on parent) → removed.  Replace
   with `derive_tombstones: <field>` on the child mapping and add the
   field to the child target with `strategy: bool_or`.
2. `resurrect: true` → removed.  Just delete it — no implicit
   suppression means no need to opt out.
3. `resurrect: false` (default) → removed.  Replace with
   `derive_tombstones: <field>` + `reverse_filter` to get the same
   "don't re-insert absent entities" behavior explicitly.

## Risks

- **Forward view complexity for nested arrays**: the UNION ALL
  synthesis must correctly handle parent FK linkage and multi-level
  nesting.  Start with single-level (`array:`) and extend to
  `array_path:` if needed.
- **Written state format assumptions**: the absence detection pipeline
  reads parent's `_written` JSONB arrays.  Moving it to forward.rs
  means the child forward view depends on the parent's `written_state`
  — currently only delta.rs joins `_written`.  This is a new
  dependency path to validate.
