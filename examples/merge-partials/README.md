# Merge Partials

Partial entity contributions — one source provides only a flag (is_customer from invoices) while another provides full entity data.

## How it works

1. CRM contacts map to organization with full name/email
2. CRM invoices provide embedded `is_customer` flag via forward-only expression
3. `is_customer` uses expression strategy `bool_or` — true if any source says true
4. ERP customers use `reverse_filter` to only receive organizations where is_customer = true
