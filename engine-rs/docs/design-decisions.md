# Engine Design Decisions

This document captures design decisions specific to the OSI reference engine — the Rust implementation that compiles mapping YAML into PostgreSQL views. For decisions about the mapping schema itself, see [docs/design/design-rationale.md](../../docs/design/design-rationale.md).

## View Pipeline Architecture

The engine generates a strict funnel of PostgreSQL views:

```
source table → _fwd_{mapping} → _id_{target} → _resolved_{target} ─┬─ {target}              (analytics, always)
                                                                   └─ _rev_{mapping} → _delta_{mapping}  (auto when mapping has reverse fields)
```

The analytics path is a strict linear chain — no diamonds, trivially IVM-safe. The reverse/delta path introduces one controlled diamond (reverse LEFT JOINs identity, see below) and is generated automatically for any mapping with bidirectional or reverse_only fields.

## Diamond Avoidance

**Constraint:** Every view on the analytics path has exactly one path from any upstream view.

**Why:** Diamond dependencies (two paths from a common ancestor to a downstream view) break Incremental View Maintenance (IVM). When a source row changes, the database must propagate through both paths and reconcile intermediate states. They also complicate manual `REFRESH MATERIALIZED VIEW` ordering.

**The analytics path** (`_fwd` → `_id` → `_resolved` → `{target}`) is diamond-free. No cross-layer joins, no shared ancestors.

**The reverse path** has one controlled diamond: `_rev_{mapping}` LEFT JOINs `_id_{target}` (which also feeds `_resolved` upstream). This means the reverse view depends on both `_resolved` and `_id`, forming a diamond through `_id`. This is an accepted trade-off — the reverse/delta views are not candidates for IVM (they serve ETL batch sync, not real-time materialization). The engine emits them in topological order so a simple sequential refresh works.

**How it's enforced in delta:** The delta view depends only on the reverse view. Classification logic (`reverse_required`, `reverse_filter`) is evaluated in the delta via a CASE expression, not by joining back to identity or source.

## Delta: Single View per Mapping

**Decision:** Each mapping produces one `_delta_{mapping}` view with an `_action` column (`insert`, `update`, `delete`) rather than three separate views.

This is purely an engine implementation choice — the spec doesn't prescribe the number of output views. Alternatives (separate `_ins_`, `_upd_`, `_del_` views) were rejected because most consumers process all three in one pass (CDC pipelines) and three views per mapping clutter the DAG.

## Delta: Single SELECT from Reverse

The delta view is a single `SELECT ... FROM _rev_{mapping}` with a CASE expression:

```sql
CASE
  WHEN _src_id IS NULL THEN 'insert'
  WHEN {reverse_required fails} THEN 'delete'
  ELSE 'update'
END AS _action
```

No joins, no UNION, no subqueries. Benefits:
- Trivially cheap — column aliasing and CASE evaluation only
- Source rows excluded by forward `filter` never appear as false deletes (they never enter the pipeline)
- IVM-safe — single dependency path

## Reverse View: LEFT JOIN, All Rows

The reverse view uses `FROM _resolved_{target} LEFT JOIN _id_{target}` so that entities without a member from this mapping still produce a row (with `_src_id IS NULL`). The delta classifies these as inserts.

No WHERE clause on the reverse view. All filtering is deferred to the delta. This keeps the reverse view simple and avoids the diamond problem.

## Identity: md5-Based Entity IDs

Entity identifiers are deterministic: `_entity_id = md5('{mapping}' || ':' || {src_id})`. Using md5 rather than raw composite strings avoids delimiter-collision issues with user data and produces fixed-width identifiers for efficient joins and grouping.

## PK Columns in Delta

The delta view includes human-readable PK columns (e.g., `id`, `order_id`) restored by the reverse view. `_src_id` is used internally in the pipeline but is not exposed in the delta output — consumers identify rows by their natural PK columns.

## _base: Raw Source Values as JSONB

Every delta view emits a `_base` column containing a JSONB object with the **raw source column values** for each mapped field.

**Built in the forward view:** The `_base` column is assembled in the forward view from raw source columns _before_ any expressions are applied:

```sql
-- in _fwd_{mapping}
jsonb_build_object('email', email, 'name', name) AS _base
```

If a field mapping has a forward expression (e.g., `expression: UPPER(name)`), `_base->>'name'` contains the raw `name` value, not the uppercased version.

**Flows through the funnel:** The `_base` column passes through the identity view via `SELECT * ... UNION ALL`. The reverse view reads it from the identity side (`id._base`), and the delta passes it through.

**No source table join:** An earlier design joined the source table in the reverse view (`LEFT JOIN source AS _src ON ...`). This was a diamond dependency — the source table feeds both the forward view and the reverse view. Building `_base` in the forward view eliminates the diamond entirely.

**Available on all action types:** For inserts, `_base` is NULL (no source row exists in this mapping's table). For deletes, `_base` contains the raw source data that was present before the delete was triggered.

**Why JSONB instead of per-field columns:** Fewer columns, no naming collisions with business fields, and the consumer can extract individual fields with `_base->>'name'`.

**Purpose:** Change detection and noop classification. The delta view uses `_base` to compare current source values against resolved values.

## Noop Detection

The delta view always includes a noop branch that compares each reverse-mapped field against its `_base` counterpart:

```sql
WHEN _base->>'email' IS NOT DISTINCT FROM email::text
 AND _base->>'name'  IS NOT DISTINCT FROM name::text
THEN 'noop'
```

Rows where all fields match are classified as `noop` instead of `update`. The ETL can then skip writes entirely for these rows.

**`IS NOT DISTINCT FROM`** handles NULLs correctly — if both the base and resolved values are NULL, it's still a noop.

**Cast to text:** The `::text` cast ensures the comparison works regardless of the column's native type, since `_base->>` always returns text.

**Ordering in CASE:** The noop check comes after insert/delete checks but before the final `ELSE 'update'`. A row must first survive the delete predicates before being eligible for noop classification.

## FK Reference Resolution

When a target field has `references:` (declaring it as an entity FK to another target), the reverse view must translate the resolved entity-level reference back to a source-level foreign key. Each source system uses its own ID namespace, so the engine needs to know *which mapping* to use when looking up the local FK value.

**Decision:** The field mapping declares `references: <mapping_name>` explicitly. No heuristics or naming conventions.

An earlier version used a longest-common-prefix (LCP) heuristic on source dataset names to guess the "same system" mapping. This was fragile and opaque — `crm_contacts` and `crm_companies` happened to share a prefix, but `sales_leads` and `support_tickets` would not group correctly. The LCP code was removed entirely.

**How it works in SQL:**

```sql
-- In _rev_{mapping}: translate entity reference to source FK
(SELECT ref_local._src_id
 FROM _id_{ref_target} ref_match
 JOIN _id_{ref_target} ref_local
   ON ref_local._entity_id_resolved = ref_match._entity_id_resolved
 WHERE ref_match._src_id = r.{target_field}::text
 AND ref_local._mapping = '{fm.references}'
 LIMIT 1) AS {source_field}
```

Without `references`, the reverse view passes through the raw entity-level value without translation.

## Cluster Identity and Insert Tracking

When the delta produces an insert, the ETL writes the new row and links it back to the entity's `_cluster_id`. Two mechanisms prevent duplicate inserts on subsequent runs:

- **`cluster_members: true`** — ETL writes `(_cluster_id, _src_id)` to a per-mapping table. The forward view LEFT JOINs this table.
- **`cluster_field: column_name`** — ETL writes `_cluster_id` directly to a source column.

Both produce the same result: a `_cluster_id` column in the forward view. Per-mapping tables are used (not a shared table) because source PKs differ in type across mappings.

## Validation: 7 Passes

The validator runs checks in order, where each pass assumes prior passes succeeded:

1. **JSON Schema** — structural correctness
2. **Unique names** — no ambiguous references
3. **Target references** — mappings point to real entities/fields
4. **Strategy consistency** — required companion properties exist
5. **Field coverage** — warns about unmapped target fields
6. **Test datasets** — test data matches declared sources
7. **SQL syntax** — optional parse check

## DAG Ordering

The engine builds a dependency graph and topologically sorts it. Views are emitted in dependency order so that each `CREATE OR REPLACE VIEW` statement finds its dependencies already defined. The DAG is also used for `dot` output visualization.

## Delta: Child Merge via LEFT JOIN

When a source has a primary mapping plus child mappings (via `parent:`) with reverse fields, the delta view merges them into a single row per source record using CTEs and LEFT JOINs on `_src_id`:

```sql
WITH
  _p AS (SELECT * FROM _rev_primary),
  _e1 AS (SELECT _src_id, field1, field2, _base FROM _rev_child1),
  _merged AS (
    SELECT _p.*, _e1.field1, _e1.field2,
           _p._base || _e1._base AS _base
    FROM _p LEFT JOIN _e1 ON _e1._src_id = _p._src_id
  )
SELECT <merged_action> AS _action, ... FROM _merged
```

**Why not UNION ALL:** The earlier approach emitted one partial row per mapping, each with NULLs for columns from other mappings. This forced the consumer to reassemble partial rows by PK — fragile and error-prone. The merged approach produces one complete row with all fields populated.

**`_base` merge:** JSONB `||` combines the base snapshots from all mappings, so noop detection covers all fields in one check.

**Insert/delete logic:** Only the primary mapping determines insert/delete classification. Child mappings contribute field values but not lifecycle events.

## Test Harness Design

Integration tests use testcontainers to spin up a real PostgreSQL instance. Each test case:

1. Creates source tables from YAML test data (type-inferred from JSON values)
2. Ensures cluster_members tables exist
3. Executes all rendered views
4. Verifies updates by querying `_delta_{mapping} WHERE _action = 'update'` and joining back to source tables for unmapped columns
5. Verifies inserts by querying `_delta_{mapping} WHERE _action = 'insert'` with `_cluster_id` seed resolution
6. Verifies deletes by querying `_delta_{mapping} WHERE _action = 'delete'` and comparing PK values

Expected `_cluster_id` values in tests use seed notation (`"crm:2"`) which gets resolved to the actual md5-based `_entity_id_resolved` at test time.
