# Computed target fields

**Status:** Design

Cross-target aggregation and recursive self-traversal on target fields,
computed late (post-resolution) so aggregated values flow through reverse
views to source systems. Includes the "missing bottom" example as the
motivating use case.

## Three capabilities in one plan

| Capability | YAML syntax | Pipeline layer | Flows to reverse? |
|------------|-------------|----------------|-------------------|
| **Child-target aggregation** | `from:` + `match:` on target field | `_enriched_` (new) | Yes |
| **Recursive self-traversal** | `traverse:` on target | Analytics | No |
| **Missing-bottom example** | Uses child-target aggregation | — | — |

These are tightly coupled: the missing-bottom example requires child-target
aggregation, and both aggregation patterns extend the same `TargetFieldDef`
model with computed values that don't come from source mappings.

---

# Part 1 — Child-target aggregation

## Problem

Today every target field resolves in isolation from a single target's forward
contributions. There is no way for a `line_item` field to aggregate values
from a related `shipment` target. The only workaround is raw SQL subqueries
in `expression:`, which the expression safety validator correctly rejects.

## Design constraints

1. **No raw SQL subqueries.** Expression safety prohibits `SELECT`/`FROM` in
   user-authored expressions. The engine must generate the subquery internally
   from declarative YAML.
2. **Late binding.** The aggregation must use resolved (post-identity-resolution)
   child entities, not raw source data. This guarantees correct results even
   when child data comes from multiple sources.
3. **Reverse flow.** Aggregated values must flow through reverse/delta views
   so source systems receive the summaries. Analytics-only computation
   doesn't satisfy this — those values never reach sources.
4. **No denormalization in forward views.** Forward views should not embed
   child-level aggregation subqueries. Forward extraction stays pure:
   normalize source columns, nothing more.
5. **DAG safety.** Cross-target references create view dependencies. Circular
   aggregation must be detected and rejected at compile time.

## Where in the pipeline?

| Layer | Pros | Cons |
|-------|------|------|
| Forward view | Source-local; simple SQL | Denormalizes; pre-resolution; wrong for multi-source children |
| Resolution view | Post-identity-linking | Violates per-target isolation; DAG changes |
| **Post-resolution wrapper** | Late-binding; resolved data; flows to reverse | New view layer |
| Analytics view | Clean separation | Doesn't flow to reverse views |
| Reverse view (inline) | No new layers | Duplicates subquery per mapping; can't be noop-compared |

**Recommendation: post-resolution wrapper.** Introduce a thin view
`_enriched_{target}` between resolution and reverse. It reads from
`_resolved_{target}` and adds cross-target aggregated columns via
`LEFT JOIN LATERAL` subqueries. Reverse views then read from
`_enriched_{target}` instead of `_resolved_{target}`.

Targets without cross-target fields skip the enriched layer — their reverse
views continue reading `_resolved_` directly.

```
_resolved_shipment ─────────────────────────┐
                                            ↓
_resolved_line_item → _enriched_line_item → _rev_b_items
                                          → _rev_a_items
```

### Why not the resolution view?

Resolution aggregates forward contributions per target using `GROUP BY
_entity_id_resolved`. Injecting a correlated subquery against another
target's resolution view would make the resolution DAG cross-target,
complicating the topological sort and risking cycles. Keeping resolution
per-target-only is a core design invariant.

### Why not the analytics view?

Analytics is the consumer-facing layer. Placing cross-target aggregation
there means values don't flow to reverse/delta views. For the missing-bottom
pattern, Warehouse B needs `total_shipped` in its items — that requires the
value to exist in the reverse pipeline.

The enriched layer gives us both: reverse views read it, and the analytics
view can also read from `_enriched_` instead of `_resolved_` to expose the
same values to consumers.

### Why not inline in reverse views?

Each reverse view would need its own copy of the aggregation subquery.
The subquery result would not appear in `_base` (the noop snapshot), so
delta detection couldn't compare it — every sync cycle would produce
spurious updates.

## Proposed YAML

Extend `TargetFieldDef` with `from:` and `match:` properties. When present,
the field becomes a cross-target aggregate computed in the enriched layer.

```yaml
targets:
  line_item:
    fields:
      item_name: { strategy: identity }
      order_id: { strategy: coalesce, references: order }
      qty: { strategy: coalesce, type: numeric }

      # Cross-target aggregation — computed from resolved shipments
      total_shipped:
        strategy: expression
        expression: "COALESCE(sum(qty), 0)"
        from: shipment
        match:
          item_name: item_name
          order_id: order_id
        type: numeric

      last_ship_date:
        strategy: expression
        expression: "max(ship_date)"
        from: shipment
        match:
          item_name: item_name
          order_id: order_id
```

### Property definitions

| Property | Type | Context | Description |
|----------|------|---------|-------------|
| `from` | string | `strategy: expression` | Target name to aggregate from |
| `match` | object | with `from` | Join conditions: `{remote_field: local_field}` pairs |

When `from:` is present:
- `strategy` must be `expression`
- `expression` is a SQL aggregate evaluated over matching rows from the
  remote target's resolved view
- `match` defines the equi-join between remote and local fields
- The field is implicitly `direction: forward_only` — it has no source
  column, so it cannot participate in reverse-to-source flow (but it does
  appear in the reverse view as a resolved value pushed to sources)

When `from:` is absent, `strategy: expression` works as today — aggregating
the target's own forward contributions in the resolution view.

## Generated SQL

### Enriched view

```sql
CREATE OR REPLACE VIEW "_enriched_line_item" AS
SELECT
  r.*,
  COALESCE(_agg_shipment.total_shipped, 0) AS "total_shipped",
  _agg_shipment.last_ship_date AS "last_ship_date"
FROM "_resolved_line_item" r
LEFT JOIN LATERAL (
  SELECT
    COALESCE(sum(s."qty"), 0) AS total_shipped,
    max(s."ship_date") AS last_ship_date
  FROM "_resolved_shipment" s
  WHERE s."item_name" = r."item_name"
    AND s."order_id" = r."order_id"
) _agg_shipment ON true;
```

When multiple fields share the same `from:` and `match:`, they are grouped
into a single lateral join. Each field's `expression:` becomes a column in
the subquery's SELECT list.

### Reverse / analytics view changes

```sql
-- Reverse and analytics read from enriched when available
FROM "_enriched_line_item" r   -- instead of "_resolved_line_item"
```

No other changes to reverse view generation. The enriched columns appear as
regular resolved fields.

## DAG impact

The enriched view adds one edge per `from:` reference:

```
_resolved_shipment → _enriched_line_item → _rev_b_items
                                         → _rev_a_items
                                         → line_item (analytics)
```

**No cycle risk.** `from:` always references `_resolved_{target}`, never
`_enriched_{target}`. Even circular `from:` references between two targets
are safe — each enriched view depends only on the other target's resolved
(pre-enrichment) view.

## Validation rules

1. `from:` value must name an existing target.
2. `match:` keys must be fields on the `from` target; values must be fields
   on the current target.
3. `from:` requires `strategy: expression` and a non-empty `expression:`.
4. `from:` fields must not also have `source:` — they are computed, not
   mapped from source data.
5. `from:` fields are implicitly `direction: forward_only`.
6. `expression:` on `from:` fields is validated as a SQL aggregate snippet
   (same rules as today's `TargetExpression` context).

## Noop detection for enriched fields

Fields computed via `from:` have no source value and thus no `_base` entry.
If `_base` has no entry, the `IS NOT DISTINCT FROM` check yields
`NULL IS NOT DISTINCT FROM new_value` — which is `false` when `new_value`
is non-null, correctly flagging the row as an update. On subsequent syncs,
`_base` will contain the previously-synced aggregate. This seems correct
without special handling.

---

# Part 2 — Recursive self-traversal

## Problem

An `employee` target with a `manager` self-reference can express the
relationship, but cannot derive values from it — e.g., a full hierarchy path
like `CEO / VP Sales / Regional Manager / Alice` or a `depth` field. These
require recursive graph traversal that no current mapping primitive supports.

## Where in the pipeline?

Hierarchy paths and depth values are **consumer-facing analytics**. They
don't need to flow back to source systems via reverse views — sources
already have their own hierarchy representation. Place this in the
**analytics view** layer.

## Proposed YAML

```yaml
targets:
  employee:
    fields:
      email: { strategy: identity }
      name: { strategy: coalesce }
      manager:
        strategy: coalesce
        references: employee       # self-reference

    traverse:
      hierarchy_path:
        follow: manager            # FK field to walk
        collect: name              # field to collect at each level
        separator: " / "
        direction: root_first      # CEO / VP / Manager / Self
        max_depth: 10

      depth:
        follow: manager
        aggregate: count           # count steps to root
        max_depth: 10
```

### `traverse:` properties

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `follow` | string | yes | FK field with self-reference (`references: same_target`) |
| `collect` | string | for string/array | Field to gather at each traversal step |
| `separator` | string | no | Join collected values (default: `" / "`) |
| `aggregate` | string | no | `string` (default), `array`, `count`, `sum` |
| `direction` | string | no | `root_first` (default) or `leaf_first` |
| `max_depth` | integer | no | Safety limit (default: 10) |

## Generated SQL

```sql
CREATE OR REPLACE VIEW "employee" AS
WITH RECURSIVE hierarchy AS (
  SELECT
    _entity_id, "name", "manager",
    "name"::text AS _path,
    1 AS _depth
  FROM "_resolved_employee"
  WHERE "manager" IS NULL

  UNION ALL

  SELECT
    e._entity_id, e."name", e."manager",
    h._path || ' / ' || e."name",
    h._depth + 1
  FROM "_resolved_employee" e
  JOIN hierarchy h ON e."manager" = h._entity_id
  WHERE h._depth < 10
    AND e._entity_id != ALL(h._visited)  -- cycle safety
)
SELECT
  r._entity_id AS _cluster_id,
  r."email", r."name", r."manager",
  COALESCE(h._path, r."name") AS "hierarchy_path",
  COALESCE(h._depth, 1) AS "depth"
FROM "_resolved_employee" r
LEFT JOIN hierarchy h ON h._entity_id = r._entity_id;
```

Multiple `traverse:` fields sharing the same `follow` field share one CTE.

## Validation rules

1. `follow` must name a field on the same target with `references: self`.
2. `collect` must name a field on the same target.
3. `max_depth` must be a positive integer.

---

# Part 3 — Missing-bottom example

The motivating example for child-target aggregation.

## Scenario

Warehouse A tracks Order → Item → Shipment (3 levels). Warehouse B tracks
Order → Item only (2 levels, no shipment concept). Item-level fields merge
bidirectionally, and child-target aggregation surfaces shipment summaries
on the parent line item.

```
Warehouse A (3 levels)              Warehouse B (2 levels)
┌────────────────────────┐          ┌────────────────────────┐
│ order_id: "ORD-1"      │          │ order_id: "ORD-1"      │
│ customer: "Acme Corp"  │          │ region: "EMEA"         │
│ items: [               │          │ items: [               │
│   { name: "Widget",    │          │   { name: "Widget",    │
│     qty: 10,           │          │     qty: 10,           │
│     unit_price: 25,    │          │     warehouse_loc:     │
│     shipments: [       │          │       "Bay 7" },       │
│       { qty: 5, ... }, │          │   { name: "Gadget",    │
│       { qty: 5, ... }  │          │     qty: 3,            │
│     ] },               │          │     warehouse_loc:     │
│   { name: "Gadget",    │          │       "Bay 12" }       │
│     qty: 3,            │          │ ]                      │
│     unit_price: 100,   │          └────────────────────────┘
│     shipments: [       │
│       { qty: 3, ... }  │
│     ] }                │
│ ]                      │
└────────────────────────┘
```

## Target model

```
order                  line_item              shipment
─────                  ─────────              ────────
order_id (id)          item_name (id)         item_name (id)
customer (coal)        order_id (id)          order_id (id)
region (coal)          qty (coal)             ship_date (id)
                       unit_price (coal)      qty (coal)
                       warehouse_loc (coal)   carrier (coal)
                       total_shipped (from)
                       last_ship_date (from)
                       fully_shipped (from)
```

## What child-target aggregation gives us

Warehouse B's resolved line_item includes `total_shipped`, `last_ship_date`,
and `fully_shipped` — values derived from Warehouse A's shipments after
identity resolution — without B knowing about shipments:

```
Resolved line_item for "Widget" on ORD-1:
  qty: 10              ← from both (match)
  unit_price: 25       ← from A only
  warehouse_loc: Bay 7 ← from B only
  total_shipped: 10    ← from: shipment aggregate
  last_ship_date: 2026-03-08  ← from: shipment aggregate
  fully_shipped: true  ← from: shipment aggregate
```

## Test cases

### Test 1: Shipment aggregates merge into B's line items

**Input:**
- Warehouse A: ORD-1 with Widget (2 shipments totaling 10) and Gadget (1 shipment of 3)
- Warehouse B: ORD-1 with Widget (Bay 7) and Gadget (Bay 12)

**Expected:**
- Widget: total\_shipped=10, fully\_shipped=true, last\_ship\_date=2026-03-08
- Gadget: total\_shipped=3, fully\_shipped=true, last\_ship\_date=2026-03-05
- Warehouse A updates: items get warehouse\_loc from B
- Warehouse B updates: items get unit\_price, total\_shipped, last\_ship\_date,
  fully\_shipped from A

### Test 2: Partial shipment — not fully shipped

**Input:**
- Warehouse A: ORD-2 with "Sprocket" qty=20, one shipment of 8
- Warehouse B: ORD-2 with "Sprocket" qty=20, warehouse\_loc="Bay 3"

**Expected:**
- Sprocket: total\_shipped=8, fully\_shipped=false

## Three depth-mismatch patterns compared

| Pattern | Example | Missing from | Engine feature |
|---------|---------|-------------|----------------|
| Extra ancestor | hierarchy-merge | Simpler lacks parent above | None |
| Missing middle | depth-mismatch | Simpler lacks intermediate | reverse\_filter |
| **Missing bottom** | **this** | **Simpler lacks leaf level** | **from: + match:** |

---

# Model changes

```rust
// TargetFieldDef additions
pub from: Option<String>,                          // remote target name
pub match_fields: Option<HashMap<String, String>>,  // remote→local join

// New struct for traverse
pub struct TraverseField {
    pub follow: String,
    pub collect: Option<String>,
    pub separator: Option<String>,
    pub aggregate: Option<TraverseAggregate>,  // string | array | count | sum
    pub direction: Option<TraverseDirection>,  // root_first | leaf_first
    pub max_depth: Option<u32>,
}
```

# Render changes

| Component | Change | Lines |
|-----------|--------|-------|
| New: `render_enriched()` | `_enriched_{target}` view with lateral joins | ~60 |
| `render_reverse()` | Read from `_enriched_` when target has `from:` fields | ~5 |
| `render_analytics()` | Read from `_enriched_` when available; add `WITH RECURSIVE` for `traverse:` | ~80 |

# Implementation phases

### Phase 1 — Child-target aggregation (~100 lines)

- Parse `from:` and `match:` on `TargetFieldDef`
- Validate references and field existence
- Render `_enriched_{target}` view with grouped lateral joins
- Switch reverse and analytics to read from enriched when present
- Test with missing-bottom example

### Phase 2 — Recursive traversal (~80 lines)

- Parse `traverse:` on targets
- Validate `follow` references self
- Render `WITH RECURSIVE` CTE in analytics view
- Test with employee hierarchy example

### Phase 3 — Missing-bottom example

- Create `examples/missing-bottom/` with mapping, tests, README

# Interaction with existing plans

- **EXPRESSION-SAFETY-PLAN:** `expression:` on `from:` fields is validated
  as a SQL aggregate snippet (same rules as `TargetExpression`). The engine
  generates the subquery; the user writes only the aggregate body.
- **EXPRESSION-SAFETY-PLAN Phase 3 (`lookup:`):** Superseded by `from:` — the
  `_enriched_` layer covers the same use cases more cleanly and integrates
  with the full pipeline.

# Open questions

1. **Should `fully_shipped` reference `qty` from the parent?** The expression
   `sum(qty) >= max(parent_qty)` assumes the child has a copy. An alternative:
   a `local:` keyword in expressions, e.g., `sum(qty) >= local.qty`.

2. **Should match conditions support non-equality?** Start with equi-join only.

3. **Should traversal values flow to reverse views?** Current design says no.
   If needed, a future enhancement could promote traverse to the enriched layer.
