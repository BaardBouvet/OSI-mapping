# MISSING-BOTTOM-PLAN

Demonstrate merging when one system has a deeper leaf level that the other lacks:
Warehouse A tracks Order → Item → Shipment (3 levels), Warehouse B tracks
Order → Item (2 levels, no shipment concept). Shows the `sql:` aggregation
pattern to surface child-level summaries on the parent entity.

## Scenario

**Warehouse A** — full shipment tracking (3 levels):
```
┌──────────────────────────────────────────┐
│ warehouse_a                              │
│  id: "A1"                                │
│  order_id: "ORD-1"                       │
│  customer: "Acme Corp"                   │
│  items: [                                │
│    { name: "Widget", qty: 10,            │
│      unit_price: 25,                     │
│      shipments: [                        │
│        { qty: 5, date: "2026-03-01",     │
│          carrier: "FedEx" },             │
│        { qty: 5, date: "2026-03-08",     │
│          carrier: "UPS" }                │
│      ]                                   │
│    },                                    │
│    { name: "Gadget", qty: 3,             │
│      unit_price: 100,                    │
│      shipments: [                        │
│        { qty: 3, date: "2026-03-05",     │
│          carrier: "FedEx" }              │
│      ]                                   │
│    }                                     │
│  ]                                       │
└──────────────────────────────────────────┘
```

**Warehouse B** — no shipment tracking (2 levels):
```
┌──────────────────────────────────────┐
│ warehouse_b                          │
│  id: "B1"                            │
│  order_id: "ORD-1"                   │
│  region: "EMEA"                      │
│  items: [                            │
│    { name: "Widget", qty: 10,        │
│      warehouse_loc: "Bay 7" },       │
│    { name: "Gadget", qty: 3,         │
│      warehouse_loc: "Bay 12" }       │
│  ]                                   │
└──────────────────────────────────────┘
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
                       total_shipped (coal)   ← forward_only aggregates
                       last_ship_date (coal)  ← from sql: expressions
                       fully_shipped (coal)
```

Key decisions:
- `shipment` is a target entity sourced only from Warehouse A.
- `line_item` gets forward-only aggregated fields (`total_shipped`,
  `last_ship_date`, `fully_shipped`) computed via `sql:` on the A mapping.
- Warehouse B consumes these summaries via coalesce without knowing about
  shipments.

## Mappings

### Warehouse A (3 levels)

```yaml
- name: a_orders
  source: { dataset: warehouse_a }
  target: order
  fields:
    - source: order_id
      target: order_id
    - source: customer
      target: customer

- name: a_items
  source:
    dataset: warehouse_a
    path: items
    parent_fields:
      parent_order: order_id
  target: line_item
  fields:
    - source: parent_order
      target: order_id
      references: a_orders
    - source: name
      target: item_name
    - source: qty
      target: qty
    - source: unit_price
      target: unit_price
    # Aggregated shipment summaries — forward_only via sql:
    - target: total_shipped
      direction: forward_only
      type: numeric
      sql: >
        (SELECT COALESCE(SUM((s.value->>'qty')::int), 0)
         FROM jsonb_array_elements(item.value->'shipments') s)
    - target: last_ship_date
      direction: forward_only
      sql: >
        (SELECT MAX(s.value->>'date')
         FROM jsonb_array_elements(item.value->'shipments') s)
    - target: fully_shipped
      direction: forward_only
      sql: >
        CASE WHEN (SELECT COALESCE(SUM((s.value->>'qty')::int), 0)
                    FROM jsonb_array_elements(item.value->'shipments') s)
                  >= (item.value->>'qty')::int
             THEN 'true' ELSE 'false' END

- name: a_shipments
  source:
    dataset: warehouse_a
    path: items.shipments
    parent_fields:
      parent_item: name
      parent_order:
        path: items
        field: order_id      # qualified ref — reaches up to order level
  target: shipment
  fields:
    - source: parent_order
      target: order_id
      references: a_orders
    - source: parent_item
      target: item_name
      references: a_items
    - source: date
      target: ship_date
    - source: qty
      target: qty
    - source: carrier
      target: carrier
```

### Warehouse B (2 levels)

```yaml
- name: b_orders
  source: { dataset: warehouse_b }
  target: order
  fields:
    - source: order_id
      target: order_id
    - source: region
      target: region

- name: b_items
  source:
    dataset: warehouse_b
    path: items
    parent_fields:
      parent_order: order_id
  target: line_item
  fields:
    - source: parent_order
      target: order_id
      references: b_orders
    - source: name
      target: item_name
    - source: qty
      target: qty
    - source: warehouse_loc
      target: warehouse_loc
```

## What `sql:` aggregation gives us

Warehouse B's resolved line_item view includes `total_shipped`, `last_ship_date`,
and `fully_shipped` — values derived from Warehouse A's shipments — without B
needing any concept of shipments. The values coalesce naturally:

```
Resolved line_item for "Widget" on ORD-1:
  qty: 10              ← from both (match)
  unit_price: 25       ← from A only
  warehouse_loc: Bay 7 ← from B only
  total_shipped: 10    ← sql: aggregate from A's shipments
  last_ship_date: 2026-03-08  ← sql: aggregate from A
  fully_shipped: true  ← sql: computed from A
```

B's reverse view gets `total_shipped`, `last_ship_date`, `fully_shipped` pushed
into its items array — enriching it with shipment summaries it never had.

## Test cases

### Test 1: Shipment aggregates merge into B's line items

**Input:**
- Warehouse A: ORD-1 with Widget (2 shipments totaling 10) and Gadget (1 shipment of 3)
- Warehouse B: ORD-1 with Widget (Bay 7) and Gadget (Bay 12)

**Expected:**
- Order ORD-1: customer from A, region from B
- Line items merge via (item_name, order_id) identity
- Widget: total_shipped=10, fully_shipped=true, last_ship_date=2026-03-08
- Gadget: total_shipped=3, fully_shipped=true, last_ship_date=2026-03-05
- Shipment entities exist (from A only)
- Warehouse A: updates — items get warehouse_loc from B
- Warehouse B: updates — items get unit_price, total_shipped, last_ship_date,
  fully_shipped from A

### Test 2: Partial shipment — not fully shipped

**Input:**
- Warehouse A: ORD-2 with "Sprocket" qty=20, one shipment of 8
- Warehouse B: ORD-2 with "Sprocket" qty=20, warehouse_loc="Bay 3"

**Expected:**
- Sprocket: total_shipped=8, fully_shipped=false
- Both systems get updates with merged data

## Three patterns compared

| Pattern | Example | Missing from | Forward | Reverse | Engine feature |
|---------|---------|-------------|---------|---------|----------------|
| Extra ancestor | hierarchy-merge | Simpler lacks parent above | Natural | Natural | None |
| Missing middle | depth-mismatch | Simpler lacks intermediate | Natural | Reverse filters orphans | reverse_filter |
| **Missing bottom** | **this** | **Simpler lacks leaf level** | **sql: aggregates children** | **Summaries flow to simpler** | **sql: + forward_only** |

## Implementation

1. Create `examples/missing-bottom/` directory
2. Write `mapping.yaml` with sql: aggregate expressions
3. Test data with full and partial shipment scenarios
4. README explaining the aggregation pattern
5. No engine changes needed — uses existing `sql:` + `forward_only` + nested arrays

## Key insight

The missing-bottom pattern is the only depth mismatch that requires **computed
fields**. Extra ancestors and missing middles are solved purely by mapping design
(which fields you map, which you don't). Missing bottoms require you to
**summarize** child data that the simpler system can't represent, and `sql:`
expressions are the mechanism for that.
