# Composite Keys

Order lines identified by composite key (order_id + line_no). Orders from different systems merge via shared external reference.

## How it works

1. `link_group: "order_line_key"` on order_ref + line_number — records match only when BOTH fields match as a tuple
2. Purchase orders merge via `external_order_ref` (identity strategy)
3. Order lines from both systems are preserved independently
4. Composite source IDs in inserts show the tuple structure
