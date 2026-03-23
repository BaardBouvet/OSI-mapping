# Soft-delete detection

Detect soft-deleted entities (source row exists but is semantically
deleted) via a `soft_delete` configuration.

## Scenario

Two systems (CRM and ERP) synchronize customer records. CRM supports
soft delete — when a user deletes a customer, the row stays but
`deleted_at` is set. ERP has no soft-delete mechanism.

Without detection, the engine sees the CRM row as normal and emits
`'update'` or `'noop'` — the soft deletion has no effect on the delta.

With `soft_delete: deleted_at`, the engine detects that `deleted_at`
is non-null and treats the entity as soft-deleted — the row is
suppressed and no stale data is written back to CRM.

## Key features

- **`soft_delete`** — string shorthand (field name, timestamp strategy)
  or object with `field` and optional `strategy`.
- **`strategy`** — `timestamp` (default), `deleted_flag`, or `active_flag`.
  Determines detection expression and undelete value automatically.
- **No UNION ALL** — soft-deleted rows still exist in the source, so
  there is no vanished-entity query. Only hard deletes (row gone)
  produce the vanished-entity UNION ALL.

## How it works

1. The CRM mapping declares `soft_delete: deleted_at`.
2. The delta CASE evaluates the detection before normal insert/update/noop:
   - `WHEN _src_id IS NOT NULL AND ("deleted_at" IS NOT NULL) THEN NULL`
3. Soft-deleted rows produce `NULL` action — excluded from the delta.
4. Active rows proceed through normal insert/update/noop logic.
5. ERP's delta is completely unaffected — no soft_delete declared.

## See also

- [soft-delete-resurrect](../soft-delete-resurrect/README.md) — resurrection
  via `soft_delete.target` when another source overrides the deletion
- [hard-delete](../hard-delete/README.md) — hard-delete propagation via
  `derive_tombstones`
