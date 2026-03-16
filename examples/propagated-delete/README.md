# Propagated Delete

GDPR-style deletion propagation using regular target fields — no special engine mechanics.

## How it works

1. CRM has a `deleted_at` timestamp column for soft-deletes
2. A `sql:` field mapping converts it to a boolean: `deleted_at IS NOT NULL` → `is_deleted`
3. The target field uses `strategy: bool_or` — if ANY source says deleted, the resolved value is `true`
4. ERP's mapping has `reverse_filter: "is_deleted IS NOT TRUE"` — when the resolved entity is marked deleted, ERP's delta emits a `'delete'` action

## Per-system control

Each system independently decides its response to the deletion signal via its own `reverse_filter`:

| System | Response | Configuration |
|--------|----------|--------------|
| ERP | Delete the record | `reverse_filter: "is_deleted IS NOT TRUE"` |
| Billing | Retain for legal reasons | No `reverse_filter` — record kept |
| Archive | Write the flag back | Map `is_deleted` to own column |

## What the tests show

| Test | Scenario |
|---|---|
| 1 | Alice soft-deleted in CRM → ERP gets delete signal, Bob is unaffected |
| 2 | No deletions — all deltas are noop |
| 3 | Single-source delete still propagates to the other system |
