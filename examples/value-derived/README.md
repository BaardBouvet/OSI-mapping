# Value Derived

Combines grouped properties with default expressions to derive missing name
components.

## Scenario

Two sources for the same person:
- CRM: `full_name = "Alice Jones"` @ 2025-01-01
- HR: `first = "Alice"`, `last = "Smith"` (grouped) @ 2025-01-15

HR wins (newer), so:
- `first_name = "Alice"`, `last_name = "Smith"` from HR
- `full_name` derived via `default_expression`: `first_name || ' ' || last_name` → "Alice Smith"

CRM reverse gets "Alice Smith" (derived from the winning first+last names).
