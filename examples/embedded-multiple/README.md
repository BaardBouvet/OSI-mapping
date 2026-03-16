# Embedded Multiple

Customer with billing and shipping addresses as embedded objects. CRM has inline fields, billing system has separate accounts.

## How it works

1. Billing and shipping addresses are separate target entities
2. CRM row maps to customer + both address targets using `parent:`
3. Billing system maps to customer + billing address only
4. Addresses use `last_modified` so the newer source wins
