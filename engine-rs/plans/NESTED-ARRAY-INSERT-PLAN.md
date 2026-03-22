# Nested array reconstruction for insert rows

**Status:** Done

## Design requirement

Nested array reconstruction must support **arbitrary nesting depth** — the
engine must not impose a limit on the number of nesting levels. This is a
fundamental design constraint: any solution must work for 2-level, 3-level,
and N-level hierarchies alike.

## Problem

When the delta view produces an **insert** row for a source that has nested arrays
(e.g. `contact_center` with embedded `phones`), the nested array column is always
NULL or empty. This happens because:

1. Insert rows have `_src_id = NULL` (no existing source row to match).
2. The nested CTE groups by the `parent_fk_field` which resolves through
   a reference query returning `ref_local._src_id` — NULL for inserts.
3. The `WHERE n.{group_col} IS NOT NULL` filter excludes insert rows.
4. The LEFT JOIN in the final delta SELECT finds no matching `_parent_key`,
   producing NULL for the nested column.

## Root Cause

The nested CTE joins on `_src_id` (the source primary key of the parent row).
Insert rows don't have a source PK, but they DO have a `_cluster_id` (entity ID).
The child reverse view's reference query for `parent_fields` returns
`ref_local._src_id`, which is NULL when the parent has no source row.

## Solution

Two targeted changes — no UNION ALL or CTE restructuring needed:

### 1. Reverse view: COALESCE fallback on reference subquery (`reverse.rs`)

For `parent_fields` references, the existing reference subquery finds the
parent mapping's identity entry and returns its `_src_id`. For insert
entities (where the parent mapping has no source row), the subquery's
`JOIN ... AND ref_local._mapping = '...'` produces zero rows → NULL.

Fix: wrap the entire reference subquery in a COALESCE with a fallback
that returns the entity's cluster ID from **any** identity entry:

```sql
COALESCE(
  -- Primary: find the specific mapping's identity entry
  (SELECT ref_local._src_id
   FROM _id_contact ref_match
   JOIN _id_contact ref_local
     ON ref_local._entity_id_resolved = ref_match._entity_id_resolved
   WHERE (...) AND ref_local._mapping = 'cc_contacts'
   ORDER BY ... LIMIT 1),
  -- Fallback: entity cluster ID from any matching identity entry
  (SELECT ref_fb._entity_id_resolved
   FROM _id_contact ref_fb
   WHERE (...)
   LIMIT 1)
)
```

For existing parents, the primary subquery returns the PK (unchanged).
For insert parents, the fallback returns the entity cluster ID, making
`_parent_key` non-NULL and passing the `IS NOT NULL` filter.

### 2. Delta join: CASE on insert (`delta.rs`)

Change the root-level nested CTE join from:

```sql
LEFT JOIN _nested_lines ON _nested_lines._parent_key = p.order_id::text
```

to:

```sql
LEFT JOIN _nested_lines ON _nested_lines._parent_key =
  CASE WHEN p._src_id IS NULL THEN p."_cluster_id"
       ELSE p.order_id::text END
```

For existing rows, the join uses the PK column (unchanged behavior).
For insert rows, the join uses `_cluster_id`, which matches the
`_entity_id_resolved` fallback from change 1.

### Why this works at arbitrary depth

Only the **root-level** join needs the fix. Intermediate joins (within the
CTE tree) use identity fields from the resolved target view, which are
always populated for entities that exist. The `ref_is_nested` code path in
`reverse.rs` already returns identity field values for nested-to-nested
references, so intermediate levels work without modification.

## Files Modified

| File | Change |
|------|--------|
| `src/render/reverse.rs` | Parent-fields reference: outer COALESCE with fallback to `ref_fb._entity_id_resolved` |
| `src/render/delta.rs` | Root nested CTE join: `CASE WHEN p._src_id IS NULL ...`; NULL entry filter on leaf and interior CTEs |
| `examples/multi-value/mapping.yaml` | Test 3: contact_center insert now includes `phones: [{number: "555-4000"}]` |

## Phase 2 — NULL entry filtering

Cross-source contributions can inject all-NULL entries into nested arrays.
For example, when CRM maps a scalar `phone` (value NULL) into a nested
`phones` array, the reverse view produces a row with `number = NULL`
whose parent FK is valid. Without filtering, `jsonb_agg` produces
`{number: null}` in the array.

### Fix (delta.rs — `build_nested_ctes`)

Add a WHERE clause fragment to both leaf and interior CTEs:

- **Scalar arrays**: `AND n.{scalar_field} IS NOT NULL`
- **Object arrays**: `AND NOT (n.{f1} IS NULL AND n.{f2} IS NULL AND ...)`
  where f1, f2, … are all `item_fields` excluding order metadata targets.

The filter triggers only when *every* value field is NULL (rows with at
least one non-NULL value are preserved). If there are no value fields
(structural nodes with only child branches), the filter is omitted.

### Impact

| File | Change |
|------|--------|
| `src/render/delta.rs` | `null_filter` computed after `obj_expr`, appended to WHERE in leaf and interior CTEs |
| `examples/multi-value/mapping.yaml` | Test 2: removed `{number: null}` from expected phones; CC no longer shows an update (phones unchanged → noop) |
