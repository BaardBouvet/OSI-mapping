# Insert PK visibility

**Status:** Done

Insert rows in the delta view carry all columns including PKs. The test
harness was stripping PK columns from insert verification because "inserts
are new rows so PKs are always null". This hid meaningful values for
natural/business keys and was unnecessarily lossy.

## Problem

When `crm_associations` has composite PK `[company_id, contact_id,
relation_type]`, the delta view resolves all three columns with valid
business values (`"C1"`, resolved contact ID, `"employee"`). The test
harness stripped them all, leaving the expected insert as just
`{ _cluster_id: "..." }` — visually empty and misleading.

The root assumption "insert PKs are always null" is only true for
**surrogate/auto-generated** keys. For **natural/business** keys the PK
columns carry meaningful resolved data.

## Solution

Stop stripping PK columns from insert verification entirely. Expected
inserts must now include PK columns:

- **Natural keys**: show resolved values — `company_id: "C1"`
- **Surrogate keys**: show null — `id: null`

Both are honest about what the delta view actually produces. No schema
changes, no new annotations, no SQL changes.

## Files modified

| File | Change |
|------|--------|
| `tests/integration.rs` | Removed PK stripping from both `execute_example` and `verify_test_expected` |
| `examples/*/mapping.yaml` | Added PK columns (null or resolved) to expected inserts |
