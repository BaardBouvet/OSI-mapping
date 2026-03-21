# Nested Array Reconstruction for Insert Rows

## Problem

When the delta view produces an **insert** row for a source that has nested arrays
(e.g. `contact_center` with embedded `phones`), the nested array column is always
NULL or empty. This happens because:

1. Insert rows have `_src_id = NULL` (no existing source row to match).
2. The nested CTE (`_nested_phones`) groups by `_src_id` and filters
   `WHERE n._src_id IS NOT NULL`, so insert rows are excluded.
3. The LEFT JOIN in the final delta SELECT finds no matching `_parent_key`,
   producing NULL for the nested column.

### Concrete example (multi-value test 3)

Carol exists only in CRM. The engine correctly generates an insert for
`contact_center`, but the insert has no `phones` array — even though Carol's
phone ("555-4000") is available in the resolved `phone_entry` target.

### Secondary issue: NULL phone entries (multi-value test 2)

When CRM's `phone` is NULL, the `crm_phones` mapping still contributes a
`phone_entry` row with `phone = NULL`. The nested CTE for `contact_center`
aggregates ALL phone_entry rows (from both mappings), so this NULL entry
appears in the phones array as `{number: null}`.

## Root Cause

The nested CTE joins on `_src_id` (the source primary key of the parent row).
Insert rows don't have a source PK, but they DO have a `_cluster_id` (entity ID).
The child rows in the reverse view also have an entity-level key (via the
`contact_ref` → `contact` reference). The join could theoretically use the
entity/cluster key instead of the source PK for insert rows.

## Proposed Solution

### Phase 1: Fix nested arrays on insert rows

**Approach**: Dual-path nested CTE — one path for existing rows (join on
`_src_id`), one path for insert rows (join on `_cluster_id` / entity key).

1. **Modify `build_nested_ctes()`** to generate a UNION ALL CTE:
   ```sql
   _nested_phones AS (
     -- Existing rows: join on source PK (current logic)
     SELECT n._src_id AS _parent_key,
            COALESCE(jsonb_agg(...), '[]'::jsonb) AS phones
     FROM _reverse_cc_phones n
     WHERE n._src_id IS NOT NULL
     GROUP BY n._src_id

     UNION ALL

     -- Insert rows: join on entity cluster ID
     SELECT n._cluster_id AS _parent_key,
            COALESCE(jsonb_agg(...), '[]'::jsonb) AS phones
     FROM _reverse_cc_phones n
     WHERE n._src_id IS NULL AND n._cluster_id IS NOT NULL
     GROUP BY n._cluster_id
   )
   ```

2. **Modify delta final SELECT** to join on `_cluster_id` for insert rows:
   ```sql
   LEFT JOIN _nested_phones
     ON _nested_phones._parent_key = CASE
       WHEN p._action = 'insert' THEN p._cluster_id
       ELSE p.cid::text
     END
   ```

**Complexity**: Medium. The main challenge is that the child reverse view
currently joins on `_src_id` of the parent mapping. For insert rows, there
is no `_src_id`, so the child reverse view needs a secondary join path via
the entity ID (cluster_id). This requires:
- The child reverse view to expose `_cluster_id` alongside `_src_id`
- The nested CTE to handle both join keys

### Phase 2: Filter NULL entries from cross-source contributions

**Problem**: `crm_phones` contributes a NULL phone entry that appears in
`contact_center`'s phones array.

**Approach**: Add a `WHERE` filter in the nested CTE that excludes rows where
all non-key fields are NULL. This is already partially implied by the
`direction: forward_only` annotation on `crm_phones` fields, but the reverse
view still generates rows.

Alternatively, the `direction: forward_only` fields should suppress the
mapping's contribution to the child target's reverse view entirely, since
those fields are declared as not participating in reverse sync.

**Complexity**: Low-Medium. The `direction: forward_only` check already exists
in field filtering; extending it to suppress entire child target rows in the
reverse view when ALL fields are forward_only is straightforward.

## Files to Modify

| File | Change |
|------|--------|
| `src/render/delta.rs` | `build_nested_ctes()`: dual-path UNION ALL for insert rows |
| `src/render/delta.rs` | Final SELECT JOIN: conditional join key based on `_action` |
| `src/render/reverse.rs` | Expose `_cluster_id` in child reverse views |
| `src/render/reverse.rs` | Consider suppressing all-forward_only child mappings |

## Test Plan

- **multi-value test 3**: Carol's `contact_center` insert should include `phones: [{number: "555-4000"}]`
- **multi-value test 2**: Bob's `contact_center` update should NOT include `{number: null}` entry
- **nested-arrays test 1**: `warehouse_lines` inserts should include correct `order_number` reference
- Existing nested-array examples must continue to pass

## Open Questions

1. Should the dual-path approach use `_cluster_id` or the resolved entity key
   from the target's identity view? `_cluster_id` is available in the delta
   view directly, making it simpler.
2. For deeply nested arrays (3+ levels), the insert-path reconstruction may
   need to cascade through multiple entity references. Is this worth
   supporting in Phase 1 or should it be deferred?
