# Element-level soft-delete for value lists

Cross-source soft-delete of individual array elements using `soft_delete` on child mappings.

## Scenario

Two CRM systems track contact tags. Instead of storing tags as bare scalars
(`["vip", "churned"]`), each tag is an object carrying its value and an
optional `removed_at` timestamp (`[{tag: "vip"}, {tag: "churned", removed_at: "2026-01-15"}]`).

When one CRM soft-deletes a tag by setting `removed_at`, the child mapping's
`soft_delete` declaration causes the engine to exclude that element from
**all** sources' reconstructed arrays — not just the source that set the
marker. This is the explicit-marker counterpart to
[`derive_element_tombstones`](../element-hard-delete/) (which detects deletions by
absence).

The two sources have different primary keys (`"A1"` vs `"B1"`) but share
an email address used for identity resolution.

## Key features

- **Soft-delete on child mapping** — `soft_delete: removed_at`
  tells the engine to treat array elements with non-null `removed_at` as soft-deleted
- **Cross-source propagation** — soft-deleted elements are excluded from all
  sources' deltas, not just the source that set the marker (deletion-wins semantics)
- **No `removed_at` in the target** — the soft-delete field is carried via passthrough
  and used only for filtering, keeping the target schema clean
- **Reuses `DeletionFilter` pipeline** — feeds into the same `_del_{segment}`
  CTE infrastructure as `derive_element_tombstones`, so both mechanisms compose

## How it works

1. Both CRMs expand their `tags` JSONB arrays into child `tag_entry` entities
   via nested child mappings (`parent:` + `array: tags`).
2. Identity resolution links tag entries by `(contact_ref, tag)` using
   `link_group`.
3. The engine scans **all** child mappings with `soft_delete` declarations for
   the same segment. For each, it generates a deletion CTE that selects
   soft-deleted elements from that mapping's reverse view.
4. All deletion CTEs are UNION ALL'd into a combined `_del_tags` CTE,
   which the nested CTE LEFT JOINs to exclude soft-deleted elements.
5. CRM A sets `removed_at` on "churned" → `_del_tags` contains "churned"
   → both CRM A and CRM B deltas exclude it → both get `'update'` with
   `[{tag: "vip"}]`.

## Comparison with derive_element_tombstones

| | `soft_delete:` (this pattern) | `derive_element_tombstones` |
|---|---|---|
| **Signal** | Explicit field (`removed_at`) | Absence from forward view vs. `_written` JSONB |
| **Requires** | Source sets a marker on the element | `written_state` table maintained by ETL |
| **Mechanism** | Scan reverse view for marked elements | Compare `_written` JSONB vs. current forward view |
| **Pipeline** | Same `_del_{segment}` CTE | Same `_del_{segment}` CTE |
| **Composable** | Yes — both can contribute to the same `_del_` UNION ALL | Yes |

## When to use

Use this pattern when:

- A source uses a **per-element soft-delete marker** (e.g. `removed_at`,
  `deleted_at`) on array elements
- Tombstoned elements should be **excluded from all sources' deltas**
  (deletion-wins)
- You want to keep the target schema **free of lifecycle metadata**

Use [`derive_element_tombstones`](../element-hard-delete/) instead when elements
simply disappear from the source array with no explicit marker.

### Modelling recommendation

When a source currently stores values as bare scalars (`["vip", "churned"]`),
promote them to objects (`[{tag: "vip"}, {tag: "churned"}]`) to enable
lifecycle metadata. This is a one-time schema change that unlocks per-element
soft-delete without requiring engine changes.
