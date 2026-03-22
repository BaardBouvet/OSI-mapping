# Cluster seed format for nested-array disambiguation

**Status:** Design

The `_cluster_id` seed in test expectations must resolve to a single entity.
For flat mappings `"mapping:src_id"` is unambiguous because `_src_id` is unique
per mapping. For nested-array mappings, multiple child entities share the same
`_src_id` (the parent's primary key). This document compares two approaches for
disambiguating those children.

---

## Current approach: query parameters

```yaml
_cluster_id: "source_children:P1?child_id=2"
_cluster_id: "source_grandchildren:P1?child_ref=1&grandchild_id=1"
```

Filters are appended as `?field=value` pairs. The harness adds
`AND "field"::text = 'value'` clauses to the identity-view query.

### Pros

- **Flat and uniform** — the entire seed is a single string; no nesting or
  structured syntax to parse.
- **Directly binding** — filter fields are target field names that exist as
  columns in `_id_{target}`. There's no ambiguity about which SQL column is
  matched.
- **Minimal by default** — you only include the fields needed to disambiguate.
  If `grandchild_id` is globally unique, `?grandchild_id=11` suffices; you
  don't need to encode the full ancestor chain.
- **Easy to implement** — simple string split on `?` and `&`, no recursive
  parsing. Already implemented and shipping.
- **Composable** — works identically for any depth. A three-level nesting adds
  more `&` pairs, not a different syntax.

### Cons

- **Flat when the data is hierarchical** — the nesting hierarchy is implicit.
  The reader must know that `child_ref=1` identifies the parent, not a sibling.
- **Depends on target field names** — `child_ref` is a target field, not a
  source field. If the user reorganizes target definitions, seeds may need
  updating.
- **Ambiguity risk** — if the user forgets a filter, `LIMIT 1` picks an
  arbitrary row. The test passes but traces to the wrong entity. (Mitigated by
  the harness's full-row comparison — wrong entity → wrong field values →
  assertion failure.)
- **No structural validation** — the harness doesn't verify that the filters
  actually narrow to exactly one row. It only checks that *at least one* row
  matches.

---

## Alternative: path expression

```yaml
_cluster_id: "source_parents:P1/source_children[child_id=2]/source_grandchildren[grandchild_id=1]"
```

Each `/`-separated segment names a mapping and optional identity filter. The
harness would walk the identity views level by level.

### Pros

- **Mirrors the hierarchy** — the nesting structure is explicit in the syntax.
  A reader sees the parent → child → grandchild chain directly.
- **Self-documenting** — each segment names its mapping, making it clear which
  level contributes which filter.
- **Structural validation possible** — the harness could verify that each
  segment's mapping is actually a child of the previous segment's mapping,
  catching typos.

### Cons

- **Verbose** — deeply nested entities require long strings even when the leaf
  identity is globally unique. The path must always start at the root.
- **Complex to implement** — requires recursive resolution: resolve parent,
  then use parent's entity ID to scope the child query, etc. The identity view
  doesn't store parent entity IDs directly, so the implementation would need to
  join across identity views at each level.
- **Fragile to refactoring** — renaming an intermediate mapping or restructuring
  the nesting depth breaks every seed below that level.
- **Redundant information** — the path repeats the hierarchy that's already
  defined in the mapping YAML. The `_id_{target}` view already encodes the
  relationship; re-specifying it in the seed is duplication.
- **Inconsistent with flat seeds** — flat seeds are `"mapping:src_id"`. Path
  seeds would be a structurally different format, requiring branching in the
  parser and complicating documentation.

---

## Hybrid: path segments with implicit resolution

```yaml
_cluster_id: "source_grandchildren:P1[child_id=2, grandchild_id=1]"
```

Same as query params but with bracket syntax. Cosmetic difference only — no
meaningful advantage over `?field=value`.

---

## Comparison matrix

| Criterion                  | Query params        | Path expression      |
|----------------------------|---------------------|----------------------|
| Implementation complexity  | Low (done)          | High (multi-join)    |
| Syntax brevity             | Short               | Long for deep nests  |
| Hierarchy visibility       | Implicit            | Explicit             |
| Minimum-info seeds         | Yes (skip ancestors)| No (full path)       |
| Consistency with flat case | Same format + suffix| Different format     |
| Refactor resilience        | Only leaf fields    | All intermediate names|
| Validation capability      | Row-count check     | Structural + row     |

---

## Recommendation

**Keep query parameters.** The current format is already implemented, minimal,
and composes cleanly at any depth. The main theoretical advantage of path
expressions — explicit hierarchy — doesn't justify the implementation cost and
verbosity, especially since the harness's full-row comparison already catches
wrong-entity resolution.

One improvement worth adding: after resolving the seed, assert that the query
returned **exactly one row**. This catches under-specified filters (the
`LIMIT 1` picks arbitrarily) and over-specified filters (nothing matches).
This is a small harness change and closes the main safety gap.
