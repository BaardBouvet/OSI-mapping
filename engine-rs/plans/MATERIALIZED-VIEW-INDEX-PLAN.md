# Materialized views with unique indexes

**Status:** Design

> **Abstract**: Adds opt-in materialized view generation to the engine and
> emits `CREATE UNIQUE INDEX ... NULLS NOT DISTINCT` on each materialized
> view. The natural unique key varies by layer: the analytics view uses
> `(_cluster_id)`, while delta views use `(_cluster_id, {pk_columns...})`
> because self-merges produce multiple rows per entity. This plan covers the
> semantics of each layer, the PostgreSQL-specific constraint syntax, the
> engine and spec changes needed, and interaction with the dbt output path.

---

## Problem

All engine views are currently plain `CREATE OR REPLACE VIEW`. This is
correct for development and for databases with small data volumes, but
production deployments almost always need **materialized views**:

1. **Performance** — identity resolution uses a recursive CTE with
   transitive closure. Querying through five layers of un-materialized views
   forces PostgreSQL to re-execute the entire pipeline on every read.
2. **Concurrent reads** — applications and BI tools holding cursors against
   deep view stacks cause planner overhead and memory pressure.
3. **Incremental refresh** — `REFRESH MATERIALIZED VIEW CONCURRENTLY`
   requires a **unique index** on the materialized view. Without one,
   PostgreSQL falls back to a full-table replacement that blocks reads.

Today, operators who want materialized views must write their own DDL
outside the engine. This is error-prone because the correct unique key
depends on internal pipeline semantics (entity ID, source PK, cluster
membership) that only the engine knows.

---

## Background: row cardinality per view layer

Understanding the unique key requires understanding the cardinality of each
view.

### Analytics view (`{target}`)

One row per resolved entity. The unique key is trivially
`_entity_id` (exposed as `_cluster_id`).

```
┌───────────┐
│ _cluster_id │  ← md5 entity hash, globally unique
│ field_a   │
│ field_b   │
└───────────┘
```

### Resolved view (`_resolved_{target}`)

One row per entity. Unique on `_entity_id`.

### Identity view (`_id_{target}`)

One row per (source row × entity). Unique on `(_entity_id)` — each
forward-view row appears exactly once. `_entity_id_resolved` is **not**
unique because multiple source rows merge into the same entity.

### Forward view (`_fwd_{mapping}`)

One row per source row. Unique on `(_mapping, _src_id)` — the mapping name
is constant within a single forward view, so `_src_id` alone suffices.

### Reverse view (`_rev_{mapping}`)

One row per source row that the entity "owns" in this mapping, **plus** one
row per entity that has no representative in this mapping (insert
candidates with `_src_id IS NULL`). The LEFT JOIN between
`_resolved_{target}` and `_id_{target}` produces:

- Existing source rows: one row per `_src_id` → unique on `_src_id`
- Insert rows: one row per entity not present in this mapping →
  `_src_id IS NULL`, unique on `_cluster_id`

Overall unique key: `(_cluster_id, _src_id)` with `NULLS NOT DISTINCT` —
no two insert rows can share the same entity.

### Delta view (`_delta_{mapping}`)

Same cardinality as the reverse view (it is a direct `SELECT ... CASE`
over the reverse). The self-merge scenario is the critical case:

**Self-merge example** (two CRM rows merge into one entity):

| _cluster_id | id (PK) | name | _action |
|-------------|---------|------|---------|
| `e7f3...`   | `A1`    | Acme Corporation | noop |
| `e7f3...`   | `A2`    | Acme Corporation | update |

Both rows share `_cluster_id` because they belong to the same resolved
entity, but they have different primary keys because they are different
source rows. The unique key is `(_cluster_id, {pk_columns...})`.

**Insert rows** (entity exists but has no source row in this mapping):

| _cluster_id | id (PK) | name | _action |
|-------------|---------|------|---------|
| `a1b2...`   | NULL    | New Corp | insert |

Here `_cluster_id` is unique (one insert per entity), and PK columns are
NULL. With `NULLS NOT DISTINCT`, the composite unique index correctly
prevents duplicate insert rows for the same entity.

---

## PostgreSQL: `NULLS NOT DISTINCT`

PostgreSQL 15+ supports `NULLS NOT DISTINCT` on unique indexes:

```sql
CREATE UNIQUE INDEX idx ON mat_view (col_a, col_b)
  NULLS NOT DISTINCT;
```

Without this modifier, PostgreSQL treats each NULL as distinct, so
`(NULL, NULL)` and `(NULL, NULL)` are considered different — defeating
uniqueness on insert rows. With `NULLS NOT DISTINCT`, two rows with
identical NULL patterns in the indexed columns are treated as duplicates.

**Minimum PostgreSQL version**: 15 (released October 2022). At the time of
writing, PostgreSQL 15 is the oldest supported major version, so this is a
safe baseline.

### Why not use `_src_id` instead of PK columns?

The delta view exposes human-readable PK columns (e.g., `id`, `order_id`,
`line_no`) and `_cluster_id`, but does **not** expose the internal `_src_id`
text column. Using PK columns keeps the unique index over columns that
are visible and meaningful to delta consumers.

For the reverse view, `_src_id` is available and could serve as the unique
key together with `_cluster_id`. The engine can use whichever is available
at each layer.

---

## Design

### Opt-in via CLI flag

```
osi-engine render mapping.yaml --materialize
```

When `--materialize` is passed:

1. Every `CREATE OR REPLACE VIEW` becomes
   `CREATE MATERIALIZED VIEW IF NOT EXISTS`
2. After each materialized view, the engine emits a
   `CREATE UNIQUE INDEX IF NOT EXISTS` statement
3. A final `REFRESH MATERIALIZED VIEW CONCURRENTLY` script is emitted as a
   separate section (or file), since refresh order must follow the DAG

The flag applies to **all** views in the pipeline. Selective materialization
(e.g., only resolution + analytics) is deferred to the dbt output path
where per-model `materialized:` config is natural.

### Unique index per view layer

| View | Unique index columns | Notes |
|------|---------------------|-------|
| `_fwd_{mapping}` | `(_src_id)` | One row per source record |
| `_id_{target}` | `(_entity_id)` | One row per forward-view row |
| `_resolved_{target}` | `(_entity_id)` | One row per entity |
| `{target}` (analytics) | `(_cluster_id)` | Alias of `_entity_id` |
| `_rev_{mapping}` | `(_cluster_id, _src_id)` NULLS NOT DISTINCT | Insert rows have NULL `_src_id` |
| `_delta_{mapping}` | `(_cluster_id, {pk_cols...})` NULLS NOT DISTINCT | PK cols are NULL on inserts |

For delta views with **composite primary keys** (e.g., `order_id, line_no`),
the index includes all PK columns:

```sql
CREATE UNIQUE INDEX IF NOT EXISTS idx_delta_erp_orders
  ON _delta_erp_orders (_cluster_id, order_id, line_no)
  NULLS NOT DISTINCT;
```

### Index naming convention

```
idx_{view_name}
```

For example:
- `idx_company` (analytics view for target `company`)
- `idx__resolved_company`
- `idx__delta_crm_contacts`

The `IF NOT EXISTS` guard ensures idempotent re-runs.

### Generated SQL example

```sql
-- Analytics view (materialized)
CREATE MATERIALIZED VIEW IF NOT EXISTS "company" AS
SELECT
  _entity_id AS _cluster_id,
  name,
  email,
  phone
FROM _resolved_company;

CREATE UNIQUE INDEX IF NOT EXISTS idx_company
  ON "company" (_cluster_id);

-- Delta view (materialized)
CREATE MATERIALIZED VIEW IF NOT EXISTS "_delta_crm_contacts" AS
SELECT
  CASE ... END AS _action,
  _cluster_id,
  contact_id,
  name,
  email,
  _base
FROM _rev_crm_contacts;

CREATE UNIQUE INDEX IF NOT EXISTS idx__delta_crm_contacts
  ON "_delta_crm_contacts" (_cluster_id, contact_id)
  NULLS NOT DISTINCT;
```

### Refresh ordering

Materialized views cannot reference other materialized views during
`CREATE` — they capture a snapshot. The creation order follows the existing
DAG topological sort. Refresh must follow the same order:

```sql
-- Refresh script (generated separately or as a trailing section)
REFRESH MATERIALIZED VIEW CONCURRENTLY "_fwd_crm_contacts";
REFRESH MATERIALIZED VIEW CONCURRENTLY "_fwd_erp_contacts";
REFRESH MATERIALIZED VIEW CONCURRENTLY "_id_company";
REFRESH MATERIALIZED VIEW CONCURRENTLY "_resolved_company";
REFRESH MATERIALIZED VIEW CONCURRENTLY "company";
REFRESH MATERIALIZED VIEW CONCURRENTLY "_rev_crm_contacts";
REFRESH MATERIALIZED VIEW CONCURRENTLY "_delta_crm_contacts";
```

`CONCURRENTLY` requires the unique index — which is why this plan pairs
the two features. Without `CONCURRENTLY`, refresh takes an exclusive lock
that blocks all reads for the duration.

---

## Interaction with dbt output

The [DBT-OUTPUT-PLAN](DBT-OUTPUT-PLAN.md) already identifies
materialization as a per-model concern:

```yaml
# dbt_project.yml
models:
  staging:
    +materialized: view
  resolution:
    +materialized: table
  delta:
    +materialized: incremental
```

When the engine emits dbt models instead of raw SQL:

- **No `CREATE MATERIALIZED VIEW`** — dbt handles materialization
- **Unique indexes** are still valuable: emitted as `post-hook` in the
  model config or as a dbt `indexes:` property (dbt-core 1.7+):

```sql
-- models/marts/company.sql
{{ config(
    materialized='table',
    indexes=[
      {'columns': ['_cluster_id'], 'unique': True},
    ]
) }}

SELECT
  _entity_id AS _cluster_id,
  ...
FROM {{ ref('_resolved_company') }}
```

For delta models with incremental materialization:

```sql
-- models/delta/_delta_crm.sql
{{ config(
    materialized='incremental',
    unique_key=['_cluster_id', 'contact_id'],
    incremental_strategy='merge',
    indexes=[
      {'columns': ['_cluster_id', 'contact_id'], 'unique': True,
       'type': 'btree'},
    ]
) }}
```

The `NULLS NOT DISTINCT` modifier is not natively supported by dbt's
`indexes` config. Options:

1. **Post-hook**: `post_hook="CREATE UNIQUE INDEX IF NOT EXISTS ... NULLS NOT DISTINCT"`
2. **Custom materialization macro** that adds the modifier
3. **Defer to PostgreSQL 15+ default** — if the delta's ETL consumer
   filters out insert rows (which is common), the index without
   `NULLS NOT DISTINCT` still works for the remaining rows

For the raw SQL output path (`--materialize`), the engine emits the full
`NULLS NOT DISTINCT` syntax directly.

---

## Self-merge deep dive

Self-merge is the scenario where multiple source rows from the **same
mapping** resolve to the **same entity** via identity field matching. This
is the case that makes `_cluster_id` alone insufficient as a unique key on
reverse and delta views.

### How it happens

1. Source table `crm_contacts` has two rows:
   - `{id: "A1", email: "alice@acme.com", name: "Alice"}`
   - `{id: "A2", email: "alice@acme.com", name: "Alice A."}`

2. The mapping declares `email` as `strategy: identity`.

3. Forward views produce two rows, each with a distinct `_src_id` (`"A1"`,
   `"A2"`) but no shared `_cluster_id` yet (each gets `md5('crm:A1')` and
   `md5('crm:A2')`).

4. Identity resolution links them: both rows have `email =
   'alice@acme.com'`, so they receive the same `_entity_id_resolved`
   (the MIN of their connected component).

5. Resolution merges them into one golden record.

6. The reverse view LEFT JOINs resolved (1 row) back to identity (2 rows
   for this mapping) → **2 reverse rows**, both with the same `_cluster_id`
   but different `_src_id` / PK values.

7. The delta view inherits these 2 rows, each classified independently.

### Why `NULLS NOT DISTINCT` is needed

Insert rows appear when the resolved view contains an entity that has **no**
source-row member in this specific mapping. The reverse view's LEFT JOIN
produces a row with `_src_id IS NULL` and all PK columns NULL.

Without `NULLS NOT DISTINCT`, two insert rows with all-NULL PK columns
would be considered distinct by the unique index, silently allowing
duplicates. In practice this can't happen because each entity produces at
most one insert row — but `NULLS NOT DISTINCT` makes the constraint
airtight rather than relying on upstream correctness.

For the analytics and resolved views, `_cluster_id` / `_entity_id` is
never NULL, so `NULLS NOT DISTINCT` is unnecessary on those indexes.

---

## Implementation

### Engine changes

1. **CLI flag**: Add `--materialize` to the `render` subcommand.

2. **`ViewNode` metadata**: extend the existing `ViewNode` enum (or a
   parallel struct) with the unique-key column list for each view. The
   render functions already know the columns — the key derivation is:

   ```rust
   fn unique_key_columns(node: &ViewNode, source: Option<&Source>) -> Vec<String> {
       match node {
           ViewNode::Forward(_) => vec!["_src_id"],
           ViewNode::Identity(_) => vec!["_entity_id"],
           ViewNode::Resolved(_) => vec!["_entity_id"],
           ViewNode::Analytics(_) => vec!["_cluster_id"],
           ViewNode::Reverse(_) => vec!["_cluster_id", "_src_id"],
           ViewNode::Delta(mapping) => {
               let mut cols = vec!["_cluster_id"];
               cols.extend(source.primary_key.columns());
               cols
           }
       }
   }
   ```

3. **SQL emission**: in the render orchestrator, after each `CREATE VIEW`:
   - Replace `CREATE OR REPLACE VIEW` with `CREATE MATERIALIZED VIEW IF NOT EXISTS`
   - Append `CREATE UNIQUE INDEX IF NOT EXISTS idx_{view} ON {view} ({cols}) [NULLS NOT DISTINCT]`
   - `NULLS NOT DISTINCT` is appended only when the key includes nullable
     columns (reverse and delta views)

4. **Refresh script**: after all views, emit `REFRESH MATERIALIZED VIEW
   CONCURRENTLY` in topological order. This could be a separate `--refresh`
   flag or an `--emit-refresh` that writes a standalone refresh script.

5. **Drop-and-recreate**: materialized views cannot use `CREATE OR REPLACE`.
   The engine should emit `DROP MATERIALIZED VIEW IF EXISTS {view} CASCADE`
   before each `CREATE MATERIALIZED VIEW IF NOT EXISTS` when in
   `--materialize` mode. Alternatively, only drop when the view definition
   has changed — but this requires diffing, which adds complexity. The
   simpler approach is `IF NOT EXISTS` on initial deployment and a separate
   `--recreate` flag for schema migrations.

### dbt output changes

When emitting dbt models:

1. Add `indexes` config to each model's `{{ config(...) }}` block with the
   appropriate unique key columns.
2. For delta models, add `unique_key` for incremental strategy.
3. Document the `NULLS NOT DISTINCT` limitation and recommended post-hook
   workaround.

### Test changes

No changes to the integration test harness. The test runner uses plain
views (faster, no refresh needed). A dedicated CLI test can verify that
`--materialize` output parses as valid SQL.

---

## Scope

- New CLI flag: `--materialize`
- Render orchestrator: conditional materialized-view DDL emission
- Unique index emission per view layer
- Refresh script emission (same file or separate)
- dbt model `indexes` config (when dbt output is implemented)

## Non-goals

- Selective per-view materialization in raw SQL mode (use dbt for that)
- Automatic refresh scheduling (operational concern, not engine concern)
- Support for databases other than PostgreSQL (materialized views and
  `NULLS NOT DISTINCT` are PostgreSQL-specific; other dialects deferred to
  POLYGLOT-SQL-PLAN)
- IVM / streaming materialization (covered by DIAMOND-AVOIDANCE-PLAN
  analysis; `REFRESH CONCURRENTLY` is the supported path)
