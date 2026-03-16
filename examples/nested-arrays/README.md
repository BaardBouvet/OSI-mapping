# Nested Arrays

Maps one-to-many relationships from both nested (shop with embedded line items) and normalized (warehouse with separate tables) structures to the same target entities.

## How it works

1. Shop system has orders with an embedded `lines` array — uses `parent:` + `array:` to extract items
2. `parent_fields` imports the parent order_id into nested item scope
3. Warehouse system has normalized tables — maps directly
4. Both map to the same `purchase_order` and `order_line` targets
