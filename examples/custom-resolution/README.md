# Custom Resolution

Uses SQL aggregation expressions to resolve conflicts with custom business logic (max price, average rating, concatenated categories).

## How it works

1. Fields with `strategy: expression` define custom SQL aggregation
2. `string_agg`, `max`, `avg` aggregate values from all matching source rows
3. Duplicate products (same SKU) merge and aggregate across rows from the same source
