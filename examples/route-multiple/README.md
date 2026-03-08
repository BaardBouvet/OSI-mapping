# Route Multiple

Single source with multiple mappings to the same target using different filters
and priorities.

## How it works

1. CRM `contacts` table split into two mappings: primary and secondary
2. Both target the same `person` target
3. `filter` routes: `is_primary = 'true'` vs `is_primary = 'false'`
4. `reverse_filter` routes back: `is_primary_contact = true` vs `= false`
5. Primary mapping gets `priority: 1`, secondary gets `priority: 2` for coalesce
