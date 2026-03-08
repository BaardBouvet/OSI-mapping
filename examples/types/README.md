# Type Tracking

Tracks entity types across systems using a multi-valued field with custom expression strategy. People can be employees, customers, or both.

## How it works

1. Each mapping contributes a constant type value (e.g. `'employee'`)
2. `expression: "string_agg(distinct type, ',' order by type)"` creates a comma-separated list
3. `filter` on each mapping restricts reverse output — HR only gets employees, CRM only gets customers
