# Element-set resolution by timestamp

The most recently modified source's entire element set wins per parent via
`elements: last_modified`.

## Scenario

Two project management tools (Tool A and Tool B) contribute task lists to the
same project. With `elements: last_modified`, whichever source most recently
modified its task set provides the entire list for that project. Tasks from
the losing source are excluded entirely, not merged element-by-element.

## Key features

- **`elements: last_modified`** on the child target — switches from per-element
  merge to atomic element-set resolution by timestamp
- **Mapping-level `last_modified`** — provides the timestamp used to determine
  which source's element set wins

## How it works

1. Each child mapping declares `last_modified: parent_ts` which resolves to
   the parent row's `tasks_updated_at` column
2. The resolution view builds an `_element_winner` CTE that picks the winning
   mapping per parent using `MAX(_last_modified)`
3. A `WHERE EXISTS` clause filters the aggregation to only include elements
   from the winning mapping
4. Tool B's tasks are excluded entirely — not merged with Tool A's

## When to use

- One source is the authoritative real-time system and its child elements
  should wholesale replace stale data from other sources
- Merging elements per-field would create inconsistent sets (e.g., mixing
  tasks from two different project plans)
