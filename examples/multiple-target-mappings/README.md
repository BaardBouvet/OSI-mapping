# Multiple Target Mappings

Demonstrates a single source mapping to multiple embedded target entities. A
customer record contains both billing and shipping address fields that each map
to a separate `address` target.

## How it works

1. `customer` target holds customer identity
2. Two embedded mappings (`billing_address`, `shipping_address`) both target the same `address` definition
3. `embedded: true` means address records have no independent identity — they belong to their parent customer
4. Different source field prefixes (`billing_*`, `shipping_*`) map to the same target field names
