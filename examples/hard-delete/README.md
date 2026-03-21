# Hard-delete detection

Propagate hard-deleted entities (source row gone, no tombstone) using
`derive_tombstones` to synthesize a deletion field that flows through
resolution and enables per-consumer reaction via `reverse_filter`.

## Scenario

Two systems (CRM and ERP) synchronize customer records. The ETL
maintains a `cluster_members` feedback table that records which
entities were synced to which source.

Between sync cycles, a user deletes Alice from ERP (hard delete — the row
is gone). Without detection, the engine treats Alice as a new entity and
re-inserts her into the system that just removed her. This creates a
re-insertion loop.

With `derive_tombstones: is_deleted`, the engine detects Alice's absence
(present in `cluster_members` but not in the source table) and synthesizes
`is_deleted = TRUE`. Resolution combines the signal via `bool_or`, and
CRM's `reverse_filter` triggers `action = 'delete'` — propagating the
deletion across systems.

## Key features

- **`derive_tombstones: is_deleted`** — synthesizes a boolean target field
  for entities that were previously synced but are now absent from the source
- **`is_deleted: { strategy: bool_or }`** — any source signaling deletion
  makes the entity deleted everywhere
- **`reverse_filter: "is_deleted IS NOT TRUE"`** — each consumer decides
  how to react; here both CRM and ERP exclude deleted entities
- **`cluster_members: true`** — the ETL feedback table that persists when
  the source row disappears

## How it works

1. ERP previously synced Alice (entry in `_cluster_members_erp_customers`).
2. Alice's row is hard-deleted from ERP (gone from the source table).
3. The engine detects the absence and synthesizes a row with
   `is_deleted = TRUE`, all other fields NULL.
4. Resolution combines: `bool_or(TRUE)` → `is_deleted = TRUE`.
5. CRM's `reverse_filter` evaluates `is_deleted IS NOT TRUE` → FALSE →
   `action = 'delete'`.
6. The ETL connector for CRM handles the physical deletion.

If Alice reappears in ERP later, her source row returns, the synthetic
row is no longer needed, `is_deleted` reverts to NULL/FALSE, and the
entity naturally resurfaces.

## When to use

- Sources hard-delete records (row removed, no soft-delete marker).
- You want deletion to propagate through resolution to other consumers.
- The ETL already maintains `cluster_members` for insert feedback.

## See also

- [propagated-delete](../propagated-delete/README.md) — soft-delete
  propagation using expression mapping + `reverse_filter`
- [element-hard-delete](../element-hard-delete/README.md) — element-level
  deletion detection using `derive_element_tombstones`
- [soft-delete](../soft-delete/README.md) — local soft-delete detection
