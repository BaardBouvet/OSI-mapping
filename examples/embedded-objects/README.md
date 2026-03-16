# Embedded Objects

CRM has inline address fields within the customer row. ERP has separate companies and addresses tables with FK relationship.

## How it works

1. CRM: embedded address fields → uses `parent:` to map to `address` target
2. ERP: separate companies + addresses tables → addresses map directly, companies use FK
3. Companies merge on `name` (identity), addresses on `address_ref` (identity)
