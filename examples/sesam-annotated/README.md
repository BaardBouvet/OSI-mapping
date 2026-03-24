# Sesam DTL Annotated Example

The [Sesam DTL annotated example](https://docs.sesam.io/hub/dtl/annotated-example.html) reimagined as an OSI mapping. A `person` dataset joins with an `orders` dataset to produce `person_with_orders` — persons with their orders in a nested array.

## Scenario

The Sesam DTL example transforms persons by joining in their orders via `apply-hops`, uppercasing the name, adding a constant `type: "customer"`, and embedding the orders as a nested list. In OSI-mapping this becomes three datasets — `person`, `orders`, and `person_with_orders` — syncing bidirectionally through shared `global_person` and `global_order` targets.

## Key features

- **`reverse_expression: "upper(name)"`** — destructive transform applied only on the way out to `person_with_orders`, equivalent to DTL's `["upper", "_S.name"]`
- **`normalize: "upper(%s)"`** — prevents false updates when the uppercase output is compared back to the mixed-case source
- **`reverse_expression: "'customer'"`** — constant injection on a specific mapping, equivalent to DTL's `["add", "type", "customer"]`; only `person_with_orders` stamps the label on the way out
- **`references: person`** — cross-entity join, equivalent to DTL's `apply-hops` matching `cust_id` → person `_id`
- **`array: orders`** — nested array extraction/reconstruction, equivalent to the embedded orders list in DTL output
- **`last_modified`** — recency-based resolution so edits flow in both directions
- **`type: numeric`** — numeric order IDs matching Sesam's entity ID convention
- **`expression:` (enriched)** — raw SQL subquery computing `order_count` from the orders target, rendered as `LEFT JOIN LATERAL`
- **`sort: [{field: amount, direction: desc}]`** — sorts the nested orders array by amount descending in the delta output
- **`direction: reverse_only`** — `order_count` flows only from global target to `person_with_orders`, never written back
- **`reverse_filter: "amount > 100"`** — only orders exceeding 100 appear in the nested array, equivalent to DTL's `["filter", ["gt", "_.amount", 100]]`

## How it works

1. `person` and `orders` are the normalized inputs (DTL's source datasets)
2. `person_with_orders` is the denormalized join result (DTL's sink dataset)
3. All three map to shared `global_person` and `global_order` targets
4. The `person_with_orders_person` mapping uppercases names via `reverse_expression` — `person_with_orders` stores the uppercase form; `person` and the global target keep the original case
5. The nested `orders[]` array in `person_with_orders` maps to the `global_order` target via `parent:` + `array:`
6. Edits flow both ways — correcting a name in `person` updates `person_with_orders`, and editing an order amount in `person_with_orders` updates `orders`

## DTL-to-OSI concept mapping

| Sesam DTL | OSI-mapping equivalent |
|---|---|
| `["copy", "_id"]` | `person_id: identity` / `order_id: identity` |
| `["upper", "_S.name"]` | `reverse_expression: "upper(name)"` on the `person_with_orders_person` mapping + `normalize: "upper(%s)"` to prevent echo updates |
| `["add", "type", "customer"]` | `reverse_expression: "'customer'"` on the `person_with_orders_person` mapping |
| `apply-hops` join on `cust_id` | `references: person` on orders mapping |
| `"order"` sub-rule | `person_with_orders_orders` nested array mapping |
| `["count", "_T.orders"]` | `expression:` enriched expression with `SELECT count(*)` subquery |
| `["filter", ["gt", ...]]` | `reverse_filter: "amount > 100"` on the nested orders mapping |

## Feature coverage

All transforms from the Sesam DTL annotated example are now covered.
