# Element last modified

The most recently modified source's entire element set wins per parent
via `elements: last_modified`.

## Scenario

A shop and a warehouse both contribute order line items to a shared
`order_line` target.  The child target declares `elements: last_modified`
so the source whose `MAX(_last_modified)` is newest wins the complete set
of elements per parent order — elements from the stale source are
excluded, not merged.

## Key features

- **`elements: last_modified`** on the child target — switches from
  per-field resolution to atomic element-set resolution per parent.
- **`link_group`** on composite identity fields — ensures identity
  matching requires ALL grouped fields to match (AND), not just any (OR).
- **`last_modified: updated_at`** / **`last_modified: modified_at`** —
  mapping-level timestamp columns drive the element-set winner selection.

## How it works

1. The resolution view builds an `_element_winner` CTE that picks the
   winning mapping per parent entity using `DISTINCT ON (parent_ref)`
   ordered by `MAX(_last_modified) DESC`.
2. A `WHERE EXISTS` clause filters the main aggregation to only include
   rows from the winning mapping.
3. Elements that only exist in the losing source are completely excluded
   from the resolved view.
4. The reverse view uses a FULL JOIN so that excluded source rows appear
   with `_element_excluded = TRUE`, producing delete actions in the delta.

## When to use

Use `elements: last_modified` when no single source is always
authoritative but the most recently active source should own the
complete child element set.  Each source must provide a reliable
modification timestamp.
