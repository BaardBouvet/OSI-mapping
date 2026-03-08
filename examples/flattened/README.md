# Flattened Mapping

Maps different source structures (flat CRM, normalized ERP) to a single flattened target entity with embedded address properties.

## How it works

1. Target has a single `customer` entity with address fields inline (street, city, postal_code, country)
2. CRM has flat records — maps directly
3. ERP has separate companies + addresses tables — both map to the same customer target
4. The `name` field merges via identity; address fields combine from both ERP sources

## Important constraint

ERP addresses cannot be reused between companies. If two companies share the same `address_id`, they merge into one customer (because `address_ref` uses identity strategy). For shared addresses, use the [embedded-objects](../embedded-objects) pattern instead.
