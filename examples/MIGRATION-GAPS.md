# Example Migration Gaps

Comparison of `old-examples/` (35 examples, JSON format) vs `examples/` (36 examples, in-file YAML tests).

## Status

- **28 examples**: fully equivalent — all test scenarios carried forward
- **7 examples**: missing test cases (details below)
- **1 new example**: `hello-world` (5 tests, not in old-examples)
- **Schema features**: all preserved — no properties, strategies, or patterns lost
- **Renames**: `flattened-global` → `flattened`, `multiple-global-mappings` → `multiple-target-mappings`

## Missing Test Cases

### concurrent-detection (old: 2, new: 1)

Missing: **test2** — "OVERLAPS with test1 (same account)"
- Two sources share the same account identifier, both get inserts into the other
- Tests that `include_base` works correctly when both sides have matching account values

### relationship-embedded (old: 4, new: 2)

Missing: **erp_to_crm_insert** — "ERP company with embedded contact triggers inserts in CRM"
- ERP has a company with embedded contact fields, CRM starts empty
- Tests insert generation for person + association in CRM from embedded ERP data

Missing: **erp_inserts_association** — "ERP company with embedded contact creates association in CRM when company and contact already exist"
- Company and contact already exist in CRM separately
- ERP's embedded relationship creates the association row linking them

### relationship-mapping (old: 3, new: 1)

Missing: **inserts** — "CRM employee associations should appear in ERP insert view"
- Tests that many-to-many associations in CRM produce inserts in ERP's one-to-many structure

Missing: **merge_groups** — "ERP employee association should appear as insert in CRM"
- Tests merge groups across relationship mappings
- ERP has associations that don't exist in CRM yet

### embedded-vs-many-to-many (old: 3, new: 1)

Missing: **crm-provides-relationship** — "CRM provides contact relationship that doesn't exist in ERP"
- CRM has an embedded contact that ERP doesn't know about
- Tests unidirectional relationship propagation CRM → ERP

Missing: **erp-provides-relationship** — "ERP provides contact relationship that doesn't exist in CRM"
- Reverse direction: ERP has a many-to-many relationship not in CRM
- Tests unidirectional relationship propagation ERP → CRM

### route-combined (old: 3, new: 2)

Missing: **erp-inserts** — "Test inserts from ERP companies into empty contacts table"
- Dedicated ERP source has companies that should insert into the contacts table
- Tests insert generation from a dedicated (non-routing) source

### types (old: 3, new: 1)

Missing: **test2** — "Test selective merge: Bob only in HR, Carol only in CRM — no merging"
- Two people each existing in only one source
- Tests that non-overlapping records stay separate (no false merges)

Missing: **test3** — "Test selective merge: Multiple people with different merge patterns"
- Complex scenario with people appearing in various combinations of sources
- Tests type tracking across selective merge patterns
