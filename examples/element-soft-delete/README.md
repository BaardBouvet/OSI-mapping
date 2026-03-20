# Element-level soft-delete for value lists

Soft-delete individual elements in a value list by promoting scalars to objects with lifecycle metadata.

## Scenario

Two CRM systems track contact tags. Instead of storing tags as bare scalars
(`["vip", "churned"]`), each tag is an object carrying its value and an
optional `removed_at` timestamp (`[{tag: "vip"}, {tag: "churned", removed_at: "2026-01-15"}]`).

When one CRM soft-deletes a tag by setting `removed_at`, the timestamp
propagates to the other CRM through normal field resolution. Unlike
[`derive_tombstones`](../derive-tombstones/) (which silently excludes deleted
elements), this pattern **preserves** deleted elements in the array with
explicit metadata — consumers decide how to handle them.

The two sources have different primary keys (`"A1"` vs `"B1"`) but share
an email address used for identity resolution.

## Key features

- **Object array elements with `removed_at`** — each tag is `{tag, removed_at}` instead
  of a bare scalar, enabling per-element soft-delete without losing the element
- **`strategy: coalesce` on `removed_at`** — first non-null value wins regardless of
  source, making removal sticky once any source marks it
- **`link_group: tag_id`** — composite identity (`contact_ref` + `tag`) links the
  same tag across sources
- **`parent_fields: {parent_email: email}`** — child elements reference the parent
  via the email column; sources can have different PKs

## How it works

1. Both CRMs expand their `tags` JSONB arrays into child `tag_entry` entities
   via nested child mappings (`parent:` + `array: tags`).
2. Identity resolution links tag entries by `(contact_ref, tag)` using
   `link_group`.
3. The `removed_at` field resolves via `coalesce` — any non-null value wins.
   When CRM A sets `removed_at: "2026-01-15"` on a tag, the resolved value
   becomes non-null even though CRM B's contribution is null.
4. CRM B's delta reconstructs the `tags` array from the resolved child
   entities, now including CRM A's `removed_at` timestamp on the soft-deleted
   tag.
5. The ETL writes the updated array (with removal metadata) back to CRM B.

## When to use

Use this pattern when:

- Value lists need **per-element lifecycle tracking** (soft-delete, archival)
- Consumers should **see** which elements are removed rather than having them
  silently disappear
- Multiple sources contribute to the same list and removals must **propagate**

Use [`derive_tombstones`](../derive-tombstones/) instead when element removal
should be invisible — deleted elements are excluded from all reconstructed
arrays with no trace.

### Removal semantics

The choice of resolution strategy on `removed_at` controls reversibility:

| Strategy | Semantics |
|---|---|
| `coalesce` | Any non-null `removed_at` wins. Removal is **sticky** — once set by any source, it stays. |
| `last_modified` | Most recent writer wins. Removal is **reversible** — a source can clear `removed_at` later. |

### Modelling recommendation

When a source currently stores values as bare scalars (`["vip", "churned"]`),
promote them to objects (`[{tag: "vip"}, {tag: "churned"}]`) to enable
lifecycle metadata. This is a one-time schema change that unlocks per-element
soft-delete, archival timestamps, and other field-level metadata without
requiring engine changes.
