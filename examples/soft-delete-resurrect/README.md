# Soft-delete resurrection

Resurrect a soft-deleted entity when another source still has it
active, using `soft_delete.target` to route detection into a shared
field and `BOOL_AND` resolution for consensus-based deletion.

## Scenario

Two systems (CRM and ERP) synchronize customer records. CRM supports
soft delete — when a user deletes a customer, the row stays but
`deleted_at` is set.

A user soft-deletes Alice in CRM, but ERP still has her as an active
customer. The business rule is: an entity is only truly deleted when
**all** sources agree. Since ERP disagrees, Alice should be resurrected
in CRM by clearing the `deleted_at` marker.

## Key features

- **`soft_delete: { field: deleted_at, target: is_deleted }`** — routes the
  soft-delete detection into a target field instead of suppressing the row.
  Non-identity fields are auto-nullified so the soft-deleted source yields
  the floor in resolution.
- **`strategy: expression` with `BOOL_AND`** — the entity is only deleted
  when every source agrees. A single active source overrides the deletion.
- **`expression: "FALSE"` on ERP** — ERP explicitly contributes "not deleted"
  so that `BOOL_AND(TRUE, FALSE) = FALSE`.
- **`source: deleted_at` → `target: deleted_at`** — mapping the soft-delete
  column as a regular data field is the key to resurrection. When detection
  fires, auto-nullification produces `deleted_at = NULL` in the forward view.
  Resolution picks up NULL, and the delta detects the difference between
  CRM's current timestamp and the resolved NULL, triggering an update that
  clears the marker.
- **`reverse_filter: "is_deleted IS NOT TRUE"`** — each consumer independently
  decides how to react to the resolved deletion state.

## How it works

1. CRM mapping declares `soft_delete: { field: deleted_at, target: is_deleted }`.
2. Alice's `deleted_at` is non-null → detection fires:
   - `is_deleted = TRUE` is injected into the forward view.
   - Non-identity fields (`name`, `deleted_at`) are auto-nullified.
3. ERP contributes `is_deleted = FALSE` via expression mapping.
4. Resolution: `BOOL_AND(TRUE, FALSE) = FALSE` → entity is not deleted.
5. Resolved `deleted_at = NULL` (CRM's value was auto-nullified, ERP has none).
6. CRM's `reverse_filter` passes (`FALSE IS NOT TRUE` → true).
7. Delta compares CRM's current `deleted_at` ("2026-03-15") against
   resolved (`NULL`) → mismatch → action = `'update'`.
8. The ETL writes back `deleted_at = NULL` to CRM, clearing the marker.

When Alice is soft-deleted in CRM **and** absent from ERP, only CRM
contributes: `BOOL_AND(TRUE) = TRUE`. The `reverse_filter` fails and
the delta emits `'delete'` — confirming the deletion.

## When to use

- A source supports soft delete and you want other sources to be able
  to override the deletion decision.
- The business rule is consensus-based: only deleted when all sources agree.
- You need the soft-delete marker physically cleared in the originating
  source when the entity is resurrected.

## See also

- [soft-delete](../soft-delete/README.md) — local soft-delete suppression
  (no resurrection, the default behavior)
- [hard-delete](../hard-delete/README.md) — hard-delete propagation via
  `derive_tombstones`
