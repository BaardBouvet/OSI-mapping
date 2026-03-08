# Combined Routing and Dedicated Sources

Demonstrates a routing source (mapping to multiple targets via filters) merging
with dedicated sources (each mapping to a single target).

## How it works

1. `contacts` table routes to both `person` and `company` targets via `contact_type` filter
2. Dedicated sources: `crm_persons` → person, `erp_companies` → company
3. Records from routing and dedicated sources merge via shared `email` identity
4. `name` and `phone` use `last_modified` to select the most recent values
