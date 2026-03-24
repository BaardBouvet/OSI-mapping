# Nested array sort predicate

**Status:** Done

Allow child mappings to declare a `sort:` predicate that controls the
ORDER BY clause when the reverse/delta view reconstructs the nested array
via `jsonb_agg`. Today the array order is determined by `order: true`
(CRDT position key) or, failing that, by the first identity field — there
is no way to sort by an arbitrary resolved field or expression.

## Motivation

The Sesam DTL annotated example includes `["sorted", "_.amount"]` and
`["sorted-descending", "_.amount"]` to sort embedded orders by amount.
OSI-mapping currently has no equivalent — nested arrays preserve source
row order or CRDT position, but cannot sort by a business field.

## Proposed YAML

Add an optional `sort:` property on child mappings (those with `parent:`
and `array:` or `array_path:`):

```yaml
mappings:
  - name: person_with_orders_orders
    parent: person_with_orders_person
    array: orders
    sort:
      - field: amount
        direction: desc
    target: global_order
    fields:
      - source: order_id
        target: order_id
      - source: amount
        target: amount
```

### Property definitions

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `sort` | list | no | — | Ordered list of sort keys for array reconstruction |
| `sort[].field` | string | yes | — | Target field name to sort by |
| `sort[].direction` | string | no | `asc` | `asc` or `desc` |

Multiple sort keys are applied in order (primary, secondary, etc.).

When `sort:` is absent, the existing behaviour applies: `order: true`
position key → first identity field → `_src_id`.

### Interaction with `order: true`

`sort:` and `order: true` are **mutually exclusive** on the same mapping.
`order: true` provides CRDT-safe positional ordering (source-authored);
`sort:` provides computed ordering from resolved field values. Declaring
both is a validation error.

## Where does sorting happen?

Sorting applies to the **reverse/delta** `jsonb_agg(...  ORDER BY ...)`
clause generated in `render/delta.rs`. This is the same location where
`order: true` position keys are injected today.

The sort field references a **resolved** target field name, which is
available in the reverse view because it reads from `_resolved_{target}`
(or `_enriched_{target}` when computed fields exist).

### Generated SQL

Before (current):
```sql
jsonb_agg(jsonb_build_object('order_id', n."order_id", 'amount', n."amount")
          ORDER BY n."order_id")
```

After (with `sort: [{field: amount, direction: desc}]`):
```sql
jsonb_agg(jsonb_build_object('order_id', n."order_id", 'amount', n."amount")
          ORDER BY n."amount" DESC NULLS LAST, n."order_id")
```

The existing identity-field fallback is appended after the sort keys to
guarantee a stable, deterministic order.

## Implementation

### Model changes

```rust
// New struct
pub struct SortKey {
    pub field: String,
    pub direction: Option<SortDirection>,  // default Asc
}

pub enum SortDirection {
    Asc,
    Desc,
}

// On Mapping
pub sort: Option<Vec<SortKey>>,
```

### Render changes

In `delta.rs`, the `order_expr_leaf` / `order_expr_nested` construction
(~line 1244) checks `mapping.sort` first. When present, the sort keys
replace the CRDT/identity ORDER BY expression:

```rust
let order_parts: Vec<String> = sort_keys
    .iter()
    .map(|sk| {
        let dir = match sk.direction {
            Some(SortDirection::Desc) => "DESC",
            _ => "ASC",
        };
        format!("n.{} {} NULLS LAST", qi(&sk.field), dir)
    })
    .collect();
// Append identity fallback for stability
order_parts.push(format!("n.{}", qi(fallback_field)));
```

### Validation rules

1. `sort:` only valid on child mappings (`parent:` is set).
2. `sort:` and `order: true` are mutually exclusive.
3. Each `sort[].field` must name a field mapped to the child's target.
4. `sort[].direction` must be `asc` or `desc` (case-insensitive).

### Schema changes

Add `sort` to the mapping object in `mapping-schema.json`.

### Estimated scope

~40 lines of model, ~20 lines of validation, ~15 lines of render, ~5
lines of schema = ~80 lines total.

## Open questions

1. **Should sort support expressions?** Starting with field names only
   keeps it simple. Expressions can be added later if needed.
2. **Should the analytics view also respect sort?** The analytics view
   uses the same reconstruction tree, so it gets sorting for free if
   the delta view has it. Confirm this is desirable.
