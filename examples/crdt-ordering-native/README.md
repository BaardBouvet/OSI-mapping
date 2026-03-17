# CRDT ordering with native sort keys

Demonstrates a mixed ordering setup: one source provides native CRDT order keys,
while the other source relies on generated ordinality.

## Scenario

`blog_cms` emits `sort_key` values per step (CRDT/fractional ordering style).
`recipe_db` is a plain ordered list without explicit order metadata.

Both map to the same `recipe_step.step_order` target field. Identity is matched
by `(recipe_name, instruction)` via `link_group`.

## Key features

- **Native CRDT ordering input**: `source: sort_key -> target: step_order`
- **Generated ordering input**: `order: true` for list-only source
- **Composite step identity**: `recipe_name + instruction` in one `link_group`
- **Cross-source merge behavior**: shared steps get values from both sides

## How it works

1. `recipe_db_steps` generates `step_order` from `WITH ORDINALITY`.
2. `blog_cms_steps` maps external `sort_key` into the same `step_order` field.
3. Resolution coalesces `step_order` and `duration` by priority.
4. Reverse/delta reconstruct arrays with `jsonb_agg(... ORDER BY step_order)`.

## Gotcha

`blog_cms.steps` output order is determined by `sort_key`, not by source array
position. Input arrays can be shuffled and still produce the same ordered
output.

When mixed ordering is used (`order: true` + external keys), reverse ETL must
apply updates in emitted array order. Generated-only rows do not have native
fractional keys from the source system; their placement is computed from
canonical ordering during reverse/delta.

## When to use

Use this pattern to document current engine behavior when mixing external CRDT
keys with generated ordinality in the same target field.
