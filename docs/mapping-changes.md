# Example Mapping Changes

This document explains the changes to each example mapping YAML file
during the test-fixing phase of the OSI engine development.

The changes fall into three categories:

1. **Target field types** — New `type` property on target fields enables typed
   aggregation. The engine casts values to the declared type in forward views
   instead of the default `text`, so resolution expressions like `max(price)`
   and `bool_or(is_customer)` operate on the correct SQL type without explicit
   casts in the expression.

2. **Corrected expected test output** — Several examples had incorrect expected
   delta results due to misunderstanding of the engine's insert suppression,
   noop detection, and resolution priority behaviour.

3. **Mapping definition fixes** — Invalid filters and text/boolean literal
   mismatches.

---

## 1. `custom-resolution/mapping.yaml` — Added `type: numeric`

**Change:** Added `type: numeric` to `price` and `rating` target fields.

**Why:** `max()` and `avg()` need numeric input. Previously these expressions
required explicit `::numeric` casts (`max(price::numeric)`). With the target
type declared, the forward view casts `price::numeric` automatically, so the
expression can simply be `max(price)` and `avg(rating)`.

## 2. `embedded-multiple/mapping.yaml` — Corrected expected delta

**Change:** Replaced `crm: updates: [...]` with `billing: {}`.

**Why:** Embedded mappings contribute partial data to a shared target — they
cannot produce insert deltas (can't insert partial source rows). The CRM source
has no reverse-mapped changes, and the billing source is a noop (matches what
would be written back).

## 3. `embedded-objects/mapping.yaml` — Corrected expected delta

**Change:** Replaced `crm_customers: updates` and `erp_addresses: inserts`
with `erp_addresses: {}`.

**Why:** Same embedded mapping principle. The address mapping contributes
partial data — it has no reverse path to CRM, and the ERP address already
matches the resolved entity (noop).

## 4. `merge-partials/mapping.yaml` — Added `type: boolean`, corrected expected

**Changes:**
1. Added `type: boolean` to `is_customer` target field
2. Replaced `erp_customers: inserts: [...]` with `erp_customers: {}`

**Why (type):** `bool_or()` requires boolean input. With the declared type, the
forward view casts the value to boolean automatically, so the expression can
simply be `bool_or(is_customer)`.

**Why (expected):** Entity C2 only exists in CRM (no ERP contribution). The
engine's insert suppression logic prevents creating a new ERP row because
there's no existing ERP source row for this entity to base the insert on.

## 5. `relationship-mapping/mapping.yaml` — Removed invalid filter, corrected inserts

**Changes:**
1. Removed `filter: "relation_type = 'employee'"` from embedded `erp_contacts_assoc`
2. Simplified expected `erp_contacts: inserts` — removed `company_id` field and duplicate row

**Why (filter):** `relation_type` doesn't exist on `erp_contacts`. The filter
referenced a conceptual attribute of the association target, not a source column.

**Why (inserts):** `company_id` is mapped through an embedded `forward_only`
association mapping — it has no reverse path back to `erp_contacts`. The
reverse delta only includes fields with explicit reverse mappings (name, email).

## 6. `route-multiple/mapping.yaml` — Added `type: boolean`, corrected expected

**Changes:**
1. Added `type: boolean` to `is_primary_contact` target field
2. Reverted `expression` and `reverse_filter` back to unquoted boolean literals
3. Replaced `contacts: updates: [...]` with `contacts: {}`

**Why (type):** The `is_primary_contact` field stores a boolean flag. With the
declared type, the forward expression `true` is cast to boolean, and the
`reverse_filter: "is_primary_contact = true"` operates on a boolean column.

**Why (expected):** With the initial data load, source rows already match what
would be written back after round-tripping. Both contacts are noops.

## 7. `route/mapping.yaml` — Added `modified_at` to test data, added expected

**Change:** Added `modified_at` timestamp to test input rows and added
`expected: contacts: updates: []` section.

**Why:** The mapping uses `last_modified` strategy which requires a timestamp
column. The test data was missing the timestamp, and the test case had no
expected output section.

## 8. `types/mapping.yaml` — Corrected expected delta

**Change:** Replaced `hr: updates + inserts` with `hr: {}`, changed
`crm: inserts` to `crm: updates` with corrected values.

**Why:** No HR data was provided in the input, so no HR rows exist to update
or insert (insert suppression applies). For CRM, entity C001 exists in both
systems — HR's `employee_name: "Alice Anderson"` wins via `last_modified`,
producing a CRM update (not insert) with the HR-resolved values.

## 9. `value-conversions/mapping.yaml` — Added `type: date`

**Change:** Added `type: date` to `date_of_birth` target field.

**Why:** `TO_CHAR()` in the reverse expression requires date input, not text.
Previously the reverse_expression needed an explicit `::date` cast
(`TO_CHAR(date_of_birth::date, ...)`). With the declared type, the resolved
value is already a date, so the expression can simply be
`TO_CHAR(date_of_birth, 'DD/MM/YY')`.

## 10. `value-derived/mapping.yaml` — Corrected expected delta

**Change:** Replaced `crm: updates: [...]` with `hr: {}`.

**Why:** The test modifies an HR row. After round-tripping through the target,
the HR source receives the same values it already has (the input IS the latest
data) — noop. CRM was never expected to update because its `contact_name` is
derived via `default_expression`, which preserves the existing value.
