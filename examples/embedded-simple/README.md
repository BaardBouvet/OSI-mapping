# Embedded Simple

Demonstrates the basic embedded pattern: one source has separate address fields inline, another has a separate address table, both map to the same target.

## How it works

1. CRM has customer + embedded billing/shipping address fields in one row
2. Billing system has separate account + address mapping
3. Both map to `customer`, `billing_address`, and `shipping_address` targets
4. `embedded: true` on CRM address mappings shares parent identity
