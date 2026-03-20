# Soft-delete detection

Detect soft-deleted entities (source row exists but is semantically
deleted) via a `tombstone` expression.

## Scenario

Two systems (CRM and ERP) synchronize customer records. CRM supports
soft delete — when a user deletes a customer, the row stays but
`deleted_at` is set. ERP has no soft-delete mechanism.

Without detection, the engine sees the CRM row as normal and emits
`'update'` or `'noop'` — the soft deletion has no effect on the delta.
Other sources may re-insert the customer into CRM, creating a loop.

With `tombstone: "deleted_at IS NOT NULL"`, the engine evaluates the
expression in the delta CASE. When true, the row is excluded from the
delta entirely (suppressed). This is independent of the `reinsert`
setting — tombstone always suppresses.

## Key features

- **`tombstone: "..."`** — SQL boolean expression evaluated per row.
  When true, the entity is treated as disappeared from this source.
- **Independent of `reinsert`** — tombstone suppression is always active.
  No detection mechanism (`cluster_members`, `derive_tombstones`) needed.
- **No UNION ALL** — soft-deleted rows still exist in the source, so
  there is no vanished-entity query. Only hard deletes (row gone)
  produce the vanished-entity UNION ALL.

## How it works

1. The CRM mapping declares `tombstone: "deleted_at IS NOT NULL"`.
2. The delta CASE evaluates the expression before normal insert/update/noop:
   - `WHEN _src_id IS NOT NULL AND (deleted_at IS NOT NULL) THEN NULL`
3. Soft-deleted rows produce `NULL` action — excluded from the delta.
4. Active rows proceed through normal insert/update/noop logic.
5. ERP's delta is completely unaffected — no tombstone expression.
