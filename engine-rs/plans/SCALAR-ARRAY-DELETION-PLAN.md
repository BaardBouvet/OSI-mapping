# Scalar array element deletion detection

**Status:** Proposed

Detect element-level deletions in **pure scalar value lists** —
arrays where elements are bare values (strings, numbers) without
lifecycle metadata. When a source drops a scalar from its array,
the engine should detect the absence and propagate a deletion signal.

## Problem

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
`["a","b","c"]`, CSV columns). These sources express deletion by
simply removing the element — there's no tombstone record, no
`removed_at` timestamp. The element just disappears.

### Current workarounds

| Approach | Mechanism | Limitation |
|----------|-----------|------------|
| Promote to objects | Source stores `{value, removed_at}` | Requires source schema change |
| `derive_tombstones` | `written_state` absence detection | Excludes deleted elements entirely — no metadata preserved |
| Manual ETL diff | ETL computes set difference externally | Duplicates generic logic in every ETL implementation |

### What's missing

A way to automatically detect "element X was in this source's previous
contribution but is now absent" for scalar arrays, without requiring
source schema changes, and with the option to either:

1. **Signal** the deletion (emit metadata like a synthetic `removed_at`)
2. **Exclude** the element (deletion-wins, already supported via
   `derive_tombstones`)
3. **Pass through** the deletion to an element-level delta view (Option E
   from [ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md))

## Design sketch

### Approach: extend `derive_tombstones` with per-element action metadata

The existing `derive_tombstones` mechanism already compares the current
forward view against the `_written` JSONB to detect absent elements.
Currently it **excludes** them from `jsonb_agg`. The extension would:

1. Instead of excluding absent elements, **retain** them in the
   reconstructed array with a synthetic deletion marker.
2. The marker would be a reserved property (`_deleted_at` or a
   user-configured field name) injected into the `jsonb_build_object`.
3. Downstream consumers see the element with deletion metadata rather
   than having it silently disappear.

### Possible YAML surface

```yaml
mappings:
  - name: crm_contacts
    source: crm
    target: contact
    written_state: true
    derive_tombstones: true    # existing: absence detection

  - name: crm_tags
    parent: crm_contacts
    array: tags
    target: tag_entry
    element_deletion: soft     # NEW: retain deleted elements with marker
    fields:
      - source: tag
        target: tag
```

Where `element_deletion`:
- `soft` — retain deleted elements, inject synthetic `removed_at` timestamp
- `hard` (default, current behaviour) — exclude deleted elements from array
- `signal` — emit to `_element_delta_{mapping}` view only (ETL decides)

### Alternative: `collect` strategy with built-in diff

For the [TARGET-ARRAYS-PLAN](TARGET-ARRAYS-PLAN.md) (array-typed target
fields without child entities), the `collect` strategy could gain
diff awareness:

```yaml
targets:
  contact:
    fields:
      tags:
        type: text[]
        strategy: collect
        element_deletion: soft
```

The engine would compare the current `array_agg(DISTINCT ...)` result
against the `_written` JSONB's stored array to detect removed elements.

## Relationship to other plans

- **[ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md)** (Done) — covers
  the general architecture of element deletion; this plan extends it
  specifically for scalar arrays without source schema changes.
- **[TARGET-ARRAYS-PLAN](TARGET-ARRAYS-PLAN.md)** (Maybe) — if array-typed
  target fields are implemented, this plan's `collect`-with-diff approach
  becomes relevant.
- **[ETL-STATE-INPUT-PLAN](ETL-STATE-INPUT-PLAN.md)** (Done, Phase 1) —
  provides the `_written` JSONB that enables absence detection.
- **[COMBINED-ETL-REVERSE-ETL-ANALYSIS](COMBINED-ETL-REVERSE-ETL-ANALYSIS.md)**
  (Design) — discusses which stateful features belong in the engine vs ETL.

## Open questions

1. **Synthetic timestamp vs boolean** — should the injected marker be a
   timestamp (`_removed_at: "2026-01-15T..."`) using the ETL's clock, or
   a boolean (`_removed: true`)? Timestamp is more useful for auditing
   but introduces clock dependency.

2. **Re-appearance** — if a source re-adds a previously deleted scalar,
   should the marker be cleared automatically? The `written_state`
   comparison would naturally handle this (element present in both
   current and previous = not deleted).

3. **Cross-source semantics** — when Source A removes a scalar that
   Source B still contributes, should deletion-wins apply (remove from
   all sources) or should Source B's contribution survive? The current
   `derive_tombstones` applies deletion-wins; soft mode would need to
   choose.

4. **Priority** — compared to promoting scalars to objects (which works
   today with zero engine changes), how often do users actually have
   immutable source schemas that prevent the promotion?
