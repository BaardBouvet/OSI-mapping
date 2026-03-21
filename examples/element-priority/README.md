# Element priority

The highest-priority source's entire element set wins per parent via
`elements: coalesce`.

## Scenario

A shop stores order line items as nested JSONB arrays while a warehouse
stores them as normalized rows.  Both contribute to the same
`order_line` target.  The warehouse mapping has `priority: 1` and the
shop has `priority: 2`.

With `elements: coalesce` on the target, the warehouse wins the entire
element set per parent order — not just individual fields.  Shop-only
line items are excluded; warehouse-only line items are included.

## Key features

- **`elements: coalesce`** on the child target — switches from
  per-field resolution to atomic element-set resolution per parent.
- **`link_group`** on composite identity fields — ensures identity
  matching requires ALL grouped fields to match (AND), not just any (OR).
- **`priority: 1` / `priority: 2`** — mapping-level priority determines
  which source's element set wins.

## How it works

1. The resolution view builds an `_element_winner` CTE that picks the
   winning mapping per parent entity using `DISTINCT ON (parent_ref)`
   ordered by `MIN(_priority) ASC`.
2. A `WHERE EXISTS` clause filters the main aggregation to only include
   rows from the winning mapping.
3. Elements that only exist in the losing source are completely excluded
   from the resolved view.
4. The reverse view uses a FULL JOIN so that excluded source rows appear
   with `_element_excluded = TRUE`, producing delete actions in the delta.

## When to use

Use `elements: coalesce` when one source is the authoritative system of
record and its complete set of child elements should replace all others.
