# Soft-delete detection

Detect soft-deleted entities (source row exists but is semantically
deleted) via a `tombstone_field` column.

## Scenario

Two systems (CRM and ERP) synchronize customer records. CRM supports
soft delete — when a user deletes a customer, the row stays but
`deleted_at` is set. ERP has no soft-delete mechanism.

Without detection, the engine sees the CRM row as normal and emits
`'update'` or `'noop'` — the soft deletion has no effect on the delta.
Other sources may re-insert the customer into CRM, creating a loop.

With `tombstone_field: deleted_at`, the engine detects that the field
differs from its alive value (null by default) and treats the entity
as soft-deleted. When `resurrect: false` (default), the row is
suppressed. When `resurrect: true`, the delta emits `'update'` with
the alive value so the ETL can clear the soft-delete marker.

## Key features

- **`tombstone_field`** — source column carrying the deletion signal.
  When the column differs from `alive` (default: null), the entity is
  soft-deleted.
- **`alive`** — optional property specifying the "not deleted" value.
  Defaults to null. Set to `false` for boolean flags, or a string for
  enum values.
- **`resurrect`** — controls behavior: `false` (default) suppresses,
  `true` enables undelete.
- **No UNION ALL** — soft-deleted rows still exist in the source, so
  there is no vanished-entity query. Only hard deletes (row gone)
  produce the vanished-entity UNION ALL.

## How it works

1. The CRM mapping declares `tombstone_field: deleted_at`.
2. The delta CASE evaluates the detection before normal insert/update/noop:
   - `WHEN _src_id IS NOT NULL AND ("deleted_at" IS NOT NULL) THEN NULL`
3. Soft-deleted rows produce `NULL` action — excluded from the delta.
4. Active rows proceed through normal insert/update/noop logic.
5. ERP's delta is completely unaffected — no tombstone field declared.
