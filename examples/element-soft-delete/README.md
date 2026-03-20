# Element-level soft-delete for value lists

Soft-delete individual elements in a value list using tombstone detection on child mappings.

## Scenario

Two CRM systems track contact tags. Instead of storing tags as bare scalars
(`["vip", "churned"]`), each tag is an object carrying its value and an
optional `removed_at` timestamp (`[{tag: "vip"}, {tag: "churned", removed_at: "2026-01-15"}]`).

When one CRM soft-deletes a tag by setting `removed_at`, the child mapping's
`tombstone` declaration causes the engine to exclude that element from the
source's reconstructed array — without needing `removed_at` as a target field.

The two sources have different primary keys (`"A1"` vs `"B1"`) but share
an email address used for identity resolution.

## Key features

- **Tombstone on child mapping** — `tombstone: { field: removed_at, undelete_value: null }`
  tells the engine to treat array elements with non-null `removed_at` as soft-deleted
- **No `removed_at` in the target** — the tombstone field is carried via passthrough
  and used only for filtering, keeping the target schema clean
- **`link_group: tag_id`** — composite identity (`contact_ref` + `tag`) links the
  same tag across sources
- **`parent_fields: {parent_email: email}`** — child elements reference the parent
  via the email column; sources can have different PKs

## How it works

1. Both CRMs expand their `tags` JSONB arrays into child `tag_entry` entities
   via nested child mappings (`parent:` + `array: tags`).
2. Identity resolution links tag entries by `(contact_ref, tag)` using
   `link_group`.
3. The `tombstone` on each child mapping detects soft-deleted elements:
   elements where `removed_at IS NOT NULL` are excluded from that source's
   reconstructed array in the nested CTE.
4. CRM A sets `removed_at` on "churned" → CRM A's delta tags become
   `[{tag: "vip"}]` (churned excluded). CRM B has no `removed_at` on
   "churned" → CRM B's delta tags remain `[{tag: "vip"}, {tag: "churned"}]`
   (no change).

## Tombstone semantics

The tombstone on child mappings follows the same per-source semantics as
entity-level tombstones:

- **Same source**: elements marked as tombstoned are excluded from that
  source's reconstructed array
- **Other sources**: unaffected — if they have the same element without
  the tombstone marker, it remains in their array

This is consistent with entity-level `tombstone` behaviour: the soft-delete
suppresses the tombstoning source's contribution without propagating a global
deletion.

## When to use

Use this pattern when:

- A source uses a **per-element soft-delete marker** (e.g. `removed_at`,
  `deleted_at`) on array elements
- The soft-deleted element should be **excluded from the source's own delta**
- You want to keep the target schema **free of lifecycle metadata**

Use [`derive_tombstones`](../derive-tombstones/) instead when element removal
should be detected automatically from the written state (elements that
disappear across syncs).

### Modelling recommendation

When a source currently stores values as bare scalars (`["vip", "churned"]`),
promote them to objects (`[{tag: "vip"}, {tag: "churned"}]`) to enable
lifecycle metadata. This is a one-time schema change that unlocks per-element
soft-delete without requiring engine changes.
