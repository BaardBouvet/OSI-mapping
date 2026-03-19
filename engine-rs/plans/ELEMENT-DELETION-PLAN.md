# Element-level deletion for array targets

**Status:** Design

Array targets are grow-only today: the resolved view is the union of all
source contributions. When a source removes an element from its array, nothing
happens — the element either survives from another source, or silently
disappears because no source contributes it. Neither case produces an explicit
delete action in the delta.

This plan explores how to propagate element-level removals without breaking
the stateless SQL-view architecture.

## Problem

### Grow-only semantics

The CRDT ordering feature merges array elements by identity. Each source's
forward view emits rows for its current elements. Resolution unions them.
There is no concept of "this element was here before but is now absent."

Consider this scenario:

**T1 — both sources have "Sift flour":**

```
Recipe DB:  [Preheat, Mix, Sift flour, Bake]
Blog CMS:  [Preheat, Mix, Sift flour, Bake]
Resolved:  [Preheat, Mix, Sift flour, Bake]
```

**T2 — Blog CMS removes "Sift flour" (it was wrong):**

```
Recipe DB:  [Preheat, Mix, Sift flour, Bake]
Blog CMS:  [Preheat, Mix, Bake]
Resolved:  [Preheat, Mix, Sift flour, Bake]    ← still there from Recipe DB
```

The removal is invisible — Recipe DB still contributes the element, so the
resolved view keeps it. Even if *both* sources dropped "Sift flour", the
element would simply vanish from the union without generating a delta
`'delete'` action.

### Where it matters

- **Collaborative editing**: one user removes a step, others should see it gone.
- **Data quality**: a source system corrects a mistake by removing an element.
- **Compliance**: specific nested records must be purged across systems.
- **Sync loops**: a removed element keeps reappearing because the resolved
  view pushes it back to the source that deleted it.

## Design constraints

1. **Stateless views** — the engine generates pure SQL views with no mutable
   state. There is no "previous snapshot" to compare against.
2. **Composability** — element deletion should work with existing primitives
   (`bool_or`, `reverse_filter`, `order: true`) rather than inventing new
   engine concepts.
3. **Per-system control** — each target system should decide its own response
   to a deletion signal (same principle as propagated-delete).

## Options

### Option A: Explicit tombstone field (no engine changes)

The source keeps removed elements in its array with a marker:

```json
{
  "steps": [
    { "instruction": "Preheat", "removed": false },
    { "instruction": "Sift flour", "removed": true }
  ]
}
```

Mapping:

```yaml
targets:
  recipe_step:
    fields:
      instruction:
        strategy: identity
      is_removed:
        strategy: bool_or
      step_order:
        strategy: coalesce

mappings:
  - name: blog_cms_steps
    fields:
      - source: instruction
        target: instruction
      - source: removed
        target: is_removed
      - target: step_order
        order: true
```

Delta array reconstruction would filter:

```sql
jsonb_agg(...) FILTER (WHERE is_removed IS NOT TRUE)
```

And `reverse_filter` at element level would propagate the removal:

```sql
reverse_filter: "is_removed IS NOT TRUE"
```

This is the element-level analog of propagated-delete. It uses existing
primitives and requires zero engine changes.

#### Why `bool_or` and not `last_modified`?

`bool_or` means: "removed if ANY source says removed." This is the safe
default — once a single source flags an element as removed, it stays
removed in the golden record regardless of what other sources say.

`last_modified` would mean: "use the most recently updated value for
`is_removed`, whether that's `true` or `false`." This lets a source
"un-remove" an element by setting `removed = false` with a newer
timestamp. That's useful when removal is a reversible decision and
sources can reinstate elements.

The choice depends on the domain:

| Strategy | Semantics | When to use |
|----------|-----------|-------------|
| `bool_or` | Removed if any source says so. Sticky — can't be undone by another source | Compliance, data quality corrections, irreversible removals |
| `last_modified` | Most recent writer wins. Reversible — a source can set `removed = false` | Collaborative editing, feature toggles, undoable actions |
| `coalesce` | Highest-priority source wins | One system is authoritative for lifecycle |

All three work with Option A. The mapping author picks the strategy that
matches their removal semantics. The examples in this plan use `bool_or`
because "any source can remove, nobody can un-remove" is the safer
default for data integrity.

**Limitation:** The source must emit tombstone records rather than simply
dropping elements. Many source systems don't do this — they just remove
the element from the array.

### Option B: Complement array (no engine changes)

For sources that simply drop elements, model a separate "removals" mapping:

```yaml
mappings:
  - name: blog_cms_steps
    array: steps
    fields:
      - source: instruction
        target: instruction

  - name: blog_cms_removals
    array: removed_steps          # maintained by the source or ETL
    fields:
      - source: instruction
        target: instruction
      - expression: "true"
        target: is_removed
```

The source system (or an ETL pre-processor) maintains a `removed_steps`
array tracking which identities were intentionally removed. Effectively
externalizing the tombstone set.

**Limitation:** Requires the source to track removals somewhere, which is the
same ask as Option A but in a different shape.

### Option C: Snapshot diff in the engine (engine owns state)

The engine compares the current source snapshot against a materialized
previous snapshot, auto-generating tombstone rows for absent elements:

```sql
-- Elements present in previous snapshot but absent in current forward view
SELECT prev.instruction, 'true' AS is_removed
FROM _prev_blog_cms_steps prev
LEFT JOIN _fwd_blog_cms_steps curr USING (instruction, recipe_name)
WHERE curr.instruction IS NULL
```

This breaks the stateless-views model. It requires materialized tables,
a refresh cycle, and introduces temporal coupling between engine runs.

### Option D: Pure ETL diff (engine stays fully unaware)

The ETL maintains a tracking table and computes the diff entirely in its own
code. The engine emits current truth; the ETL compares against its state.

This works but forces every ETL implementation to re-implement the same
set-difference logic. The diff computation is generic and deterministic — it
doesn't depend on target-specific API knowledge — so it's better expressed as
SQL views than as bespoke ETL code.

### Option E: Engine reads ETL state table (recommended)

The engine reads a state table that the **ETL maintains**, and uses it to
compute element-level diffs in SQL. The engine doesn't write to the table —
it only reads. The ETL writes to it after each sync cycle.

This follows the exact `cluster_members` and `written_state` pattern:

| | `cluster_members` | `written_state` |
|---|---|---|
| **Purpose** | Insert feedback — prevent duplicate inserts | Entity tracking — noop + hard-delete + element deletion detection |
| **Who writes** | ETL (after inserting a new entity) | ETL (after syncing entity — writes the whole object including arrays) |
| **Who reads** | Engine (LEFT JOIN in forward view) | Engine (LEFT JOIN in delta view; extracts arrays for element delta) |
| **Engine writes?** | No | No |
| **Declared on** | Mapping (`cluster_members: true`) | Parent mapping (`written_state: true`) |
| **Table** | `_cluster_members_{mapping}` | `_written_{mapping}` |

The diff logic — elements present now but not before (insert), elements
present before but not now (delete), elements in both (update/noop) — is
pure SQL, deterministic, and testable. It belongs in the engine's view
pipeline, not in ETL application code.

## Design (Option E)

### Elements live inside the parent's written JSONB

No separate element-tracking table is needed. The parent mapping's
`written_state` table (`_written_{parent_mapping}`) already stores the
whole resolved object as JSONB — including its nested arrays. The engine
extracts previously-written elements from that JSONB and compares against
the current resolved elements to detect insertions, deletions, and updates.

This keeps the ETL maximally simple: after writing an entity to the target,
the ETL stores the entire object (fields + arrays) in one `_written` column.
One table, one concept, one write operation per entity.

```yaml
mappings:
  - name: blog_cms_recipes
    source: blog_cms
    target: recipe
    written_state: true             # ← stores the whole object including arrays
    written_noop: true
    fields:
      - source: id
        target: name
      - source: cuisine
        target: cuisine

  - name: blog_cms_steps
    parent: blog_cms_recipes
    array: steps
    parent_fields:
      parent_recipe: id
    target: recipe_step
    fields:
      - source: parent_recipe
        target: recipe_name
        references: blog_cms_recipes
      - target: step_order
        order: true
      - source: instruction
        target: instruction
      - source: duration
        target: duration
```

Note: `written_state` is declared on the **parent** mapping (recipes), not
on the child (steps). The child mapping doesn't need any written-state
configuration — the engine knows its elements are embedded in the parent's
JSONB because of the `parent:` / `array:` relationship.

### What the ETL writes

The ETL writes the full resolved object to `_written_blog_cms_recipes`:

```sql
CREATE TABLE _written_blog_cms_recipes (
    _cluster_id   text PRIMARY KEY,
    _written      jsonb NOT NULL,
    _written_at   timestamptz NOT NULL DEFAULT now()
);

INSERT INTO _written_blog_cms_recipes (_cluster_id, _written)
VALUES (
  'abc123',
  '{
    "name": "Chocolate Cake",
    "cuisine": "French",
    "steps": [
      {"instruction": "Preheat to 200°C", "step_order": "0000000000", "duration": "10"},
      {"instruction": "Mix dry ingredients", "step_order": "0000000001", "duration": "5"},
      {"instruction": "Bake 30 min", "step_order": "0000000003", "duration": "30"}
    ]
  }'
);
```

The `steps` array in the JSONB mirrors what the parent delta's `jsonb_agg`
produced. The ETL just stores what it wrote — no separate element tracking.

### Engine-generated element actions view

The engine generates `_element_delta_{child_mapping}` by extracting the
previously-written array from the parent's `_written` JSONB and comparing
it against the current reverse view:

```sql
CREATE OR REPLACE VIEW _element_delta_blog_cms_steps AS

WITH _prev AS (
    -- Extract previously written elements from parent's JSONB
    SELECT
        w._cluster_id AS _parent_id,
        elem->>'instruction' AS _element_id,
        elem
    FROM _written_blog_cms_recipes AS w,
         jsonb_array_elements(w._written->'steps') AS elem
)

-- Current elements: check if previously written
SELECT
    r."recipe_name"  AS _parent_id,
    r."instruction"  AS _element_id,
    r."step_order",
    r."duration",
    CASE
      WHEN p._element_id IS NULL THEN 'insert'
      ELSE 'present'
    END AS _element_action,
    r._base
FROM _rev_blog_cms_steps AS r
LEFT JOIN _prev AS p
  ON p._parent_id  = r."recipe_name"::text
 AND p._element_id = r."instruction"::text

UNION ALL

-- Removed elements: in written JSONB but not in current reverse view
SELECT
    p._parent_id,
    p._element_id AS "instruction",
    NULL AS "step_order",
    NULL AS "duration",
    'delete' AS _element_action,
    NULL AS _base
FROM _prev AS p
LEFT JOIN _rev_blog_cms_steps AS r
  ON r."recipe_name"::text = p._parent_id
 AND r."instruction"::text = p._element_id
WHERE r."instruction" IS NULL
```

The `'present'` rows could be refined to `'noop'` vs `'update'` by
comparing element field values against the `_prev.elem` JSONB — same
pattern as entity-level noop detection but using the written array element
instead of a separate state table row.

### How it fits in the view pipeline

```
_fwd_{mapping}  →  _id_{target}  →  _resolved_{target}  →  _rev_{mapping}
                                                                  │
                                          ┌───────────────────────┤
                                          ▼                       ▼
                                  _element_delta_{mapping}    _delta_{source}
                                  (per-element actions)       (parent row with
                                                               reconstructed
                                                               arrays)
```

The `_element_delta_{mapping}` view sits alongside the regular delta. It
doesn't replace it — the parent-level delta still produces rows with
reconstructed `jsonb_agg` arrays. The element delta is a supplementary view
for ETL processes that operate at the element level.

### Impact on parent delta array reconstruction

The parent delta's `jsonb_agg` continues to include all current elements
(the resolved truth). The parent's `_written` JSONB does NOT filter the
`jsonb_agg` — it only informs the element-level delta view.

If the mapping author wants the resolved array to exclude removed elements,
they use Option A (tombstone field + `bool_or`) in combination. The two
approaches compose:

- **Option A** handles _source-signaled_ removals (source emits tombstone)
- **Option E** handles _absence-detected_ removals (source drops element)

### What the ETL consumes

The ETL can query element-level actions instead of diffing arrays itself:

```sql
-- Elements to insert into target system
SELECT * FROM _element_delta_blog_cms_steps
WHERE _element_action = 'insert';

-- Elements to delete from target system
SELECT * FROM _element_delta_blog_cms_steps
WHERE _element_action = 'delete';
```

After processing, the ETL writes the whole object (including its arrays)
back to the parent's `_written_{mapping}` table. One INSERT/UPDATE per
entity — no separate element tracking.

### Noop detection for elements (optional refinement)

For elements that are `'present'` (existed before, still exist), the engine
can check whether field values changed by comparing against the matching
element from the parent's `_written` JSONB:

```sql
CASE
  WHEN p._element_id IS NULL THEN 'insert'
  WHEN p.elem->>'duration' IS NOT DISTINCT FROM r."duration"::text
  THEN 'noop'
  ELSE 'update'
END AS _element_action
```

Since the parent's `_written` JSONB contains the full element objects
(not just identities), this comparison is immediate — no separate state
table with per-element fields needed.

## Option comparison

| | A: Tombstone | B: Complement | C: Engine state | D: Pure ETL | E: Engine reads ETL state |
|---|---|---|---|---|---|
| Engine changes | None | None | Materialization | None | New view (no new table) |
| ETL logic | None | None | None | Diff computation | Store whole object (trivial) |
| Source changes | Must emit tombstones | Must track removals | None | None | None |
| Stateless views | ✓ | ✓ | ✗ | ✓ | ✓ (reads external table) |
| Handles silent drops | ✗ | ✗ | ✓ | ✓ | ✓ |
| Diff is SQL | N/A | N/A | ✓ | ✗ (ETL code) | ✓ |
| Composable | ✓ (bool_or) | ✓ | New mechanism | ✓ | ✓ (same as cluster_members) |
| Per-system control | reverse_filter | reverse_filter | Needs new concept | ETL policy | ETL policy + SQL views |
| Testable in engine | ✓ | ✓ | N/A | ✗ | ✓ |

## Recommendation

**Option E (engine reads ETL state table)** as the primary solution. The
engine computes the diff in SQL; the ETL only records what it synced. This
follows the `cluster_members` precedent: engine reads external state it
doesn't own.

**Option A (tombstone field)** remains complementary for sources that
naturally emit tombstone records.

The key insight: the diff computation is a pure function of (current resolved
elements × previously synced elements). It's generic, deterministic, and
testable — it belongs in the engine's SQL views, not in bespoke ETL code.
The temporal state (what was synced before) belongs in the ETL, which is
already stateful.

## What needs to happen

### Engine

1. **Model** — no new model property. The existing `written_state` on the
   parent mapping provides everything needed. The engine detects child
   mappings (via `parent:` / `array:`) and knows which JSONB key to extract
   elements from.

2. **Validation** — when a parent mapping has `written_state` and child
   mappings exist, the child's target must have at least one
   `strategy: identity` field (needed for element identity in the delta
   view). Warn if this is missing.

3. **Render** — generate `_element_delta_{child_mapping}` view when the
   child's parent has `written_state`. The view uses a CTE to extract
   elements from the parent's `_written` JSONB, then LEFT JOINs against
   the child's reverse view (and vice versa for deletions).

4. **Schema** — `written_state` is already in `mapping-schema.json`. No
   additional schema changes needed.

### Documentation

1. **New example**: `examples/element-deletion/` showing the tombstone
   pattern (Option A) and documenting how the parent's `written_state`
   enables element deletion detection for child array mappings.

2. **Schema reference**: document how `written_state` on a parent mapping
   with child array mappings generates `_element_delta_{child_mapping}`
   views.

3. **ETL guidance**: document that the ETL writes the whole object
   (including arrays) to `_written_{mapping}` — no separate element
   tracking needed.

### Relationship to other plans

- **[HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md)** — same
  pattern at the entity level. Also contains the generalised analysis of the
  one-way-door problem: deletion suppression (at both entity and element
  level) requires an override mechanism. The unified `_overrides_{mapping}`
  table handles both entity re-insertion and element un-removal.
- **[CRDT-ORDERING-PLAN](CRDT-ORDERING-PLAN.md)** — element ordering
  composes with element deletion. The `step_order` field flows through
  the element delta view, and deleted elements lose their ordering position.
- **[PROPAGATED-DELETE-PLAN](PROPAGATED-DELETE-PLAN.md)** — soft-delete
  propagation uses engine-native views (no state table). This plan handles
  hard deletes (no tombstone in source) via ETL state. Complementary.
- **[ETL-STATE-INPUT-PLAN](ETL-STATE-INPUT-PLAN.md)** — generalises the
  engine-reads-ETL-state pattern. The parent's `_written` JSONB provides both
  entity-level noop/hard-delete detection and element-level deletion detection
  (array presence in the JSONB). One table, one concept.

