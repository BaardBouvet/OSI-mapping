# Analytics View Plan

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
