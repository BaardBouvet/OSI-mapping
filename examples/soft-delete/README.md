# Soft-delete detection

Detect soft-deleted entities (source row exists but is semantically
deleted) via a `tombstone` configuration.

## Scenario

Two systems (CRM and ERP) synchronize customer records. CRM supports
soft delete — when a user deletes a customer, the row stays but
`deleted_at` is set. ERP has no soft-delete mechanism.

Without detection, the engine sees the CRM row as normal and emits
`'update'` or `'noop'` — the soft deletion has no effect on the delta.

With `tombstone: { field: deleted_at, undelete_value: null }`, the engine detects that
`deleted_at` is non-null and treats the entity as soft-deleted.
When `resurrect: false` (default), the row is suppressed — no stale
data is written back to CRM.  When `resurrect: true`, the delta emits
`'update'` with the undelete values so the ETL can clear the marker.

## Key features

- **`tombstone.field`** — source column carrying the deletion signal.
- **`tombstone.undelete_value`** — the value this field holds when NOT deleted.
  null means IS NOT NULL = deleted. Set to `false` for boolean flags, or a string
  for enum values. Mutually exclusive with `undelete_expression`.
- **`tombstone.undelete_expression`** — raw SQL expression for the tombstone
  field when undeleting. Requires `detect`. Mutually exclusive with `undelete_value`.
- **`tombstone.detect`** — optional SQL expression override for
  custom detection logic.
- **`tombstone.undelete_columns`** — optional map of additional columns
  to override when undeleting (keys auto-included as passthrough).
- **`resurrect`** — controls behavior: `false` (default) suppresses,
  `true` enables undelete.
- **No UNION ALL** — soft-deleted rows still exist in the source, so
  there is no vanished-entity query. Only hard deletes (row gone)
  produce the vanished-entity UNION ALL.

## How it works

1. The CRM mapping declares `tombstone: { field: deleted_at, undelete_value: null }`.
2. The delta CASE evaluates the detection before normal insert/update/noop:
   - `WHEN _src_id IS NOT NULL AND ("deleted_at" IS NOT NULL) THEN NULL`
3. Soft-deleted rows produce `NULL` action — excluded from the delta.
4. Active rows proceed through normal insert/update/noop logic.
5. ERP's delta is completely unaffected — no tombstone declared.
