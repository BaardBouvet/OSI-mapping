# Positional arrays

**Status:** Superseded

Superseded by [CRDT-ORDERING-PLAN](CRDT-ORDERING-PLAN.md), which separates
ordering from identity and adds CRDT-style merge semantics for multi-source
array element ordering.

---

*Original description:* Support nested arrays where elements have no natural
identity fields. Identity is derived from the parent's identity + the element's
array index position.

## Problem

Current nested array mappings require identity fields on the target entity that
uniquely identify each element. For example, `order_lines` needs `line_number`
or `item_name` as identity — the engine uses these to match elements across
systems and detect changes.

Many real-world arrays have elements with no natural key:
```yaml
# Tags — no key, order matters
tags: ["urgent", "customer-facing", "q2"]

# Address lines — positional
address_lines: ["123 Main Street", "Suite 400", "Building C"]

# Steps in a process — order IS the identity
steps: [
  { instruction: "Preheat oven to 200°C", duration: 10 },
  { instruction: "Mix ingredients",       duration: 5  },
  { instruction: "Bake for 30 minutes",   duration: 30 }
]
```

Today you'd need to invent an identity field (e.g. add `step_number: 1, 2, 3`)
or use the value itself as identity (fragile for non-unique values like tags
where "urgent" might appear twice in different contexts).

## Design

### New property: `position: true`

A field mapping property that auto-populates the field with the 0-based array
element index. Mutually exclusive with `source`, `source_path`, and `sql` — the
value comes from the array position, not from data.

```yaml
- name: source_steps
  source:
    dataset: recipes
    path: steps
    parent_fields:
      parent_recipe: name
  target: recipe_step
  fields:
    - source: parent_recipe
      target: recipe_name
      references: source_recipes
    - target: step_index
      position: true           # ← 0-based array index
    - source: instruction
      target: instruction
    - source: duration
      target: duration
```

The target entity uses `step_index` as a normal identity field:

```yaml
targets:
  recipe_step:
    fields:
      recipe_name:
        strategy: identity
      step_index:
        strategy: identity      # positional — auto-populated
      instruction:
        strategy: coalesce
      duration:
        strategy: coalesce
```

### SQL generation

`position: true` triggers `WITH ORDINALITY` on the LATERAL join:

```sql
CROSS JOIN LATERAL jsonb_array_elements("steps") WITH ORDINALITY AS item(value, idx)
```

The field expression becomes `(item.idx - 1)::text` (converting 1-based
ordinality to 0-based index).

### Why a dedicated property instead of a magic source name

A magic `source: _index` would collide with a real column called `_index`.
`position: true` is:
- **Unambiguous** — no collision with any source column name.
- **Explicit** — the user consciously opts into positional identity.
- **Validatable** — the engine can enforce mutual exclusivity with `source`,
  `source_path`, and `sql` at parse time.
- **Consistent** — similar to how `direction:` or `type:` annotate a field.

## Example: Recipe steps from two systems

**Recipe DB** — steps with instructions and duration:
```
┌────────────────────────────────────────┐
│ recipe_db                              │
│  id: "R1"                              │
│  name: "Chocolate Cake"               │
│  steps: [                              │
│    { instruction: "Preheat to 200°C",  │
│      duration: 10 },                   │
│    { instruction: "Mix dry ingredients",│
│      duration: 5 },                    │
│    { instruction: "Bake 30 min",       │
│      duration: 30 }                    │
│  ]                                     │
└────────────────────────────────────────┘
```

**Blog CMS** — recipes with steps as simple strings and ratings:
```
┌──────────────────────────────────┐
│ blog_cms                         │
│  id: "B1"                        │
│  recipe_name: "Chocolate Cake"   │
│  steps: [                        │
│    "Preheat to 200°C",           │
│    "Mix dry ingredients",        │
│    "Bake 30 min"                 │
│  ]                               │
│  rating: 4.5                     │
└──────────────────────────────────┘
```

### Targets

```yaml
targets:
  recipe:
    fields:
      name:
        strategy: identity
      rating:
        strategy: coalesce

  recipe_step:
    fields:
      recipe_name:
        strategy: identity
      step_index:
        strategy: identity
      instruction:
        strategy: coalesce
      duration:
        strategy: coalesce
```

### Mappings

```yaml
- name: db_recipes
  source: { dataset: recipe_db }
  target: recipe
  fields:
    - source: name
      target: name

- name: db_steps
  source:
    dataset: recipe_db
    path: steps
    parent_fields:
      parent_recipe: name
  target: recipe_step
  fields:
    - source: parent_recipe
      target: recipe_name
      references: db_recipes
    - target: step_index
      position: true
    - source: instruction
      target: instruction
    - source: duration
      target: duration

- name: blog_recipes
  source: { dataset: blog_cms }
  target: recipe
  fields:
    - source: recipe_name
      target: name
    - source: rating
      target: rating

- name: blog_steps
  source:
    dataset: blog_cms
    path: steps
    parent_fields:
      parent_recipe: recipe_name
  target: recipe_step
  fields:
    - source: parent_recipe
      target: recipe_name
      references: blog_recipes
    - target: step_index
      position: true
    - target: instruction
      sql: "item.value #>> '{}'"    # scalar array — value IS the string
```

### Test case

**Input:** Both systems have "Chocolate Cake" with same 3 steps.

**Expected:**
- Recipe "Chocolate Cake" resolves: rating=4.5 from blog
- Step (Chocolate Cake, 0): instruction from both, duration=10 from recipe_db
- Step (Chocolate Cake, 1): instruction from both, duration=5
- Step (Chocolate Cake, 2): instruction from both, duration=30
- recipe_db delta: noop (no new data flows back)
- blog_cms delta: noop (duration wouldn't flow back — blog has no duration)

This example also shows **scalar arrays** (blog steps are plain strings, not
objects) — `item.value #>> '{}'` extracts the scalar value from the JSONB element.

## When positional identity works and doesn't

### Works well (single source or synchronized ordering)

- **Ordered lists within one system** — steps, lines, instructions
- **Positional data** — CSV row numbers, spreadsheet rows
- **Round-tripping** — extract by index, resolve, write back to same positions
- **Multi-source ONLY when both systems guarantee the same ordering** — e.g.
  recipe steps that are always in the same order in both systems

### Dangerous (unsynchronized multi-source)

- **Independently managed lists** — System A has \[X, Y, Z\], System B has
  \[Y, X, Z\]. Position 0 is X in A but Y in B → wrong merge.
- **Different-length arrays** — A has 3 elements, B has 5. Positions 3-4 only
  exist in B. Positions 0-2 may not correspond.

### Validation warning

The engine should emit a **warning** (not error) when:
- `position: true` is used as identity
- The target has mappings from multiple sources
- The user should confirm that array ordering is synchronized

## Implementation

### Phase 1 — Model (`model.rs`)

Add `position: bool` (default false) to `FieldMapping`. Mutually exclusive
with `source`, `source_path`, and `sql`.

### Phase 2 — `WITH ORDINALITY` support (`forward.rs`)

Modify `jsonb_array_elements` generation:

Current:
```sql
CROSS JOIN LATERAL jsonb_array_elements("steps") AS item
```

With ordinality (when any field has `position: true`):
```sql
CROSS JOIN LATERAL jsonb_array_elements("steps") WITH ORDINALITY AS item(value, idx)
```

Detection: scan the mapping's fields for `position: true`. If found, enable
ordinality on the LATERAL join.

For multi-segment paths, ordinality is only needed on the last segment (the
one that produces `item`).

### Phase 3 — Position field resolution (`forward.rs`)

In `resolve_nested_source` (or a new check before it), when a field has
`position: true`:

```rust
if fm.position {
    return "(item.idx - 1)::text".to_string();  // 0-based
}
```

This makes the positional field behave like any other source field — it flows
through identity resolution, noop detection, and reverse normally.

### Phase 4 — Delta reconstruction (`delta.rs`)

When reconstructing the array via `jsonb_agg`, ORDER BY should use the
position-derived field to preserve original array order:

```sql
jsonb_agg(jsonb_build_object('instruction', ..., 'duration', ...)
          ORDER BY "step_index"::int)
```

The current code already orders by the first item field. If `step_index` is an
identity field it will be available. May need adjustment to prefer the
position-derived field for ordering.

### Phase 5 — Scalar array support (bonus)

Blog steps are a scalar array (`["a", "b", "c"]`), not objects. The engine
currently assumes `item.value` is a JSONB object and uses `item.value->>'key'`.

For scalar arrays, the entire `item.value` IS the value. Support this via
`sql: "item.value #>> '{}'"` (works today) or a dedicated keyword like
`value: true` on a field to extract the scalar directly.

### Phase 6 — Validation

- `position: true` is mutually exclusive with `source`, `source_path`, `sql`.
- `position: true` can only appear on a nested array mapping (one with
  `source.path` or, after PARENT-MAPPING-PLAN, `array`/`array_path`).
- Warn when `position: true` is used and the target has multiple source mappings.

### Phase 7 — Schema (`spec/mapping-schema.json`)

Add to field mapping properties:
```json
"position": {
  "type": "boolean",
  "default": false,
  "description": "Populate this field with the 0-based array element index. Only valid in nested array mappings. Mutually exclusive with source, source_path, and sql."
}
```

## Execution order

1. Model (add `position: bool` to FieldMapping)
2. `WITH ORDINALITY` in forward.rs (gated by `position: true`)
3. Position field resolution in forward.rs
4. Delta ORDER BY adjustment for positional fields
5. Validation rules
6. Schema update
7. Example: `positional-array/`
8. Test all examples still pass

## Risk

Low for single-source positional identity. The `WITH ORDINALITY` change is
additive — only emitted when `position: true` is used, so existing examples are
unaffected. Multi-source positional merge is warned but allowed (user's
responsibility to ensure ordering is consistent).
