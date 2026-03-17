# CRDT ordering for array elements

Deterministic ordering for nested array elements using `order: true`.

## Scenario

Two recipe databases (Recipe DB and Blog CMS) contribute steps for the same
recipes. Steps are identified by their instruction text (content-based identity).
The Blog CMS has higher priority for ordering.

Blog CMS inserts an extra step ("Grease the pan") that Recipe DB doesn't have.
After merge, the reconstructed array follows Blog CMS's ordering while recipe-
level fields (for example `cuisine`) can still come from Recipe DB by priority.

## Key features

- **`order: true`** — generates a sortable position key from `WITH ORDINALITY`
- **Content-based identity** — steps matched by `instruction`, not position
- **Position ≠ identity** — `step_order` uses `coalesce` (ordering metadata),
  not `identity`
- **Priority-driven ordering** — Blog CMS step ordering wins via per-mapping
  priority while recipe-level fields can resolve independently

## How it works

1. Forward views unpack each recipe's `steps` array with `WITH ORDINALITY`,
   generating `step_order` as `lpad((item.idx - 1)::text, 10, '0')`.
2. Identity resolution matches steps across sources by `instruction` text.
3. Resolution picks Blog CMS's ordering (higher priority) for `step_order`.
4. Delta reconstructs the array using `jsonb_agg(... ORDER BY step_order)`.

## When to use

Use `order: true` when:
- Array elements have natural content-based identity
- A single source's ordering should win (via priority)
- You need stable, deterministic array reconstruction
