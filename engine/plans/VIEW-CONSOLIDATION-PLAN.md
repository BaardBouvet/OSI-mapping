# View Consolidation Plan

## Summary

Refactor the view pipeline to reduce intermediate views and improve naming.
Four changes bundled together:

1. **Merge reverse+delta into one sync view with CTE**
2. **Collapse forward views into the leaf sync view as CTEs**
3. **Rename `_delta_` → `sync_` (consumer-facing, opt-in)**
4. **Drop `_` prefix on consumer-facing views; keep it on internal ones**

## Current state

```
source → _fwd_{mapping} → _id_{target} → _resolved_{target} → _rev_{mapping} → _delta_{mapping}
                                                              └──► _analytics_{target}
```

7 views for hello-world with 2 mappings:
`_fwd_crm`, `_fwd_erp`, `_id_contact`, `_resolved_contact`, `_analytics_contact`,
`_rev_crm`, `_rev_erp`, `_delta_crm`, `_delta_erp`

## Proposed state

```
source ──► _id_{target} ──► _resolved_{target} ──► {target}   (analytics, always)
                                                  └──► sync_{mapping}  (opt-in)
```

5 views for hello-world (assuming both mappings have `sync: true`):
`_id_contact`, `_resolved_contact`, `contact`, `sync_crm`, `sync_erp`

Without `sync: true`: just 3 views:
`_id_contact`, `_resolved_contact`, `contact`

### Change 1: Merge reverse+delta → sync (with CTE)

The reverse view is only consumed by the delta. Merge them into one view:

```sql
CREATE OR REPLACE VIEW sync_crm AS
WITH _rev AS (
  SELECT
    id._src_id,
    COALESCE(id._entity_id_resolved, r._entity_id) AS _cluster_id,
    id._src_id AS id,
    COALESCE(id.email, r.email) AS email,
    r.name AS name,
    id._base
  FROM _resolved_contact AS r
  LEFT JOIN _id_contact AS id
    ON id._entity_id_resolved = r._entity_id
    AND id._mapping = 'crm'
)
SELECT
  CASE
    WHEN _src_id IS NULL THEN 'insert'
    WHEN NOT (name NOT LIKE 'X%') THEN 'delete'
    WHEN _base->>'email' IS NOT DISTINCT FROM email::text
     AND _base->>'name'  IS NOT DISTINCT FROM name::text THEN 'noop'
    ELSE 'update'
  END AS _action,
  _src_id, _cluster_id, id, email, name, _base
FROM _rev;
```

Implementation:
- New `render/sync.rs` that combines reverse + delta logic
- Delete `render/reverse.rs` and `render/delta.rs`
- Remove `ViewNode::Reverse` and `ViewNode::Delta` from DAG
- Add `ViewNode::Sync(String)` — depends on `Resolved` + `Identity` (via join edge)

### Change 2: Inline forward views as CTEs in identity

Forward views are only consumed by the identity view. Inline them:

```sql
CREATE OR REPLACE VIEW _id_contact AS
WITH _fwd_crm AS (
  SELECT ... FROM crm LEFT JOIN _cluster_members_crm ...
),
_fwd_erp AS (
  SELECT ... FROM erp LEFT JOIN _cluster_members_erp ...
),
_id_base AS (
  SELECT * FROM _fwd_crm UNION ALL SELECT * FROM _fwd_erp
),
...
```

Implementation:
- `forward::render_forward_cte(mapping, source_meta, target)` → returns CTE body (no `CREATE VIEW`)
- `identity::render_identity_view` accepts forward CTEs instead of view names
- Remove `ViewNode::Forward` from DAG
- Source tables feed directly into Identity node

### Change 3: Rename and make sync opt-in

- New model field: `sync: bool` on `Mapping` (default: false)
- Sync views only generated when `sync: true`
- Rename: `_delta_{mapping}` → `sync_{mapping}` (no underscore prefix)

### Change 4: Consumer-facing names

| View | Internal? | Name |
|------|-----------|------|
| Identity | Yes | `_id_{target}` |
| Resolved | Yes | `_resolved_{target}` |
| Analytics | **No** | `{target}` |
| Sync | **No** | `sync_{mapping}` |

The analytics view becomes just `{target}`. Clean.

## Impact on DAG

Before:
```
Source → Forward → Identity → Resolved → Reverse → Delta
                                        └──► Analytics
```

After:
```
Source → Identity → Resolved → {target}  (analytics, always)
                              └──► sync_{mapping}  (opt-in)
```

Node types: `Source`, `Identity`, `Resolved`, `Analytics`, `Sync`
Removed: `Forward`, `Reverse`, `Delta`

## Impact on tests

- Integration test queries `_delta_{mapping}` → change to `sync_{mapping}`
- Dump tests reference `_fwd_*`, `_rev_*`, `_delta_*` → update view names
- Forward column-matching unit test → becomes an internal test of identity CTE generation
- hello-world mapping needs `sync: true` on both mappings for tests to work
- inserts-and-deletes mapping needs `sync: true` on both mappings

## Impact on examples

All examples with `test:` sections that verify updates/inserts/deletes need
`sync: true` added to their mappings. Examples that just demonstrate analytics
don't need it.

## Migration order

1. Add `sync: bool` to model
2. Create `render/sync.rs` (combining reverse + delta)
3. Modify `forward.rs` to expose CTE-only rendering
4. Modify `identity.rs` to accept forward CTEs
5. Update DAG: remove Forward/Reverse/Delta, rename Analytics
6. Update `render/mod.rs` orchestration
7. Update annotations
8. Update tests
9. Delete `render/reverse.rs`, `render/delta.rs`
10. Update docs
