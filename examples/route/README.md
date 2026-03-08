# Conditional Routing

Routes records from one source to multiple target entities based on a filter condition.

## How it works

1. A single source mapping uses `filter` with a SQL WHERE condition to select rows
2. Person-type contacts route to `person`, company-type to `company`
3. `contact_type` is reverse-only — it reconstructs the discriminator when writing back
4. Each target entity gets its own field mappings
