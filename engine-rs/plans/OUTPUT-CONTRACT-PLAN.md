# Configurable output column names

**Status:** Maybe

Tracks the hardcoded parts of the engine's output contract — column names
in consumer-facing views that downstream ETL pipelines and BI tools reference
by name. Documents which are internal plumbing vs. consumer-facing, and how
they could be made configurable if needed.

## Current state

### Input tables: fully configurable (done)

All input table names are already overridable:

| Input | Config property | Default |
|-------|----------------|---------|
| Source datasets | `sources.{name}.table` | Source key name |
| Cluster members | `cluster_members.table` | `_cluster_members_{mapping}` |
| Cluster member columns | `cluster_members.cluster_id`, `.source_key` | `_cluster_id`, `_src_id` |
| Link tables | `sources.{name}.table` (link mappings use a source) | Source key name |

Planned input tables (`synced_entities`, `synced_elements`, `_overrides_`)
follow the same pattern — designed with `true` for defaults, object for
custom names.

### Internal view names: not configurable, not exposed

Generated view names use fixed prefixes. These are pipeline plumbing — not
part of the consumer contract:

| View | Name pattern | Consumer sees? |
|------|-------------|----------------|
| Forward | `_fwd_{mapping}` | No |
| Identity | `_id_{target}` | No |
| Resolved | `_resolved_{target}` | No |
| Ordered | `_ordered_{target}` | No |
| Analytics | `{target}` | **Yes** (user-defined target name) |
| Reverse | `_rev_{mapping}` | No (ETL reads delta, not reverse) |
| Delta | `_delta_{source}` | **Yes** |

The analytics view name is the target name itself — already user-controlled.

The delta view name and the cluster_members default table name both use
the `_` prefix despite being consumer-facing. The `_action` column in the
delta view has the same issue. "Delta" is also a misnomer — the view emits
desired state, not a diff. These naming inconsistencies are addressed in
[CONSUMER-NAMING-PLAN](CONSUMER-NAMING-PLAN.md).

### Internal plumbing columns: not configurable, not exposed

These flow between internal views and are never seen by consumers:

| Column | Used in | Purpose |
|--------|---------|---------|
| `_entity_id` | Identity → Resolved | Deterministic row identity (md5) |
| `_entity_id_resolved` | Identity → Resolved → Reverse | Transitive closure result |
| `_mapping` | Forward → Identity | Source mapping label |
| `_priority` | Forward → Resolved | Mapping-level priority |
| `_priority_{field}` | Forward → Resolved | Per-field priority |
| `_last_modified` | Forward → Resolved | Mapping-level timestamp |
| `_ts_{field}` | Forward → Resolved | Per-field timestamp |
| `_order_rank_{field}` | Ordered → Reverse → Delta | Array element ordering |
| `_id_base` | Identity (CTE) | Base union before closure |
| `_grp_{group}` | Resolved (CTE) | Atomic group aggregation |
| `_p`, `_merged`, `_e{i}` | Delta (CTE) | Internal delta CTEs |
| `__native_{field}`, `__gen_{field}` | Ordered (CTE) | Ordering intermediates |

These should **not** be configurable — they're generated identifiers that
never appear in consumer queries.

### Consumer-facing output columns: hardcoded

These are the columns that ETL pipelines and BI tools reference by name.
They appear in the two consumer-facing views — analytics and delta:

| View | Column | SQL | Notes |
|------|--------|-----|-------|
| **Analytics** | `_cluster_id` | `_entity_id AS _cluster_id` | Stable entity key for BI |
| **Delta** | `_action` | `CASE ... END AS _action` | ETL action instruction |
| **Delta** | `_cluster_id` | passthrough from reverse | Entity key for ETL |
| **Delta** | PK columns | passthrough from reverse | Real source PK (e.g. `contact_id`) |
| **Delta** | reverse fields | passthrough from reverse | Source columns the ETL writes back |
| **Delta** | `_base` | passthrough from forward | Compare-and-set baseline for ETL |

Note: `_src_id` is **not** a delta output column. It exists in the reverse
view and is used internally by the delta's CASE expression (`WHEN _src_id
IS NULL THEN 'insert'`), but the delta SELECT list emits the real PK
columns instead.

The reverse view (`_rev_{mapping}`) is **not** consumer-facing — it's an
internal pipeline stage between the resolved view and the delta view. Its
columns flow into the delta, which is where consumers actually read them.

All three engine-generated columns (`_action`, `_cluster_id`, `_base`)
use the `_` prefix to distinguish them from user-defined source/target
fields in the same SELECT. This is a namespace convention — without it,
a source column named `action` or `base` would collide.

## What could be configurable

After [CONSUMER-NAMING-PLAN](CONSUMER-NAMING-PLAN.md) renames `_delta_` →
`sync_` and `_cluster_members_` → `cluster_members_`, the remaining
rename candidates are:

| Column | Why rename | Example |
|--------|-----------|---------|
| `_cluster_id` | Leading underscore breaks some BI tools; "cluster" is implementation jargon | `entity_id`, `golden_id`, `master_id` |
| `_base` | Unlikely — only used by the ETL for noop diffing | — |

## How it would work

### Option A: top-level `output` config

A top-level YAML section that maps internal names to output aliases:

```yaml
output:
  columns:
    _cluster_id: entity_id
    _action: sync_action
    _src_id: source_key
```

Pros: single place to configure, applies uniformly.
Cons: new top-level section; aliases apply to all views which may not
always be desired.

### Option B: per-view overrides

```yaml
targets:
  customer:
    columns:
      _cluster_id: master_id

mappings:
  - name: crm_contacts
    delta_columns:
      _action: sync_action
      _cluster_id: entity_id
```

Pros: granular control.
Cons: more complex, more config surface.

### Option C: wrapper views (no engine change)

The consumer creates a wrapper view:

```sql
CREATE VIEW my_delta AS
SELECT _action AS sync_action, _cluster_id AS entity_id, *
FROM _delta_crm;
```

Pros: zero engine changes.
Cons: manual, not tracked in the mapping YAML, one more object to maintain.

## Recommendation

**Option C (wrapper views) for now.** The current column names are a
reasonable convention. Consumers who need different names can alias in SQL
or in their ETL layer (dbt, views, SELECT aliases). This avoids adding
config surface for a problem that may not materialise.

If repeated demand emerges, **Option A** is the cleanest path — a single
`output.columns` map that the render pipeline reads when emitting the
final SELECT aliases. The internal plumbing columns would be unaffected;
only the consumer-facing alias in the outermost SELECT changes.

## Implementation notes (if Option A is chosen)

1. **Model** — add `OutputConfig` to `MappingDocument` with a
   `columns: IndexMap<String, String>` mapping internal name → alias.

2. **Analytics render** — replace hardcoded `_entity_id AS _cluster_id`
   with `_entity_id AS {output_alias("_cluster_id")}`.

3. **Delta render** — replace hardcoded `AS _action` and `_cluster_id`
   references in the outermost SELECT with configured aliases.

4. **Validation** — only allow renaming the consumer-facing columns listed
   above. Reject attempts to rename internal plumbing columns.

5. **Downstream impact** — `synced_entities`, `synced_elements`, and
   `_overrides_` tables reference `_cluster_id` by name. If the analytics
   alias changes, these ETL-side tables use the _internal_ name (which
   stays `_cluster_id`), not the output alias. No conflict.
