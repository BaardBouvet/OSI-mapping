# Dot-path expressions

**Status:** Design

Cross-target field references using `target.field` dot-path syntax in
`strategy: expression` fields. The engine follows explicit `references:`
edges to build join conditions — no inference heuristics. Multi-hop paths
like `line_item.shipment.qty` are supported.

Builds on the enriched view layer from
[COMPUTED-FIELDS-PLAN](COMPUTED-FIELDS-PLAN.md). When the graph has no
unambiguous path, the user falls back to `from:` + `match:` from that plan.

---

## Motivating example: missing bottom

Warehouse A tracks Order → Line Item → Shipment (3 levels).
Warehouse B tracks Order → Line Item only (2 levels, no shipments).
Line item fields merge bidirectionally. Child-target aggregation surfaces
shipment summaries on the parent line item so Warehouse B receives
`total_shipped`, `last_ship_date`, and `fully_shipped` without knowing
about shipments.

```
Warehouse A (3 levels)              Warehouse B (2 levels)
┌────────────────────────┐          ┌────────────────────────┐
│ order_id: "ORD-1"      │          │ order_id: "ORD-1"      │
│ customer: "Acme Corp"  │          │ region: "EMEA"         │
│ items: [               │          │ items: [               │
│   { id: "LI-1",        │          │   { id: "LI-1",        │
│     name: "Widget",    │          │     name: "Widget",    │
│     qty: 10,           │          │     qty: 10,           │
│     unit_price: 25,    │          │     warehouse_loc:     │
│     shipments: [       │          │       "Bay 7" },       │
│       { qty: 5, ... }, │          │   { id: "LI-2",        │
│       { qty: 5, ... }  │          │     name: "Gadget",    │
│     ] },               │          │     qty: 3,            │
│   { id: "LI-2",        │          │     warehouse_loc:     │
│     name: "Gadget",    │          │       "Bay 12" }       │
│     qty: 3,            │          │ ]                      │
│     unit_price: 100,   │          └────────────────────────┘
│     shipments: [       │
│       { qty: 3, ... }  │
│     ] }                │
│ ]                      │
└────────────────────────┘
```

### Target model

```yaml
targets:
  order:
    fields:
      order_id: { strategy: identity }
      customer: { strategy: coalesce }
      region: { strategy: coalesce }

  line_item:
    fields:
      line_item_id: { strategy: identity }
      order: { strategy: coalesce, references: order }
      item_name: { strategy: coalesce }
      qty: { strategy: coalesce, type: numeric }
      unit_price: { strategy: coalesce, type: numeric }
      warehouse_loc: { strategy: coalesce }

      # Aggregate from shipment — join via shipment.line_item → line_item
      total_shipped:
        strategy: expression
        expression: "COALESCE(sum(shipment.qty), 0)"
        type: numeric
      last_ship_date:
        strategy: expression
        expression: "max(shipment.ship_date)"
      fully_shipped:
        strategy: expression
        expression: "COALESCE(sum(shipment.qty), 0) >= qty"

  shipment:
    fields:
      shipment_id: { strategy: identity }
      line_item: { strategy: coalesce, references: line_item }
      ship_date: { strategy: coalesce }
      qty: { strategy: coalesce, type: numeric }
      carrier: { strategy: coalesce }
```

Key relationships:
- `line_item.order` → `references: order` (line_item is child of order)
- `shipment.line_item` → `references: line_item` (shipment is child of
  line_item)

The dot-path `shipment.qty` in line_item's expression follows the reverse
edge: `shipment.line_item references: line_item`.

### What dot-path aggregation gives us

Warehouse B's resolved line_item includes aggregate values derived from
Warehouse A's shipments — without B knowing about shipments:

```
Resolved line_item "LI-1" (Widget):
  qty: 10              ← from both (match)
  unit_price: 25       ← from A only
  warehouse_loc: Bay 7 ← from B only
  total_shipped: 10    ← sum(shipment.qty) via enriched layer
  last_ship_date: ...  ← max(shipment.ship_date)
  fully_shipped: true  ← sum(shipment.qty) >= qty
```

### Test cases

#### Test 1: Shipment aggregates merge into B's line items

**Input:**
- Warehouse A: ORD-1 with Widget (2 shipments totaling 10) and Gadget
  (1 shipment of 3)
- Warehouse B: ORD-1 with Widget (Bay 7) and Gadget (Bay 12)

**Expected:**
- Widget: total\_shipped=10, fully\_shipped=true
- Gadget: total\_shipped=3, fully\_shipped=true
- Warehouse A updates: items get warehouse\_loc from B
- Warehouse B updates: items get unit\_price, total\_shipped,
  last\_ship\_date, fully\_shipped from A

#### Test 2: Partial shipment — not fully shipped

**Input:**
- Warehouse A: ORD-2 with "Sprocket" qty=20, one shipment of 8
- Warehouse B: ORD-2 with "Sprocket" qty=20, warehouse\_loc="Bay 3"

**Expected:**
- Sprocket: total\_shipped=8, fully\_shipped=false

---

## Design: references-only graph traversal

### Core rule

Each segment in a dot-path names a target. The engine traverses from the
local target to the named target by finding a `references:` edge between
adjacent hops. No same-name identity inference — only explicit
`references:` declarations create traversable edges.

### Edge direction

| Direction | Meaning | Cardinality | Example |
|-----------|---------|-------------|---------|
| **Forward** | Local has `field: references: remote` | Many-to-one (scalar) | `line_item.order` → order |
| **Reverse** | Remote has `field: references: local` | One-to-many (aggregate) | `shipment.line_item` → line_item |

Forward hops produce at most one row (parent lookup).
Reverse hops produce zero or more rows (child aggregation).

When the path contains at least one reverse hop, the expression must
contain an aggregate function (`sum`, `max`, `count`, etc.).

### Single-hop examples

**Reverse (aggregation):** `sum(shipment.qty)` on `line_item`

```
line_item ←── shipment.line_item references: line_item
```

**Forward (lookup):** `order.customer` on `line_item`

```
line_item ──→ line_item.order references: order
```

### Multi-hop

`sum(line_item.shipment.qty)` on `order`:

```
order ←── line_item.order references: order        (reverse)
          line_item ←── shipment.line_item references: line_item  (reverse)
                        shipment.qty
```

Two reverse hops — the engine chains the joins in a single lateral
subquery.

### Parsing dot-paths in expressions

The last segment of a dot-path is always the field name. Everything before
it is the target path.

| Expression | Path | Field | Direction |
|------------|------|-------|-----------|
| `sum(shipment.qty)` | `[shipment]` | `qty` | reverse |
| `max(shipment.ship_date)` | `[shipment]` | `ship_date` | reverse |
| `sum(line_item.shipment.qty)` | `[line_item, shipment]` | `qty` | reverse, reverse |
| `line_item.order.customer` | `[line_item, order]` | `customer` | forward, forward |

Unqualified names (e.g., `qty` in `sum(shipment.qty) >= qty`) refer to
the local target's fields.

### Ambiguity

If multiple `references:` edges connect two adjacent targets in the same
direction, the hop is ambiguous. Example: `employee` has both
`manager: references: employee` and `mentor: references: employee`.

**Rule: ambiguous hops are a compile-time error.** The error names the
conflicting edges. The user falls back to `from:` + `match:` from
[COMPUTED-FIELDS-PLAN](COMPUTED-FIELDS-PLAN.md).

---

## Enriched view layer

Dot-path expressions are computed in the `_enriched_{target}` view, a thin
wrapper between `_resolved_` and reverse views. See
[COMPUTED-FIELDS-PLAN § Where in the pipeline?](COMPUTED-FIELDS-PLAN.md)
for the full design rationale.

```
_resolved_shipment ───────────────────────────┐
                                              ↓
_resolved_line_item → _enriched_line_item → _rev_b_items
                                          → _rev_a_items
```

Targets without dot-path expressions skip the enriched layer.

---

## Generated SQL

### Single-hop aggregation (line_item → shipment)

```sql
CREATE OR REPLACE VIEW "_enriched_line_item" AS
SELECT
  r.*,
  COALESCE(_agg_shipment.total_shipped, 0) AS "total_shipped",
  _agg_shipment.last_ship_date AS "last_ship_date",
  _agg_shipment.fully_shipped AS "fully_shipped"
FROM "_resolved_line_item" r
LEFT JOIN LATERAL (
  SELECT
    COALESCE(sum(s."qty"), 0) AS total_shipped,
    max(s."ship_date") AS last_ship_date,
    COALESCE(sum(s."qty"), 0) >= r."qty" AS fully_shipped
  FROM "_resolved_shipment" s
  WHERE s."line_item" = r."_entity_id"
) _agg_shipment ON true;
```

The engine rewrites `sum(shipment.qty)` → `sum(s."qty")` and generates
the WHERE clause from the `references:` edge. The FK field on the remote
target (`shipment.line_item`) is matched against the local target's
entity ID.

Multiple fields referencing the same remote target are grouped into a
single lateral join.

### Multi-hop aggregation (order → line_item → shipment)

```sql
CREATE OR REPLACE VIEW "_enriched_order" AS
SELECT
  r.*,
  COALESCE(_agg.total_shipped, 0) AS "total_shipped"
FROM "_resolved_order" r
LEFT JOIN LATERAL (
  SELECT COALESCE(sum(s."qty"), 0) AS total_shipped
  FROM "_resolved_line_item" li
  JOIN "_resolved_shipment" s
    ON s."line_item" = li."_entity_id"
  WHERE li."order" = r."_entity_id"
) _agg ON true;
```

Each hop adds a JOIN in the subquery. The outermost WHERE anchors to the
local target's entity ID.

### Forward lookup (no aggregation)

When all hops are forward (parent-ward), the result is a scalar — no
aggregate function needed. The engine can use a plain LEFT JOIN:

```sql
-- shipment wants order.customer via shipment → line_item → order
LEFT JOIN "_resolved_line_item" li
  ON li."_entity_id" = r."line_item"
LEFT JOIN "_resolved_order" o
  ON o."_entity_id" = li."order"
```

And selects `o."customer"` directly.

---

## Prerequisite: no dots in target field names

Target field names must not contain `.` so that dot-paths are unambiguous.
Add `"propertyNames": { "pattern": "^[a-z][a-z0-9_]*$" }` to the JSON
Schema for target `fields`, and enforce the same regex in the Rust
validator.

## Validation rules

1. Each segment in the dot-path (before the final field) must name an
   existing target.
2. The final segment must name a field on the last target in the path.
3. For each adjacent pair in the path, exactly one `references:` edge must
   exist. If zero: error (no relationship). If multiple: error (ambiguous,
   use `from:` + `match:`).
4. All dot-path references in a single expression must traverse to the
   same final target (no multi-target aggregation in one field).
5. If any hop in the path is a reverse edge, the expression must contain
   an aggregate function.
6. Fields with dot-path references are implicitly `direction: reverse_only`.
7. Expression is validated as a SQL aggregate snippet — dot-paths are
   rewritten to qualified column aliases before safety validation.

## Direction semantics

Cross-target aggregated fields are reverse-only — computed post-resolution,
pushed to sources via the reverse pipeline. See
[COMPUTED-FIELDS-PLAN § Direction semantics](COMPUTED-FIELDS-PLAN.md) for
merge strategy discussion.

## Noop detection

See [COMPUTED-FIELDS-PLAN § Noop detection](COMPUTED-FIELDS-PLAN.md). No
special handling needed — `_base` tracks the previously-synced aggregate
value.

---

## Implementation

This plan adds ~30–50 lines on top of the enriched view infrastructure
from COMPUTED-FIELDS-PLAN:

- **Expression parser** (~20 lines): extract dot-paths, split into
  target path + field, validate each hop against `references:` edges.
- **Join generator** (~15 lines): for each hop, emit the appropriate
  JOIN clause (forward or reverse) based on edge direction.
- **Expression rewriter** (~10 lines): replace `target.field` with
  qualified aliases (`s."field"`) in the generated SQL.

Multi-hop support is incremental — the single-hop case is a special case
of the general chain.

## Interaction with COMPUTED-FIELDS-PLAN

| Concern | Where it lives |
|---------|----------------|
| Enriched view layer | COMPUTED-FIELDS-PLAN |
| `from:` + `match:` (explicit join) | COMPUTED-FIELDS-PLAN |
| `traverse:` (recursive CTE) | COMPUTED-FIELDS-PLAN |
| Dot-path syntax + graph traversal | **this plan** |
| Missing-bottom example | Both (different target models) |

Dot-paths are the recommended syntax for the common case. `from:` + `match:`
is the fallback when the `references:` graph is ambiguous or the desired
join doesn't match the graph.

## Three depth-mismatch patterns compared

| Pattern | Example | Missing from | Engine feature |
|---------|---------|-------------|----------------|
| Extra ancestor | hierarchy-merge | Simpler lacks parent above | None |
| Missing middle | depth-mismatch | Simpler lacks intermediate | reverse\_filter |
| **Missing bottom** | **this** | **Simpler lacks leaf level** | **dot-path expressions (or from: + match:)** |

## Open questions

1. **Composite identity targets.** When the referenced target has composite
   identity (e.g., identified by `order_id` + `item_name`), a single
   `references:` field can't express the FK. Until composite references
   are supported, these relationships need `from:` + `match:`. This plan
   uses surrogate keys to avoid the limitation.

2. **Mixed forward + reverse paths.** A path like `line_item.order.customer`
   from `shipment` combines forward hops (scalar lookup). A path like
   `line_item.shipment.qty` from `order` combines reverse hops
   (aggregation). Mixed paths (forward then reverse) are valid but need
   clear rules about when aggregation applies — the presence of any
   reverse hop requires an aggregate function.

3. **Filtering aggregated rows.** Same as COMPUTED-FIELDS-PLAN open
   question 4 — use a computed boolean + `reverse_filter:` for composable
   filtering.
