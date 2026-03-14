# Diamond Avoidance Plan

## The Problem

The reverse/sync view has a structural diamond dependency:

```
           _id_{target}
            │         │
            ▼         │ (LEFT JOIN)
     _resolved_{target}    │
            │         │
            ▼         ▼
         sync_{mapping}
```

Both `_resolved` and `sync` depend on `_id`. For **ordered materialized view
refresh** this is fine — by the time `sync` refreshes, both `_id` and
`_resolved` are already current. But for **streaming IVM** (e.g., pg_ivm,
Materialize), when `_id` changes, the system would need to reconcile the sync
view seeing the new `_id` with a stale `_resolved`, which is incorrect.

## Why It Exists

The sync view needs two things from identity:

1. **Resolved field values** — via `_resolved` (which aggregates from `_id`)
2. **Per-source-row membership** — which specific `_src_id` from this mapping
   belongs to each entity, plus the raw `_base` JSONB

Item 1 flows cleanly through the funnel. Item 2 is the problem — we need to
join back to `_id` to recover per-row identity (which entity this source row
belongs to, and its raw forward columns).

## Options

### Option A: Carry membership info through resolution

Instead of joining back to `_id` in the sync view, carry per-source-row
information forward into the resolution view.

**How:** The resolution view currently groups by `_entity_id_resolved`. Extend
it to group by `(_entity_id_resolved, _mapping, _src_id)` — but only for
source-row-level metadata (`_base`, `_src_id`, PKs). The business fields still
use the same aggregation.

**Problem:** This destroys the "one row per entity" property of resolution.
We'd get N rows per entity (one per contributing source row). That's a different
beast — more like a denormalized output. The analytics view would need to
deduplicate, and the resolution view becomes huge.

**Verdict:** Bad. Violates the clean "funnel narrows at resolution" design.

### Option B: Embed _base in a JSONB array during resolution

Aggregate all per-source-row `_base` values into a `jsonb_agg(...)` during
resolution, keyed by `(_mapping, _src_id)`. The sync view then extracts the
relevant `_base` from this aggregate without joining back to identity.

```sql
-- In resolution:
jsonb_object_agg(_mapping || ':' || _src_id, _base) AS _bases

-- In sync:
_bases->>('{mapping}:' || _src_id) AS _base
```

**Problem:** Resolution still doesn't carry `_src_id` — it groups by entity.
We'd need to `GROUP BY _entity_id_resolved` and aggregate membership as JSONB.
Doable, but:
- The JSONB blob grows with entity size
- Extracting individual rows back out is awkward
- We still need to know which `_src_id` values belong to this mapping — which
  requires an unnest or cross-reference

**Verdict:** Feasible but ugly. Adds significant complexity to resolution for
the sake of IVM purity.

### Option C: Make the sync view a function, not a view

Instead of a SQL view, generate a `REFRESH FUNCTION` that materializes the sync
output into a table. The function reads `_id` and `_resolved` at call time,
which is always consistent (snapshot isolation).

**Problem:** Functions aren't views — they don't participate in IVM at all.
This abandons the view-based approach entirely for the sync layer.

**Verdict:** Viable for ETL use cases but loses the elegant "everything is a
view" property.

### Option D: Accept the diamond for sync, document it

The diamond is only a problem for streaming IVM engines. For the common use
cases (materialized view refresh, ETL pipelines, direct querying), ordered
refresh works perfectly:

1. Refresh `_id_{target}`
2. Refresh `_resolved_{target}`  
3. Refresh `sync_{mapping}` — reads consistent `_id` and `_resolved`

Document this explicitly:
- The analytics view (`{target}`) has a clean linear dependency chain — IVM-safe
- The sync view (`sync_{mapping}`) has a diamond — use ordered refresh

This is actually the pragmatic choice: most users will use analytics views
(no diamond). Sync views are for ETL write-back, which runs in batch anyway.

**Verdict:** Best balance of simplicity and honesty.

### Option E: Duplicate identity columns into resolution

Add per-mapping "member lists" as array columns in resolution:

```sql
-- In resolution, for each mapping:
array_agg(_src_id) FILTER (WHERE _mapping = 'crm') AS _members_crm
```

The sync view can then unnest `_members_crm` instead of joining back to
identity.

**Problem:** Still need `_base` per source row. Could aggregate as JSONB:
```sql
jsonb_object_agg(_src_id, _base) FILTER (WHERE _mapping = 'crm') AS _bases_crm
```

Then sync does:
```sql
SELECT ... FROM _resolved CROSS JOIN LATERAL (
  SELECT key AS _src_id, value AS _base
  FROM jsonb_each(_bases_crm)
  UNION ALL
  -- plus one synthetic NULL row for entities with no member → insert
) AS members
```

**Verdict:** Technically eliminates the diamond but adds significant complexity.
The resolution view becomes mapping-aware (currently it's pure target-level
aggregation). Not worth it unless streaming IVM is a hard requirement.

## Recommendation

**Option D: Accept and document.** The diamond exists, it's honest to say so.
The mitigation is simple (ordered refresh) and matches how these views will
actually be used. The IVM-safe path (analytics only) covers the majority of
users who just want combined data.

If streaming IVM becomes a hard requirement in the future, Option E is the
technical path forward — but it should be driven by a real user need, not
prophylactic architecture.

## Action Items

- [ ] Fix docs: remove "no diamonds" claim for the sync view path
- [ ] Add a "Refresh Order" section explaining the materialization strategy
- [ ] Clearly distinguish: analytics path = IVM-safe, sync path = ordered refresh
