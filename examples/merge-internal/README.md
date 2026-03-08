# Internal Merge

Multiple rows from the same source merge together via shared identity values.

## How it works

1. Multiple rows in CRM share the same email address
2. Identity strategy on email connects them into one entity
3. `last_modified` on name picks the newer value
4. Both original rows get the resolved values on reverse
