# Cross-source element-level soft-delete via tombstone

**Status:** Done

When a child mapping declares `tombstone: { field: removed_at }`, elements
marked as soft-deleted are excluded from **all** sources' reconstructed
arrays — not just the source that set the marker. This is the element-level
analog of "deletion wins" semantics: once any source tombstones an element,
it disappears from the golden record everywhere.

The implementation reuses the existing `DeletionFilter` mechanism from
`derive_tombstones` (Option E in [ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md)),
feeding explicitly-tombstoned elements into the same `_del_{segment}` CTE
pipeline. No new view types or resolution changes are needed.

## Problem

### Per-source filtering is insufficient

The current implementation filters tombstoned elements only from the
tombstoning source's own nested CTE. The nested CTE reads from the
source's reverse view, which carries the tombstone field via passthrough
(`id._base->>'removed_at'`). Since passthrough is per-source, CRM B's
reverse view has `removed_at = NULL` even when CRM A has tombstoned the
same element.

**Current behaviour:**

```
CRM A source: [{tag: "vip"}, {tag: "churned", removed_at: "2026-01-15"}]
CRM B source: [{tag: "vip"}, {tag: "churned"}]

CRM A delta tags: [{tag: "vip"}]              ← "churned" filtered (correct)
CRM B delta tags: [{tag: "vip"}, {tag: "churned"}]  ← "churned" NOT filtered
```

CRM B still sees "churned" because the tombstone check uses CRM B's own
passthrough value (`NULL`), not the resolved tombstone status across sources.

### Why cross-source matters

1. **Sync loops**: CRM B's delta writes "churned" back → next cycle CRM A
   sees it again, even though CRM A explicitly removed it.
2. **Consistency**: The resolved entity is shared across sources. If an
   element is tombstoned in the golden record, all consumers should see
   the same state.
3. **User expectation**: "I deleted this tag in CRM A" should mean it
   disappears everywhere, not just in CRM A's view.

### Desired behaviour

```
CRM A source: [{tag: "vip"}, {tag: "churned", removed_at: "2026-01-15"}]
CRM B source: [{tag: "vip"}, {tag: "churned"}]

CRM A delta tags: [{tag: "vip"}]    ← "churned" filtered
CRM B delta tags: [{tag: "vip"}]    ← "churned" ALSO filtered (cross-source)
```

## Design constraints

1. **No target schema changes** — `removed_at` must not be a declared
   target field. The tombstone field stays in passthrough only.
2. **Existing infrastructure** — use `_base`, identity view, and
   `_cluster_id` (already available on reverse views) rather than
   modifying resolution or adding new view types.
3. **Composability** — must work alongside existing deletion mechanisms
   (`derive_tombstones`, `written_state`, element deletion CTEs).
4. **Performance** — avoid correlated subqueries per element. Pre-compute
   tombstone status in a CTE.

## Options

### Option A: EXISTS subquery in nested CTE (rejected)

Replace the per-source `WHERE NOT (n."removed_at" IS NOT NULL)` with a
correlated subquery against the identity view:

```sql
AND NOT EXISTS (
  SELECT 1 FROM _id_tag_entry AS _ts
  WHERE _ts._entity_id_resolved = n._cluster_id
  AND _ts._base->>'removed_at' IS NOT NULL
)
```

**Rejected** — correlated subquery executes per row. For arrays with many
elements, this causes N×M evaluation (N elements × M identity rows).

### Option B: Add tombstone to resolution view (rejected)

Add `removed_at` as an implicit resolved field (e.g. `BOOL_OR(...)`) in the
resolution view, then check the resolved value in the reverse view.

**Rejected** — requires changes to resolution view generation for an internal
concern. Mixes cross-cutting tombstone logic into the resolution pipeline.

### Option C: Pre-computed CTE via identity view (rejected)

Scan `_id_{child_target}` for rows where `_base->>'removed_at' IS NOT NULL`,
group by `_entity_id_resolved`, and LEFT JOIN to the nested CTE via
`n._cluster_id`.

**Rejected** — adds a new CTE type and requires the nested CTE to reference
`_cluster_id`, adding complexity. The existing `DeletionFilter` mechanism
already solves cross-source element exclusion for `derive_tombstones`.

### Option D: Reuse DeletionFilter mechanism (implemented)

Feed explicitly-tombstoned elements into the **same** `_del_{segment}` CTE
pipeline that `derive_tombstones` uses. The only difference is how the
deletion set is computed: `derive_tombstones` finds elements absent from
the current forward view vs. previously-written JSONB; explicit tombstones
find elements present but marked on the reverse view.

**Advantages:**

- Zero new concepts — reuses the existing `DeletionFilter` pipeline end-to-end
- Both mechanisms (derived absence + explicit marker) can coexist and combine
  via UNION ALL in the same `_del_{segment}` CTE
- No changes to `build_nested_ctes()` signature or logic
- No changes to forward, identity, resolution, or reverse views

## Design (Option D)

### How it works

The `derive_tombstones` mechanism in `render_delta_with_nested()` builds
deletion CTEs in three stages:

1. **`_del_prev_{segment}_{idx}`** — elements extracted from `_written` JSONB
2. **`_del_curr_{segment}_{idx}`** — elements from the current forward view
3. **`_del_src_{segment}_{idx}`** — set difference: prev minus curr (= deletions)

All `_del_src_` CTEs are UNION ALL'd into `_del_{segment}`, which
`build_nested_ctes()` LEFT JOINs to exclude matching elements.

For explicit tombstones, a new loop scans all child mappings that:
- Target the same child target and array segment
- Declare `tombstone:` with `resurrect: false`
- Use field-based detection (not custom `detect:`)

Each such mapping produces a single CTE:

```sql
_del_ts_tags_0 AS (
  SELECT n."parent_email"::text AS _parent_key,
         n."tag"::text AS "tag"
  FROM _rev_crm_a_tags AS n
  WHERE "removed_at" IS NOT NULL
)
```

This CTE reads from the **reverse view** (where the tombstone field is
available via passthrough) and produces the same `(_parent_key, identity_cols)`
shape as the `_del_src_` CTEs. It joins into the same `_del_{segment}`
UNION ALL, so `build_nested_ctes()` handles it identically.

### Data flow

```
Source JSONB element: {tag: "churned", removed_at: "2026-01-15"}
    │
    ▼
Forward view (_fwd_crm_a_tags):
    • removed_at detected via detection_expr_with_base("item.value")
    • Non-identity fields NULLed (tag_order → NULL)
    • _base includes raw value: {'removed_at': '2026-01-15', ...}
    │
    ▼
Identity view → Resolution → Reverse view (_rev_crm_a_tags):
    • Passthrough extracts: id._base->>'removed_at' AS "removed_at"
    │
    ▼
Deletion CTE (_del_ts_tags_0):
    • Scans _rev_crm_a_tags WHERE "removed_at" IS NOT NULL
    • Produces (_parent_key, identity_cols) for tombstoned elements
    │
    ▼
Combined deletion CTE (_del_tags):
    • UNION ALL of _del_ts_tags_0 (+ any _del_src_ from derive_tombstones)
    │
    ▼
Nested CTE (_nested_tags for CRM B):
    • LEFT JOIN _del_tags → excludes "churned" even though CRM B has it
```

### Noop detection

Noop detection already works correctly for cross-source tombstones:

```sql
COALESCE(_osi_text_norm(p._base->'tags')::text, '[]')
IS NOT DISTINCT FROM
COALESCE(_osi_text_norm(_nested_tags."tags")::text, '[]')
```

- `p._base->'tags'` = raw source JSONB array (what the source currently has)
- `_nested_tags."tags"` = reconstructed array (with tombstoned elements excluded)

When CRM A tombstones "churned", CRM B's raw source still has it but the
reconstructed array doesn't → mismatch → `'update'` action → ETL writes the
updated array to CRM B.

### Forward view tombstone detection

The forward view already NULLs non-identity fields for tombstoned elements
via `detection_expr_with_base()`. This ensures tombstoned elements can't
win field resolution — even though the element exists in the identity view,
its non-identity values are NULL, so the non-tombstoning source's values
win resolution.

This interplay is important: the forward-view NULLing handles **resolution**
(which source's value wins), while the delta CTE handles **reconstruction**
(whether the element appears in the array at all).

### `resurrect: true` behaviour

When `resurrect: true`, the child mapping is skipped during deletion CTE
generation — the tombstoned element remains in the array. If some mappings
have `resurrect: false` and others `true`, only the `false` mappings
contribute deletion CTEs.

### Custom `detect:` expressions

When the tombstone uses `detect:` (custom SQL expression) instead of the
standard `field:` + `undelete_value:`, the detection expression references
source columns that may not exist on other sources' reverse views. Custom
`detect:` on child mappings is skipped for cross-source deletion CTE
generation.

### Edge cases

| Scenario | Behaviour |
|----------|-----------|
| Only one source declares tombstone | CTE filters by that mapping only. Other sources' elements with the same identity are excluded when tombstoned. |
| Source has `removed_at` but no tombstone declared | Not scanned. The reverse view may still have the column via passthrough but it's not checked. |
| Tombstoned element with no matching identity in other sources | Element only exists in tombstoning source → filtered from that source's array. No effect on others. |
| Derives tombstones AND explicit tombstone | Both contribute to the same `_del_{segment}` UNION ALL. An element deleted by either mechanism is excluded. |
| `resurrect: true` | That mapping is skipped for deletion CTE generation. |

## Test case

Update `examples/element-soft-delete/mapping.yaml` to expect cross-source
propagation:

```yaml
tests:
  - description: >
      CRM A soft-deletes "churned" via removed_at.  The cross-source
      tombstone CTE detects this and excludes "churned" from BOTH
      sources' reconstructed arrays.
    input:
      crm_a:
        - id: "A1"
          email: "alice@example.com"
          name: "Alice"
          tags:
            - { tag: "vip" }
            - { tag: "churned", removed_at: "2026-01-15" }
      crm_b:
        - id: "B1"
          email: "alice@example.com"
          name: "Alice"
          tags:
            - { tag: "vip" }
            - { tag: "churned" }
    expected:
      crm_a:
        updates:
          - id: "A1"
            email: "alice@example.com"
            name: "Alice"
            tags:
              - { tag: "vip" }
      crm_b:
        updates:
          - id: "B1"
            email: "alice@example.com"
            name: "Alice"
            tags:
              - { tag: "vip" }
```

## Relationship to other plans

- [ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md) — deletion-wins via
  `written_state` (complementary: handles absence detection, this handles
  explicit tombstone markers)
- [SCALAR-ARRAY-DELETION-PLAN](SCALAR-ARRAY-DELETION-PLAN.md) — future
  extension for pure scalar arrays (would build on this tombstone CTE
  mechanism)
- [SOFT-DELETE-PLAN](SOFT-DELETE-PLAN.md) — entity-level soft-delete (this
  plan extends tombstone semantics to element level with cross-source scope)
