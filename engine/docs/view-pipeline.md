# View Pipeline

Each mapping file produces a DAG of PostgreSQL views. This document describes what each phase does.

```
source table
     │
     ▼
_fwd_{mapping}          Forward: normalize source → target shape
     │
     ▼
_id_{target}            Identity: transitive closure, entity linking
     │
     ▼
_resolved_{target}      Resolution: merge contributions, pick winners
     │
     ▼
_rev_{mapping}          Reverse: project resolved target → source shape
     │
     ▼
_delta_{mapping}        Delta: classify changes (insert/update/delete)
```

## Forward (`_fwd_{mapping}`)

Projects source columns into target field names. One view per mapping.

- Applies field expressions (e.g., `UPPER(name)`)
- Applies forward filters (`WHERE type = 'customer'`)
- Emits metadata: `_src_id`, `_mapping`, `_cluster_id`, per-field `_priority_*` and `_ts_*`
- Builds `_base` JSONB from raw source columns (when `include_base` is enabled on any mapping for this target)
- Handles nested arrays via `LATERAL jsonb_array_elements`
- Joins `_cluster_members` table when declared

All forward views for the same target emit identical column sets for UNION ALL compatibility.

## Identity (`_id_{target}`)

Links records across sources into entities. One view per target.

- UNIONs all forward views for this target
- Computes `_entity_id = md5(_mapping || ':' || _src_id)` — deterministic, per-source-row
- Runs recursive transitive closure on identity fields (and `_cluster_id` when cluster config exists)
- Produces `_entity_id_resolved` — the canonical entity ID (MIN of connected component)
- Incorporates link edges from `links` declarations

Output: every forward row augmented with `_entity_id` and `_entity_id_resolved`.

## Resolution (`_resolved_{target}`)

Merges all contributions for each entity into one golden record. One view per target.

- Groups by `_entity_id_resolved`
- Applies per-field resolution strategies:
  - `identity` → `min(field)` (deterministic pick)
  - `coalesce` → first non-NULL ordered by priority
  - `last_modified` → value with newest timestamp
  - `expression` → custom SQL aggregate
  - `collect` → `array_agg`
- Groups (`group:`) resolve atomically — all fields in a group come from the same winning source

Output: one row per entity with resolved field values.

## Reverse (`_rev_{mapping}`)

Projects the resolved golden record back into source shape. One view per mapping.

- `FROM _resolved LEFT JOIN _id` — every entity gets a row, even those without a member from this mapping (`_src_id = NULL`)
- Identity/collect fields: `COALESCE(id.field, r.field)` — source's own value when it exists, resolved value for inserts
- Other fields: `r.field` — the resolved/merged winner
- Restores human-readable PK columns from `_src_id`
- Passes through `_base` from the identity view (built in forward)
- No WHERE clause — all filtering deferred to delta

## Delta (`_delta_{mapping}`)

Classifies each row as an insert, update, delete, or noop. One view per mapping.

Single SELECT from the reverse view with a CASE expression:

- `_src_id IS NULL` → **insert** (entity exists but not in this source)
- `reverse_required` field is NULL → **delete** (resolved value can't satisfy this source)
- `reverse_filter` fails → **delete**
- All fields match `_base` → **noop** (when `detect_noop` is enabled; no write needed)
- Otherwise → **update**

Includes: `_action`, `_src_id`, `_cluster_id`, PK columns, business fields, `_base` (when enabled).
