# Route Embedded

Multiple embedded objects without IDs from the same source routing to the same target.

## How it works

1. Customer records have billing and shipping address fields (no address IDs)
2. Both `billing_address` and `shipping_address` mappings target the same `address` definition
3. Both marked `embedded: true` — addresses rely on parent customer for identity
4. Reverse matches using composite source IDs (no foreign keys since addresses lack IDs)
