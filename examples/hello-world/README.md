# Hello World

The absolute simplest mapping: two systems that each have a contacts table,
synced by email address.

## How it works

1. Both CRM and ERP have a contact with the same email
2. `email` is the identity field — it's how records are matched
3. `name` uses `coalesce` — the first non-null value wins (priority 1 beats 2)
4. After sync, both systems agree on the same name
