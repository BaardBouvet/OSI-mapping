# Curated Merge with Linking Table

Explicit merge decisions using a linking/matching table as a data source.

## How it works

1. CRM and billing records map to `customer` target
2. A separate `merges` source provides explicit merge directives (which records should merge)
3. `crm_id` and `billing_id` use identity strategy — the merge source links them together
4. `name` uses coalesce with priorities so CRM wins
