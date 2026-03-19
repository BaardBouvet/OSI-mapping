# Analytics view

**Status:** Done

## Goal

Add a new view layer — `_analytics_{target}` — that exposes the resolved golden record in a clean, consumer-friendly shape suitable for BI tools and analytics queries. One view per target.

## Motivation

The existing resolved view (`_resolved_{target}`) is an internal pipeline stage: it uses the opaque `_entity_id` as its key and serves as input to the reverse views. Analysts and BI tools don't care about internal entity IDs — they need a stable `_cluster_id` and the resolved business fields, nothing else.

Today, to query the golden record you have to know to look at `_resolved_{target}` and mentally map `_entity_id` back to a meaningful identifier. The analytics view eliminates that friction.

## Design

### Output columns

| Column | Source | Description |
|--------|--------|-------------|
| `_cluster_id` | `_entity_id` (same value, aliased) | Stable entity identifier — the canonical md5 cluster ID |
| *{field}* | `{field}` from resolved | One column per target field, resolved values only |

No metadata columns (`_priority`, `_ts_*`, `_priority_*`, `_last_modified`, `_base`, `_src_id`, `_mapping`). The view is purely business data.

### SQL shape

```sql
CREATE OR REPLACE VIEW _analytics_{target} AS
SELECT
  _entity_id AS _cluster_id,
  field_a,
  field_b,
  ...
FROM _resolved_{target};
```

It's a trivial SELECT — no joins, no aggregation, just column aliasing/filtering. This keeps it:
- Cheap (nearly zero cost — Postgres optimizes it away)
- IVM-safe (single upstream dependency)
- Easy to understand

### DAG placement

```
_resolved_{target}
     │
     ├──► _rev_{mapping}      (existing)
     │
     └──► _analytics_{target}  (new)
```

The analytics view depends only on the resolved view. It does NOT create a diamond — the reverse views also depend on resolved, but nothing downstream depends on both analytics and reverse.

### Node type

Add `ViewNode::Analytics(String)` to the DAG. In DOT output it renders with a `tab` shape (or `note`) to visually distinguish it from pipeline views.

## Scope

- New file: `engine/src/render/analytics.rs`
- DAG: add `Analytics` variant, wire dependency from resolved
- Render orchestrator: emit analytics views after resolution views
- DOT: render with distinctive shape
- No CLI changes needed — the view is always emitted when there are target definitions
- No test harness changes needed — the integration test doesn't query analytics views (consumers do)

## Non-goals

- No row-level security or tenant filtering (out of scope)
- No materialization hints (that's the consumer's choice)
- No denormalization of referenced targets (the view just exposes what resolved has)

## Scope: analytics as the only consumer-facing data product

The engine produces several consumer-usable outputs — analytics views,
delta views, provenance/contributions views
([ANALYTICS-PROVENANCE-PLAN](ANALYTICS-PROVENANCE-PLAN.md)), and sync
status views. Of these, only the analytics view is positioned as a "data
product" for end users. The question arose whether the tool should support
additional data products beyond analytics: API views, segment views,
export shapes, aggregate summaries.

**Decision: the mapping tool should not generate serving-layer products.**

The engine should generate the outputs that *require* resolution logic,
identity graphs, and delta computation — these are the things only the
engine can produce:

| Output | Purpose | Only engine can produce? |
|--------|---------|------------------------|
| `{target}` (analytics) | Golden record for BI | Yes — resolution + identity |
| `_delta_{mapping}` | Sync changesets | Yes — reverse + noop detection |
| `_provenance_{target}` | Source attribution | Yes — identity graph |
| `_contributions_{target}` | Per-source field values | Yes — forward + identity |
| `_sync_status_{mapping}` | Written state feedback | Yes — written state + resolved |

Everything downstream — API views, filtered segments, export shapes,
aggregates — is a `SELECT ... FROM {target} WHERE ...` that any SQL
consumer, dbt model, or application can write. These don't need the
engine; they need the engine's output.

**Why not add them to the mapping schema:**
- Adding segment/API/export definitions blurs the boundary between
  "integration logic" (what data means, how it resolves) and "serving
  logic" (how data is shaped for specific consumers).
- The mapping schema's strength is declaring resolution semantics. Serving
  is the consumer's problem.
- The [DBT-OUTPUT-PLAN](DBT-OUTPUT-PLAN.md) already provides the right
  extension mechanism: the engine generates dbt `staging` models, and
  teams add their own `marts` for consumer-specific products.

**If data products are needed,** the dbt output is the right vehicle:
engine views become dbt staging models; teams extend with custom marts
for API views, segments, or exports. The engine handles the hard
integration logic; dbt handles the last mile.
