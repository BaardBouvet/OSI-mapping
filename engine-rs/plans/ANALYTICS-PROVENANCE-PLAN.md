# ANALYTICS-PROVENANCE-PLAN

Extend the analytics view to optionally include source provenance, so
consumers can correlate golden records with the raw source data that
produced them.

## Problem

The analytics view (`{target}`) exposes clean resolved business fields with
`_cluster_id`. But consumers often need to answer:

- "Which sources contributed to this golden record?"
- "What was CRM's original value before coalesce picked ERP's?"
- "Show me the golden record alongside each source's raw contribution"

Today this requires manually querying internal views (`_id_{target}`,
`_fwd_{mapping}`) — which are not documented, not stable, and not
consumer-friendly.

### Real-world use cases

1. **Data stewardship UI.** Show a golden contact record with a "sources"
   panel listing each contributing system, its original values, and when
   they last changed.

2. **Audit / compliance.** "Prove where this customer's email came from."
   Requires tracing the resolved value back to a specific source row.

3. **Conflict review.** "3 sources disagree on the company name — which one
   won and what were the others?" Coalesce picks one; the user wants to see
   all candidates.

4. **Analytics enrichment.** "For each golden company, show me how many
   sources contribute, which systems are involved, and what percentage of
   fields come from each source."

## Current state

| What exists | What it provides | Consumer-friendly? |
|-------------|-----------------|-------------------|
| `{target}` (analytics) | `_cluster_id` + resolved fields | ✅ Yes |
| `_resolved_{target}` | `_entity_id` + resolved fields | ❌ Internal |
| `_id_{target}` | `_entity_id_resolved` + `_mapping` + `_src_id` | ❌ Internal |
| `_fwd_{mapping}` | Source columns → target fields (per mapping) | ❌ Internal |

The provenance view concept was sketched in
[ORIGIN-PLAN](ORIGIN-PLAN.md#provenance-view-optional) but never
implemented:

```sql
CREATE OR REPLACE VIEW _provenance_{target} AS
SELECT _entity_id_resolved AS _cluster_id, _mapping, _src_id
FROM _id_{target};
```

This tells you *which source rows* are in each cluster, but not *what values*
they contributed. For stewardship UIs and conflict review, you need the
actual field values alongside the provenance metadata.

## Design

Two new views per target, both opt-in:

### 1. Provenance view: `_provenance_{target}`

Lists all source rows belonging to each cluster with their mapping and
source primary key.

```sql
CREATE OR REPLACE VIEW _provenance_{target} AS
SELECT
  _entity_id_resolved AS _cluster_id,
  _mapping,
  _src_id
FROM _id_{target};
```

**Use case:** Cluster composition queries, source counting, system coverage.

```sql
-- How many sources contribute to each company?
SELECT _cluster_id, count(distinct _mapping) as source_count
FROM _provenance_company
GROUP BY _cluster_id;

-- Which companies are only known to CRM?
SELECT c._cluster_id, c.name
FROM company c
JOIN _provenance_company p ON p._cluster_id = c._cluster_id
GROUP BY c._cluster_id, c.name
HAVING count(distinct p._mapping) = 1
   AND min(p._mapping) = 'crm_companies';
```

### 2. Contributions view: `_contributions_{target}`

Shows each source's actual contributed field values alongside the cluster
they belong to. This is the provenance view enriched with field data.

```sql
CREATE OR REPLACE VIEW _contributions_{target} AS
SELECT
  id._entity_id_resolved AS _cluster_id,
  id._mapping,
  id._src_id,
  fwd.field_a,
  fwd.field_b,
  ...
FROM _id_{target} id
JOIN (
  -- UNION ALL of all forward views for this target
  SELECT _src_id, _mapping, field_a, field_b, ... FROM _fwd_mapping1
  UNION ALL
  SELECT _src_id, _mapping, field_a, field_b, ... FROM _fwd_mapping2
  ...
) fwd ON fwd._src_id = id._src_id AND fwd._mapping = id._mapping;
```

**Use case:** Conflict review, stewardship UI, audit trails.

```sql
-- Show all source values alongside the golden record for a company
SELECT
  c._cluster_id,
  c.name AS resolved_name,
  ct._mapping AS source,
  ct.name AS source_name,
  ct._src_id
FROM company c
JOIN _contributions_company ct ON ct._cluster_id = c._cluster_id
WHERE c._cluster_id = 'abc123';
```

Result:
```
_cluster_id | resolved_name | source          | source_name  | _src_id
abc123      | Acme Corp     | crm_companies   | Acme Corp    | CRM-001
abc123      | Acme Corp     | erp_companies   | ACME CORP.   | E-5001
abc123      | Acme Corp     | web_companies   | Acme          | W-42
```

The consumer sees that CRM won the coalesce (name matches resolved), ERP
had a different casing, and web had a truncated name.

### Why two views instead of one

The provenance view is cheap (no join, just projection from `_id_{target}`)
and covers the common "which sources?" query. The contributions view is
heavier (UNION ALL of forward views + join) and only needed for conflict
review. Keeping them separate lets consumers pick the right cost level.

## Opt-in mechanism

Both views are only useful when the consumer needs source-level detail.
Generate them when the target has at least one mapping with `sync: true`
(same gate as reverse/delta views), since those are the targets with active
ETL interest.

Alternatively, a simpler gate: always generate the provenance view (it's
trivial), and generate the contributions view only when explicitly requested
via a target-level property:

```yaml
targets:
  company:
    contributions: true    # generate _contributions_company view
    fields:
      ...
```

**Recommendation:** Always generate the provenance view (near-zero cost).
Gate the contributions view behind `contributions: true` on the target.

## DAG placement

```
_fwd_{mapping1} ──┐
_fwd_{mapping2} ──┤
                  ├──► _id_{target} ──► _resolved_{target} ──► {target} (analytics)
                  │         │
                  │         └──► _provenance_{target}
                  │
                  └──────────────► _contributions_{target}
```

The provenance view depends on `_id_{target}` only.
The contributions view depends on `_id_{target}` + all `_fwd_*` for the target.

### New DAG node types

```rust
enum ViewNode {
    // ... existing variants ...
    Provenance(String),      // _provenance_{target}
    Contributions(String),   // _contributions_{target}
}
```

## Scope of changes

### Provenance view (always generated)
- New render function in `analytics.rs` (or new `provenance.rs`): ~20 lines
- DAG: add `Provenance` variant, dependency on identity view
- DOT: distinctive shape for provenance nodes

### Contributions view (opt-in)
- New `contributions: bool` field on `Target` model
- New render function: ~40 lines (UNION ALL of forward views + join)
- DAG: add `Contributions` variant, dependencies on identity + all forward views
- Schema: add `contributions` property to target definition
- Validation: verify at least one mapping exists for target when contributions enabled

### Test infrastructure
- Extend `TestCase` model to support `contributions:` expected data
- Or: verify via column existence checks (simpler)

## Open questions

1. **Metadata columns in contributions.** Should `_contributions_{target}`
   also expose `_priority` and `_last_modified`? Useful for "why did this
   source win?" analysis, but adds noise. Proposal: include them — the view
   is already for power users.

2. **Nested arrays / child targets.** Should `_contributions_phone_entry`
   exist? It would show each source's phone contributions per cluster. Makes
   sense but adds complexity. Proposal: yes, same mechanism — UNION ALL of
   forward views for that target.

3. **Performance.** The contributions view is a UNION ALL + join. For large
   datasets, consumers should `WHERE _cluster_id = ?` rather than scanning
   the whole thing. Could add a note about materializing if needed.

4. **Naming.** `_provenance_` vs `_sources_` vs `_members_`? And
   `_contributions_` vs `_raw_` vs `_originals_`? Current naming is
   descriptive: provenance = "where it came from", contributions = "what
   each source gave."
