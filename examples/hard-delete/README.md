# Hard-delete detection

Detect hard-deleted entities (source row gone, no tombstone) using
persisted sync state to distinguish "never synced" from "was synced,
now gone."

## Scenario

Two systems (CRM and ERP) synchronize customer records. The ETL pushes
changes bidirectionally: CRM inserts flow to ERP, and vice versa. The
ETL maintains a `cluster_members` feedback table that records which
entities were synced to which source.

Between sync cycles, a user deletes Alice from ERP (hard delete — the row
is gone). Without detection, the engine sees `_src_id IS NULL` for Alice in
ERP's delta and emits `'insert'` — re-inserting her into the system that
just removed her. This creates a re-insertion loop.

With `cluster_members` declared and `resurrect: false`, the engine LEFT JOINs
the feedback table into the delta view. Alice's entry exists in
`cluster_members` (she was previously synced) but her source row is gone.
The engine recognizes this as a hard delete and suppresses resurrection.

## Key features

- **`cluster_members: true`** — the ETL feedback table records which
  entities were synced; persists when the source row disappears
- **`resurrect`** — defaults to `false`, which suppresses resurrection of
  hard-deleted entities (exclude from the delta entirely). Set to `true`
  to allow re-insertion (opt out of detection).
- **Two detection paths** — `cluster_members` (ETL feedback) or
  `derive_tombstones` + `written_state` (noop/element state table).
  Either activates entity-level detection.

## How it works

1. The ERP mapping declares `cluster_members: true` (resurrect defaults
   to `false`, so detection is active).
2. The engine LEFT JOINs `_cluster_members_erp_customers` into the delta.
3. For each entity where `_src_id IS NULL` (no source row) but
   `_cm_hd._src_id IS NOT NULL` (previously synced), the engine emits
   `NULL` — the row is excluded from the delta entirely.
4. Entities absent from both the source AND `cluster_members` get the
   normal `'insert'` action — they are genuinely new.
5. Entities in `cluster_members` but absent from the resolved view
   entirely (gone from ALL sources) produce `'delete'` — cleaning up
   entities that vanished completely.

## When to use

- Sources hard-delete records (row removed, no soft-delete marker).
- You want to prevent the re-insertion loop where deleted entities keep
  coming back.
- The ETL already maintains `cluster_members` for insert feedback.

## See also

- [propagated-delete](../propagated-delete/README.md) — soft-delete
  propagation using `reverse_filter` (source keeps the row with a flag)
- [derive-tombstones](../derive-tombstones/README.md) — element-level
  deletion detection using the same `derive_tombstones` mechanism
- [derive-noop](../derive-noop/README.md) — noop detection using the
  same `_written` table
