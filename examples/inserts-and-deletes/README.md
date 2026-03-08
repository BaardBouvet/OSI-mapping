# Inserts and Deletes

Demonstrates insert/delete propagation using `reverse_required`. Only people with a customer_id are sent to crm_b.

## How it works

1. `reverse_required: true` on customer_id means: if resolved customer_id is null, exclude the row from crm_b output
2. People with customer_id in crm_a but not in crm_b → inserts in crm_b
3. People in crm_b without customer_id → deletes from crm_b
