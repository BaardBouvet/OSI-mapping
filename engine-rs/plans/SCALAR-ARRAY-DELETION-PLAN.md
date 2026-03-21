# Scalar array element deletion detection

**Status:** Done

Detect element-level deletions in **pure scalar value lists** —
arrays where elements are bare values (strings, numbers) without
lifecycle metadata.  When a source drops a scalar from its array,
the engine should detect the absence and propagate a deletion signal.

The approach is to model scalar arrays as child targets using
existing building blocks.  A `scalar: true` field mapping was added
to extract bare values from JSONB array elements without requiring
an expression.

## Problem (unchanged)

Today, soft-deleting individual elements in a value list requires
promoting scalars to objects with explicit metadata (the
[element-soft-delete](../../examples/element-soft-delete/) pattern):

```json
// Before: bare scalar list — no way to signal removal
["vip", "churned", "newsletter"]

// After: promoted to objects with lifecycle metadata
[{"tag": "vip"}, {"tag": "churned", "removed_at": "2026-01-15"}]
```

This works well when the source can be modified, but many real-world
sources store value lists as plain scalar arrays (`text[]`, JSONB
`["a","b","c"]`, CSV columns).  These sources express deletion by
simply removing the element — there's no tombstone record, no
`removed_at` timestamp.  The element just disappears.

## Solution: model scalar arrays as child targets

The key insight is that a scalar array element is just an entity whose
single value doubles as its identity.  Every building block needed for
deletion detection already exists — the only step is to model the
scalar array as a child mapping to a separate target.

### What already works

| Building block | Status | Role |
|---|---|---|
| Child mappings (`parent:` + `array:`) | Done | Maps array elements as a child target |
| `derive_tombstones` on child mappings | Done | Detects absent elements via parent's `written_state` |
| `written_state` on parent | Done | Stores previously-synced JSONB for absence comparison |
| `bool_or` strategy | Done | Resolves `is_removed` across sources (any-source-removed = removed) |
| `reverse_filter` on child mappings | Done | Per-consumer control over which elements to include |
| Cross-source element-level soft-delete | Done | Tombstoned elements filtered from all sources' arrays |

### YAML

```yaml
targets:
  contact:
    fields:
      email: identity
      name: coalesce

  # Scalar array → separate child target
  contact_tag:
    fields:
      tag: { strategy: identity, link_group: tag_identity }
      is_removed: { strategy: bool_or, type: boolean }

mappings:
  - name: crm_contacts
    source: crm
    target: contact
    written_state: true          # parent stores full object including arrays
    fields:
      - source: email
        target: email
      - source: name
        target: name

  - name: crm_tags
    parent: crm_contacts
    array: tags                  # source JSONB path: contact.tags[]
    target: contact_tag
    derive_tombstones: is_removed  # absent elements → is_removed = TRUE
    reverse_filter: "is_removed IS NOT TRUE"
    fields:
      - target: tag
        scalar: true             # extract bare value directly from array element
```

### How it works

1. Source has `tags: ["vip", "churned", "newsletter"]`
2. Forward view emits one row per tag with `tag` as identity
3. ETL syncs → parent's `_written` JSONB stores the full object
   including `tags` array
4. Source removes "churned": `tags: ["vip", "newsletter"]`
5. `derive_tombstones` compares current forward view against `_written`
   JSONB → "churned" is absent → synthesizes a row with
   `is_removed = TRUE`
6. Resolution combines via `bool_or` → `is_removed = TRUE` for churned
7. `reverse_filter` excludes churned from all consumers' arrays
8. Delta emits a delete for the removed element

### Why this is better than the original design

The original plan proposed new engine concepts (`element_deletion: soft`,
synthetic `_removed_at` injection, `collect` strategy with diff
awareness).  None of that is needed:

| Original proposal | Replaced by |
|---|---|
| `element_deletion: soft/hard/signal` | `derive_tombstones` + `reverse_filter` (existing) |
| Synthetic `_removed_at` timestamp | `is_removed` field with `bool_or` (existing) |
| `collect` strategy with built-in diff | Child mapping to separate target (existing) |
| New per-element action metadata | `_element_delta_{mapping}` view from written_state (existing) |

## Open questions resolved

1. **Synthetic timestamp vs boolean** → Resolved: use `is_removed` with
   `bool_or`.  If a timestamp is needed, use `last_modified` strategy
   instead.

2. **Re-appearance** → Resolved: `derive_tombstones` naturally handles
   this — element present in both current and previous = not absent =
   no `is_removed` synthesis.

3. **Cross-source semantics** → Resolved: `bool_or` means any-source-
   removed wins.  Use `last_modified` or `coalesce` for different
   semantics.

4. **Priority vs engine changes** → Resolved: only `scalar: true` was
   added to the engine — a convenience for extracting bare values from
   JSONB array elements.  The core deletion detection uses existing
   primitives.

## Relationship to other plans

- **[ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md)** (Done) — provides
  `_element_delta_{mapping}` and the written_state absence detection.
- **[ELEMENT-TOMBSTONES-AS-FIELD-PLAN](ELEMENT-TOMBSTONES-AS-FIELD-PLAN.md)**
  (Done) — unified `derive_tombstones` on child mappings is the building
  block that makes this work.
- **[ELEMENT-SOFT-DELETE-PLAN](ELEMENT-SOFT-DELETE-PLAN.md)** (Done) —
  cross-source tombstone filtering ensures removed elements disappear
  from all consumers.
