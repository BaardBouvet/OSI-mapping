# CRDT ordering for array elements

**Status:** Planned

Supersedes [POSITIONAL-ARRAY-PLAN](POSITIONAL-ARRAY-PLAN.md).

Deterministic, conflict-free ordering for flattened nested array elements.
Separates ordering from identity — elements are identified by content, ordered
by CRDT-style position metadata.

## Problem

### Arrays without natural identity (same as POSITIONAL-ARRAY)

Many nested arrays have no natural key:

```
tags: ["urgent", "customer-facing", "q2"]
address_lines: ["123 Main Street", "Suite 400"]
steps: [{ instruction: "Preheat to 200°C" }, { instruction: "Mix" }]
```

Today you must invent an identity field or use the value itself (fragile for
non-unique values).

### The fundamental flaw of positional identity

The POSITIONAL-ARRAY-PLAN proposed making the array index itself the identity
field (`strategy: identity`). This is dangerous:

```
System A: steps = ["Preheat", "Mix", "Bake"]     → positions 0, 1, 2
System B: steps = ["Preheat", "Sift", "Mix", "Bake"]  → positions 0, 1, 2, 3
```

Position 1 in A is "Mix" but position 1 in B is "Sift". Using position as
identity merges "Mix" with "Sift" — a silent data corruption. The
POSITIONAL-ARRAY-PLAN acknowledged this risk but only recommended a warning.

### Multi-source ordering conflict

Even when elements have proper content-based identity, ordering can differ:

```
System A: priorities = [Bug, Feature, Docs]
System B: priorities = [Feature, Bug, Docs, Tests]
```

After identity merge, what order should the result be? Whose ordering wins?
What position does "Tests" (only in B) get relative to elements from A?

## Key design decision: position ≠ identity

**Position is metadata, not identity.** Elements are identified by their
content fields (via `strategy: identity`). Position is a separate `coalesce`
field that determines reconstruction order. This is the core departure from
POSITIONAL-ARRAY-PLAN.

## Design

Two tiers. Tier 1 covers most practical cases. Tier 2 adds full CRDT merge
semantics for complex multi-source ordering.

### Tier 1 — Ordinal ordering

A field property `order: true` that auto-populates the field with a sortable
position key derived from the array element's index.

```yaml
- name: source_steps
  parent: source_recipes
  array: steps
  parent_fields:
    parent_recipe: name
  target: recipe_step
  fields:
    - source: parent_recipe
      target: recipe_name
      references: source_recipes
    - target: step_order
      order: true              # ← sortable position key from array index
    - source: instruction
      target: instruction
    - source: duration
      target: duration
```

Target configuration — note `step_order` is NOT identity:

```yaml
targets:
  recipe_step:
    fields:
      recipe_name:
        strategy: identity
      instruction:
        strategy: identity     # content-based identity
      step_order:
        strategy: coalesce     # ordering metadata, not identity
      duration:
        strategy: coalesce
```

**What `order: true` generates:**

A zero-padded text key from `WITH ORDINALITY`:

```sql
lpad((item.idx - 1)::text, 10, '0')
```

This produces `'0000000000'`, `'0000000001'`, ... — lexicographically sortable,
text-typed (works with JSONB), and leaves room for insertion between elements.

**Merge semantics for Tier 1:**

Since `step_order` uses `strategy: coalesce`, the highest-priority source's
ordering wins. Elements unique to a lower-priority source keep their
original position. This is simple last-writer-wins (LWW) on the ordering.

**Array reconstruction:**

Reverse/delta views reconstruct arrays with:

```sql
jsonb_agg(... ORDER BY "step_order")
```

### Tier 2 — Linked-list CRDT ordering

For true conflict-free interleaving from multiple sources, each element
carries references to its previous and next siblings. This is the
"posisjon med forrige og neste plass" approach — a doubly-linked list CRDT
that handles concurrent insertions and reorderings.

**Additional field properties:** `order_prev: true` and `order_next: true`.

```yaml
- name: source_steps
  parent: source_recipes
  array: steps
  parent_fields:
    parent_recipe: name
  target: recipe_step
  fields:
    - source: parent_recipe
      target: recipe_name
      references: source_recipes
    - target: step_order
      order: true
    - target: step_prev
      order_prev: true         # ← identity of previous sibling
    - target: step_next
      order_next: true         # ← identity of next sibling
    - source: instruction
      target: instruction
    - source: duration
      target: duration
```

Target side:

```yaml
targets:
  recipe_step:
    fields:
      recipe_name:
        strategy: identity
      instruction:
        strategy: identity
      step_order:
        strategy: coalesce
      step_prev:
        strategy: coalesce     # prev sibling's instruction (identity field)
      step_next:
        strategy: coalesce     # next sibling's instruction (identity field)
      duration:
        strategy: coalesce
```

**What `order_prev` / `order_next` generate:**

Window functions over the identity field(s), ordered by array position:

```sql
LAG("instruction")  OVER (PARTITION BY "recipe_name" ORDER BY item.idx) AS "step_prev",
LEAD("instruction") OVER (PARTITION BY "recipe_name" ORDER BY item.idx) AS "step_next"
```

For composite identity (multiple identity fields), the prev/next values are
a JSON object of the neighbor's identity fields. The engine determines the
identity fields from the target definition.

**Merge semantics for Tier 2:**

After resolution, a new `_ordered_{target}` view topologically sorts elements
using the prev/next links:

```sql
CREATE OR REPLACE VIEW _ordered_recipe_step AS
WITH RECURSIVE chain AS (
    -- Head: elements with no predecessor
    SELECT *, 1 AS _pos
    FROM _resolved_recipe_step r
    WHERE r."step_prev" IS NULL
       OR NOT EXISTS (
           SELECT 1 FROM _resolved_recipe_step p
           WHERE p."instruction" = r."step_prev"
             AND p."recipe_name" = r."recipe_name"
       )

    UNION ALL

    -- Follow the next-sibling chain
    SELECT r.*, c._pos + 1
    FROM _resolved_recipe_step r
    JOIN chain c
      ON r."recipe_name" = c."recipe_name"
     AND r."step_prev" = c."instruction"
    WHERE c._pos < 1000  -- safety limit
)
SELECT *, lpad(_pos::text, 10, '0') AS "_crdt_order"
FROM chain;
```

This produces a deterministic total order by following the linked list.

**Conflict resolution in the chain:**

When two sources contribute conflicting prev/next links:

1. **Fork** — two elements claim the same predecessor. Tie-break by source
   priority (higher priority source's link wins), then by the element's
   identity value (lexicographic, deterministic).

2. **Cycle** — a bug or data corruption created a loop. The `WHERE _pos < 1000`
   guard prevents infinite recursion. The engine emits a warning for cycles.

3. **Orphan** — an element references a predecessor that doesn't exist (e.g.
   it was deleted in another source). Treat as a new head — it starts its own
   chain segment.

**View pipeline placement:**

```
forward → identity → resolution → _ordered_ → reverse → delta
                                      ↑
                              (only for targets with order_prev/order_next)
```

The `_ordered_` view replaces `_resolved_` as input to reverse views for
targets that use linked ordering.

## Example: Recipe steps from two systems

**Recipe DB** — 3 steps with instructions and duration:
```json
{ "name": "Chocolate Cake", "steps": [
    { "instruction": "Preheat to 200°C", "duration": 10 },
    { "instruction": "Mix dry ingredients", "duration": 5 },
    { "instruction": "Bake 30 min", "duration": 30 }
]}
```

**Blog CMS** — same recipe, but with an extra step inserted between "Preheat"
and "Mix":
```json
{ "recipe_name": "Chocolate Cake", "steps": [
    "Preheat to 200°C",
    "Grease the pan",
    "Mix dry ingredients",
    "Bake 30 min"
]}
```

### With Tier 1 (ordinal ordering)

Blog has priority. Elements matched by `instruction` identity:

| instruction | step_order (DB) | step_order (Blog) | resolved |
|---|---|---|---|
| Preheat to 200°C | 0000000000 | 0000000000 | 0000000000 |
| Grease the pan | — | 0000000001 | 0000000001 |
| Mix dry ingredients | 0000000001 | 0000000002 | 0000000002 |
| Bake 30 min | 0000000002 | 0000000003 | 0000000003 |

Blog's ordering wins (higher priority). Result: Preheat, Grease, Mix, Bake.

Duration for "Grease the pan" is NULL (Blog has no duration).

### With Tier 2 (linked CRDT ordering)

Both sources contribute prev/next links without priority:

| instruction | prev (DB) | next (DB) | prev (Blog) | next (Blog) |
|---|---|---|---|---|
| Preheat | NULL | Mix | NULL | Grease |
| Grease | — | — | Preheat | Mix |
| Mix | Preheat | Bake | Grease | Bake |
| Bake | Mix | NULL | Mix | NULL |

After coalesce (Blog priority for prev/next):

| instruction | step_prev | step_next |
|---|---|---|
| Preheat | NULL | Grease |
| Grease | Preheat | Mix |
| Mix | Grease | Bake |
| Bake | Mix | NULL |

Topological sort: Preheat → Grease → Mix → Bake. Same result as Tier 1 in
this case, but Tier 2 handles cases where element priority differs from
ordering priority.

## When to use which tier

| Scenario | Tier 1 | Tier 2 |
|----------|--------|--------|
| Single source, needs stable order | ✓ | overkill |
| Multiple sources, one authoritative ordering | ✓ | overkill |
| Multiple sources, independent inserts | partial | ✓ |
| Collaborative editing (concurrent reordering) | ✗ | ✓ |
| Elements have no content-based identity | needs generated IDs | needs generated IDs |

**Note:** Both tiers require content-based identity on the target. If elements
have no natural key (e.g. duplicate tag values), you need to introduce a
synthetic identity field — possibly from the source's own ID scheme or a
content hash.

## What `order: true` does NOT do (vs. old `position: true`)

| Property | `position: true` (old) | `order: true` (new) |
|---|---|---|
| Used as identity? | Yes | **No** |
| Strategy | `identity` | `coalesce` |
| Multi-source safe? | Only if synchronized | Yes |
| Supports insertion? | No (integers shift) | Yes (text keys, fractional) |
| Sibling references? | No | Optional (Tier 2) |

## Implementation

### Phase 1 — Tier 1: Ordinal ordering

**Model changes (`model.rs`):**

Add `order: bool` to `FieldMapping`. Mutually exclusive with `source`,
`source_path`, and `expression`. Same mutual-exclusivity rules as
`position: true` from the old plan.

**Parser validation (`validate.rs`):**

- `order: true` fields must NOT be `strategy: identity` on the target.
- `order: true` only valid on nested mappings (those with `parent:`).
- At most one `order: true` field per mapping.

**Forward view (`forward.rs`):**

When any field has `order: true`, add `WITH ORDINALITY` to the LATERAL join:

```sql
CROSS JOIN LATERAL jsonb_array_elements("steps") WITH ORDINALITY AS item(value, idx)
```

For multi-segment paths, ordinality applies to the last segment only.

The field expression: `lpad((item.idx - 1)::text, 10, '0')`.

**Reverse/delta (`reverse.rs`, `delta.rs`):**

`jsonb_agg(... ORDER BY "step_order")` — the order field drives array
reconstruction order.

### Phase 2 — Tier 2: Linked-list CRDT

**Model changes (`model.rs`):**

Add `order_prev: bool` and `order_next: bool` to `FieldMapping`. Same
mutual-exclusivity as `order`. Requires that the mapping also has an
`order: true` field (Tier 2 builds on Tier 1).

**Parser validation (`validate.rs`):**

- `order_prev` and `order_next` must appear together (both or neither).
- The target must have at least one `strategy: identity` field (needed for
  the neighbor references).

**Forward view (`forward.rs`):**

Generate `LAG()` / `LEAD()` window functions. The value is the identity
field(s) of the sibling element. For single identity field:

```sql
LAG("instruction") OVER (
    PARTITION BY "recipe_name" ORDER BY item.idx
) AS "step_prev"
```

For multiple identity fields, generate a JSONB object of the neighbor's
identity values:

```sql
LAG(jsonb_build_object('field_a', "field_a", 'field_b', "field_b")) OVER (
    PARTITION BY {parent_key} ORDER BY item.idx
) AS "step_prev"
```

**New view layer (`render/ordered.rs`):**

`_ordered_{target}` view with `WITH RECURSIVE` topological sort. Placed
between resolution and reverse in the DAG. Only generated for targets that
have `order_prev` / `order_next` fields.

The `dag.rs` dependency wiring routes reverse views to read from `_ordered_`
instead of `_resolved_` when the target uses Tier 2 ordering.

**Conflict handling:**

- **Forks:** `ROW_NUMBER()` tie-breaker within the recursive CTE.
- **Cycles:** `_pos < 1000` depth guard + diagnostic warning.
- **Orphans:** Secondary `UNION` in the base case catches elements whose
  predecessor doesn't exist.

### Phase 3 — Example and tests

Example: `examples/crdt-ordering/`

Test cases:
1. Single source with ordinal ordering — round-trips correctly.
2. Two sources, element inserted by one — merged array has correct interleaved
   order.
3. Two sources with different orderings — priority determines winner (Tier 1)
   or topological sort resolves (Tier 2).
4. Deletion in one source — element removed, remaining elements maintain order.
5. Scalar arrays (string lists) — `order: true` works with `item.value #>> '{}'`
   as identity.

### Phase 4 — Delta reconstruction

`delta.rs` changes for ordered arrays:

```sql
jsonb_agg(
    jsonb_build_object('instruction', ..., 'duration', ...)
    ORDER BY "step_order"
) AS steps
```

The `step_prev` and `step_next` fields are NOT included in the reconstructed
JSONB — they are internal ordering metadata stripped before output.

**Exception — see "Reverse ETL to CRDT-aware systems" below.**

## Reverse ETL to CRDT-aware systems

### The problem

A reverse ETL process pushing changes to a CRDT-aware system typically uses
an API that expects positioning operations, not a full array replacement:

- **Figma/Notion-style:** "set element X's `fractional_index` to `a0V`"
- **Automerge/Yjs-style:** "insert element X after element Y"
- **Operational:** "move element X to position 3"

The reconstructed `jsonb_agg` delta gives the full array in correct order —
sufficient for systems that accept full replacement. But CRDT-aware systems
need the **individual element ordering metadata** so the ETL tool can
construct the right API calls.

### Solution: reverse view exposes ordering columns

The **reverse view** (`_reverse_{source}_{target}`) is per-row — it already
has one row per array element before `jsonb_agg` aggregation. This view
includes all mapped fields, including ordering metadata:

```sql
-- _reverse_editor_steps already contains:
SELECT
    "recipe_name",
    "instruction",
    "duration",
    "step_order",      -- ← resolved position key
    "step_prev",       -- ← resolved prev sibling identity
    "step_next"        -- ← resolved next sibling identity
FROM _resolved_recipe_step  -- (or _ordered_recipe_step for Tier 2)
WHERE ...
```

An ETL tool reading `_reverse_editor_steps` directly gets per-element rows
with ordering context — enough to construct "insert after Y" or "set
position to Z" API calls.

### What about NEW elements from another source?

When Source B contributes an element that Source A doesn't have, the reverse
view for Source A includes it (that's the whole point of reverse views —
push merged data back). But what fractional index should the ETL tool
assign?

The engine provides:
- `step_order` — the resolved position key (might be a generated ordinal
  like `"0000000003"` if no CRDT-aware source contributed this element)
- `step_prev` / `step_next` — the identity of the neighboring elements

The ETL tool uses `step_prev` and `step_next` to determine WHERE in the
CRDT sequence to insert. It then generates an appropriate position key
for the target system's CRDT format (e.g., a fractional index between
the prev and next elements). **The engine doesn't generate
system-specific CRDT keys** — it provides the relative ordering, and
the ETL tool translates.

### Field-level control: `direction:`

The `direction:` property on field mappings already controls which fields
flow to which views. For ordering metadata fields:

- `direction: forward_only` — ordering metadata used internally for sorting
  but NOT included in the reverse view. Use for full-replacement targets.
- `direction: bidirectional` (default for `coalesce`) — ordering metadata
  flows to the reverse view. Use when the ETL tool needs positioning info.

This means the mapping author explicitly opts into exposing CRDT metadata
per source:

```yaml
# Source A: CRDT-aware, ETL needs positioning info
- name: editor_steps
  ...
  fields:
    - source: fractional_index
      target: step_order            # bidirectional by default
    - source: instruction
      target: instruction

# Source B: full-replacement, doesn't need ordering metadata back
- name: cms_steps
  ...
  fields:
    - target: step_order
      order: true
      direction: forward_only       # ← don't push order back
    - source: instruction
      target: instruction
```

### Delta JSONB for CRDT-aware sources

For sources where `step_order` is bidirectional, the delta JSONB includes
the ordering field in the reconstructed objects:

```sql
jsonb_agg(
    jsonb_build_object(
        'instruction', "instruction",
        'duration', "duration",
        'fractional_index', "step_order"   -- ← included because bidirectional
    )
    ORDER BY "step_order"
) AS steps
```

For sources where `step_order` is `forward_only`, it's excluded from the
JSONB objects but still drives `ORDER BY`.

### Summary

| Output | Ordering metadata included? | Use case |
|--------|----------------------------|----------|
| Forward view | Always | Pipeline internal |
| Analytics view | `step_order` only | Consumer queries |
| Reverse view (per-row) | If bidirectional | ETL tool reads rows directly |
| Delta JSONB (aggregated) | If bidirectional | Full object reconstruction |
| Delta JSONB ORDER BY | Always | Array element ordering |

## Analytics view

The analytics view (`_analytics_{target}`) includes `step_order` — consumers
querying the golden record can `ORDER BY "step_order"` to get elements in
resolved order. The `step_prev` and `step_next` fields are excluded from
analytics output — they are internal plumbing for the topological sort.

## Data flow summary

All CRDT metadata is **computed by the engine**, not sourced from input data.
Sources provide only the array and its elements. The engine synthesizes
ordering information during forward view rendering:

```
Source array  ───▶  forward view (WITH ORDINALITY → order, LAG/LEAD → prev/next)
                         │
                    normal columns flow through pipeline
                         │
                    identity + resolution (order/prev/next are coalesce fields)
                         │
                    _ordered_ view (Tier 2: topological sort from prev/next)
                         │
               ┌─────────┴──────────┐
               ▼                    ▼
         analytics view       reverse / delta
         (includes order,     (jsonb_agg ORDER BY order,
          excludes prev/next)  strips prev/next from JSONB)
```

No special source format is required. No CRDT fields appear in output JSONB.
The ordering is applied implicitly via array element order in the
reconstructed `jsonb_agg` output.

## Integrating with sources that have CRDT metadata

Some source systems already carry ordering metadata — fractional indices
(Figma, Notion), prev/next linked-list pointers (Automerge, Yjs list types),
Lamport timestamps, or explicit sort keys. The engine should consume these
directly rather than re-deriving order from array position.

### Pattern: map external CRDT fields with `source:`

When a source already has ordering metadata, use normal `source:` mappings
to bring it into the same target fields that `order:` / `order_prev:` /
`order_next:` would populate:

```yaml
# Source A: has fractional index keys (e.g. from a CRDT editor)
- name: editor_steps
  parent: editor_recipes
  array: steps
  parent_fields:
    parent_recipe: name
  target: recipe_step
  fields:
    - source: parent_recipe
      target: recipe_name
      references: editor_recipes
    - source: fractional_index      # ← external CRDT position key
      target: step_order            #   maps to same field as order: true
    - source: instruction
      target: instruction
    - source: duration
      target: duration

# Source B: plain array, no CRDT metadata — engine generates order
- name: cms_steps
  parent: cms_recipes
  array: steps
  parent_fields:
    parent_recipe: recipe_name
  target: recipe_step
  fields:
    - source: parent_recipe
      target: recipe_name
      references: cms_recipes
    - target: step_order
      order: true                   # ← generated from array position
    - source: instruction
      target: instruction
```

Both sources feed `step_order` on the same target. Resolution applies
`strategy: coalesce` — the higher-priority source's ordering wins.

### Prev/next from external sources

For sources that store linked-list CRDT pointers (prev/next sibling
references), map them directly:

```yaml
- name: collab_steps
  parent: collab_recipes
  array: steps
  parent_fields:
    parent_recipe: name
  target: recipe_step
  fields:
    - source: parent_recipe
      target: recipe_name
      references: collab_recipes
    - source: sort_key              # external fractional index
      target: step_order
    - source: prev_instruction      # external prev pointer
      target: step_prev
    - source: next_instruction      # external next pointer
      target: step_next
    - source: instruction
      target: instruction
```

The `_ordered_` view (Tier 2) works identically — it doesn't care whether
`step_prev`/`step_next` came from `order_prev: true` (generated by
`LAG()`/`LEAD()`) or from `source: prev_instruction` (external data). The
topological sort consumes whatever values land in those target fields after
resolution.

### Mixed sources: generated + external CRDT

The most interesting case: Source A has CRDT metadata, Source B doesn't.

```
Source A (CRDT editor):  step_order = "a0V", prev = NULL,      next = "Mix"
Source B (plain array):  step_order = "0000000000" (generated), prev = NULL (generated), next = "Mix" (generated)
```

After identity resolution, the coalesce picks the higher-priority source's
values. This means:

- If the CRDT-aware source has higher priority, its fractional indices and
  explicit prev/next links drive the ordering — the engine preserved the
  richer metadata.
- If the plain-array source has higher priority, generated ordinals and
  generated prev/next links drive ordering — the CRDT metadata is overridden.

This is the correct behavior: priority determines whose world-view wins,
regardless of whether that view was computed or native.

### Fractional index compatibility

External fractional indices (e.g. `"a0"`, `"a0V"`, `"a1"`) are already
lexicographically sortable text — they work directly as `step_order` values.
The engine's generated `lpad` keys (`"0000000000"`, `"0000000001"`, ...) are
also lexicographically sortable. Both sort correctly in `ORDER BY`, but they
are NOT interleave-compatible — a generated `"0000000001"` and an external
`"a0V"` won't produce a meaningful merged sort order.

**Recommendation:** When mixing generated and external ordering in the same
target, ensure a single source's ordering wins via priority. Cross-source
interleaving of different position-key formats is handled by automatic
Tier 2 promotion (see below).

### Automatic Tier 2 promotion for mixed ordering

When the engine detects that a target's order field is populated by different
mechanisms across mappings (`order: true` from one, `source:` from another),
it auto-promotes to Tier 2 behavior — even if the user didn't declare
`order_prev` / `order_next` fields.

How it works:

1. **Detection (at render time):** The engine scans all mappings for a given
   target. If the order field has both `order: true` and `source:` mappings,
   flag it as mixed-format.

2. **Internal prev/next columns:** For each forward view that uses
   `order: true` on a mixed-format target, the engine generates hidden
   `_prev` and `_next` columns using `LAG()` / `LEAD()` over the identity
   field(s) — identical to explicit Tier 2 generation. These columns are
   internal (prefixed with `_`) and not declared in the target schema.

3. **Forward views with external CRDT metadata:** These also generate
   `_prev` / `_next` using `LAG()` / `LEAD()` over the identity field(s),
   ordered by the external position key. If the source already provides
   explicit prev/next pointers, those are used instead (mapped via `source:`
   to the internal `_prev` / `_next` columns).

4. **Internal `_ordered_` view:** Generated automatically for mixed-format
   targets. Uses the same `WITH RECURSIVE` topological sort as explicit
   Tier 2. Reads `_prev` / `_next` from the resolved data, follows the
   linked list, and produces a canonical `_crdt_order` column.

5. **Stripping:** The internal `_prev`, `_next`, and mixed-format
   `step_order` columns are excluded from analytics and delta output. Only
   the reconstructed array order (from `jsonb_agg ORDER BY _crdt_order`)
   is visible.

This means the user's mapping stays simple:

```yaml
# Source A: external fractional indices
- name: editor_steps
  ...
  fields:
    - source: fractional_index
      target: step_order
    - source: instruction
      target: instruction

# Source B: plain array
- name: cms_steps
  ...
  fields:
    - target: step_order
      order: true
    - source: instruction
      target: instruction
```

The engine silently detects the mixed formats, generates internal prev/next
columns for both forward views, creates the `_ordered_` view, and produces a
correctly interleaved array — no Tier 2 declaration needed from the user.

**Cost:** Two extra columns per forward view + one recursive CTE. This is the
same overhead as explicit Tier 2, just triggered automatically. For targets
where all mappings use the same ordering mechanism (all `order: true` or all
`source:`), no promotion happens — Tier 1 behavior is preserved.

## Scalar arrays: a special note

For arrays of primitives (`["urgent", "customer-facing"]`), the element value
IS the content. This becomes identity:

```yaml
targets:
  tag:
    fields:
      parent_id:
        strategy: identity
      value:
        strategy: identity     # the tag string itself
      tag_order:
        strategy: coalesce
```

Duplicate values (same tag twice) are a genuine ambiguity. The engine should
warn when `order: true` is used with a target where all identity fields come
from scalar array values — these naturally lack uniqueness.

## Relationship to other plans

- **Supersedes POSITIONAL-ARRAY-PLAN** — Tier 1 covers the same use cases but
  with the critical fix of separating ordering from identity.
- **DEEP-NESTING-PLAN** (Done) — Multi-level LATERAL joins work unchanged;
  `order: true` applies to the leaf level.
- **TARGET-ARRAYS-PLAN** — For simple value lists (`text[]`), TARGET-ARRAYS
  eliminates the child target entirely. CRDT ordering applies when elements
  are complex objects that need a child target.
- **COMPOSITE-TYPES-PLAN** — Ordered arrays could use composite-type output
  instead of JSONB reconstruction.
