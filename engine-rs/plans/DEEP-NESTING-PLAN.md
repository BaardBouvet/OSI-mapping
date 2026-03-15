# Deep Nesting (2+ Levels) — Delta Reconstruction Plan

## Current State

The forward pipeline handles arbitrary nesting depths correctly:
- `source.path: children` → `LATERAL jsonb_array_elements(src."children")`
- `source.path: children.grandchildren` → chained LATERAL joins, 2 levels deep
- `source.path: a.b.c` → 3 levels deep, etc.

**DOT graph**: Nested child reverse views now render as dotted JOIN lines to the
delta node (implemented), correctly showing that the parent mapping drives the
delta and nested children are LEFT JOINed in.

The **delta** pipeline only reconstructs 1 level of nesting. In `render_delta_view()`,
multi-segment paths (`children.grandchildren`) are filtered out:

```rust
// delta.rs ~line 120
if path.contains('.') {
    return None;
}
```

This means:
- `nested-arrays-deep` (parent → children → grandchildren) can't round-trip
- `nested-arrays-multiple` (org → departments → employees, org → projects → tasks)
  can't round-trip for the deeper levels

Both examples currently have no `expected:` in their tests because the delta
produces a false "update" — the reconstructed children array is missing the
grandchildren sub-arrays.

## The Problem

Given this mapping structure:
```yaml
source: parent (PK: id)
  └── children (path: children, parent_fields: {parent_id: id})
        └── grandchildren (path: children.grandchildren, parent_fields: {child_ref: child_id})
```

The delta currently produces:
```sql
_nested_0 AS (
  SELECT parent_id AS _parent_key,
         jsonb_agg(jsonb_build_object('child_id', child_id, 'value', value)) AS children
  FROM _rev_source_children
  GROUP BY parent_id
)
-- children array has {child_id, value} but NO grandchildren sub-array
```

What it needs to produce:
```sql
_nested_gc AS (
  SELECT child_ref AS _parent_key,
         jsonb_agg(jsonb_build_object('grandchild_id', grandchild_id, 'data', data)) AS grandchildren
  FROM _rev_source_grandchildren
  GROUP BY child_ref
)
, _nested_children AS (
  SELECT c.parent_id AS _parent_key,
         jsonb_agg(jsonb_build_object(
           'child_id', c.child_id,
           'value', c.value,
           'grandchildren', COALESCE(gc.grandchildren, '[]'::jsonb)
         )) AS children
  FROM _rev_source_children c
  LEFT JOIN _nested_gc gc ON gc._parent_key = c.child_id::text
  GROUP BY c.parent_id
)
-- Now children array has {child_id, value, grandchildren: [...]}
```

## Approach: Bottom-Up Tree Assembly

### Step 1: Build a nesting tree

Parse all nested-path mappings from the source and organize them into a tree
based on the `source.path` segments:

```
root (parent mapping)
├── children (path: "children")
│   └── grandchildren (path: "children.grandchildren")
├── projects (path: "projects")
│   └── tasks (path: "projects.tasks")
```

Each node knows:
- Its mapping (reverse view name, item fields, parent FK)
- Its children (deeper nested levels)
- Its segment name (the JSONB column it produces)

### Step 2: Generate CTEs bottom-up

Walk the tree from leaves to root. For each node:

1. **Leaf nodes** (no children): same as today — simple `jsonb_agg(jsonb_build_object(...))` grouped by parent FK.

2. **Interior nodes** (have children): `jsonb_agg(jsonb_build_object(..., child_col, COALESCE(child_cte.child_col, '[]'::jsonb)))` — the `jsonb_build_object` includes sub-array columns from child CTEs via LEFT JOIN.

The CTE naming follows the path: `_nested_children`, `_nested_children_grandchildren`, etc.

### Step 3: Join top-level CTEs to parent

Same as today — the top-level nested CTEs (direct children of root) are LEFT JOINed
to the parent reverse view. The only change is that the top-level CTEs now contain
fully nested sub-arrays.

### Step 4: Noop comparison

The noop comparison stays the same — `COALESCE(p._base->>'children', '[]') IS NOT DISTINCT FROM COALESCE(_nested_children.children::text, '[]')` — because `_base` stores the original input including all nesting levels, and the reconstructed CTE now includes all levels too.

## Data Structures

```rust
struct NestingNode<'a> {
    /// The segment name (e.g., "children", "grandchildren")
    segment: String,
    /// The full path (e.g., "children.grandchildren")
    full_path: String,
    /// The mapping for this nesting level
    mapping: &'a Mapping,
    /// Item fields to include in jsonb_build_object
    item_fields: Vec<String>,
    /// Parent FK field for GROUP BY
    parent_fk_field: Option<String>,
    /// Child nesting levels
    children: Vec<NestingNode<'a>>,
}
```

## Implementation Steps

1. **Build nesting tree** — In `render_delta_view`, after separating parent vs nested
   mappings, build a tree instead of a flat list. Insert each mapping at the correct
   depth based on its path segments.

2. **Recursive CTE generation** — Replace the flat CTE loop with a recursive function
   that processes the tree bottom-up:
   ```rust
   fn build_nested_cte(node: &NestingNode, ...) -> (String, String)
   // Returns (cte_sql, alias)
   ```

3. **Interior node CTE** — For nodes with children, the CTE:
   - Starts from the node's reverse view
   - LEFT JOINs each child's CTE on the child's parent FK = this node's item PK
   - Adds each child's array column to the `jsonb_build_object`
   - Groups by this node's parent FK

4. **Remove the `path.contains('.')` filter** — Multi-segment paths are now handled.

5. **Update tests** — Add `expected:` sections to `nested-arrays-deep` and
   `nested-arrays-multiple` now that they can round-trip.

## Edge Cases

- **3+ levels deep**: The recursive approach handles arbitrary depth naturally.
- **Multiple branches at the same level**: Already handled — siblings are independent
  CTEs. The parent's CTE LEFT JOINs all of them.
- **Mixed depths**: e.g., parent has both `items` (1 level) and `items.subitems`
  (2 levels). The tree correctly has `items` as an interior node with `subitems`
  as its child.
- **Grandchild FK column name**: The grandchild's `parent_fk_field` references a
  column in the child's reverse view (e.g., `child_ref`), which must match the
  child's PK-like identity column. This works because `parent_fields` in the
  mapping declares the relationship.

## SQL Output Example (nested-arrays-deep)

```sql
CREATE OR REPLACE VIEW "_delta_source" AS
WITH _nested_gc AS (
  SELECT "child_ref" AS _parent_key,
         COALESCE(jsonb_agg(jsonb_build_object(
           'grandchild_id', "grandchild_id",
           'data', "data"
         ) ORDER BY "grandchild_id"), '[]'::jsonb) AS "grandchildren"
  FROM "_rev_source_grandchildren"
  WHERE "child_ref" IS NOT NULL
  GROUP BY "child_ref"
),
_nested_children AS (
  SELECT c."parent_id" AS _parent_key,
         COALESCE(jsonb_agg(jsonb_build_object(
           'child_id', c."child_id",
           'value', c."value",
           'grandchildren', COALESCE(gc."grandchildren", '[]'::jsonb)
         ) ORDER BY c."child_id"), '[]'::jsonb) AS "children"
  FROM "_rev_source_children" c
  LEFT JOIN _nested_gc gc ON gc._parent_key = c."child_id"::text
  WHERE c."parent_id" IS NOT NULL
  GROUP BY c."parent_id"
)
SELECT
  CASE
    WHEN p._src_id IS NULL THEN 'insert'
    WHEN p._base->>'name' IS NOT DISTINCT FROM p."name"::text
     AND COALESCE(p._base->>'children', '[]') IS NOT DISTINCT FROM
         COALESCE(_nested_children."children"::text, '[]')
    THEN 'noop'
    ELSE 'update'
  END AS _action,
  p."_cluster_id",
  p."id",
  p."name",
  _nested_children."children",
  p."_base"
FROM "_rev_source_parents" AS p
LEFT JOIN _nested_children ON _nested_children._parent_key = p."id"::text;
```

## Ordering Concern

JSONB array comparison is order-sensitive. The noop check compares the original
`_base` JSON with the reconstructed array. If the reconstruction produces elements
in a different order than the original input, it will look like an "update" even
when nothing changed.

Solution: add `ORDER BY` to the `jsonb_agg()` calls using the identity/PK field
of each nested level. This ensures deterministic ordering. The forward direction
should also store arrays in consistent order (it does — LATERAL preserves source
array order, and the reverse/resolution views maintain insertion order).

## Estimated Scope

- ~80 lines of new code (tree building + recursive CTE generation)
- ~30 lines removed (flat loop replaced by recursive function)
- 2 test updates (add expected sections to deep nesting examples)
- No schema changes needed
