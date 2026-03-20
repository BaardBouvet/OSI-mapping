# Delayed enrichment from external providers

**Status:** Design

When a SaaS system like HubSpot creates a company record, it returns
immediately — then asynchronously enriches the row with metadata from public
sources (Clearbit, ZoomInfo, etc.) minutes or hours later. The enriched
version lands as an update on the **same source row** (same PK, new
`updated_at`). This pattern interacts with every pipeline stage and produces
failure modes that the basic update path doesn't exercise.

Relates to [EVENTUAL-CONSISTENCY-PLAN](EVENTUAL-CONSISTENCY-PLAN.md)
(visibility delays), [PRECISION-LOSS-PLAN](PRECISION-LOSS-PLAN.md) (lossy
noop), and [HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md)
(provenance tracking for ETL-inserted records).

---

## The happy path (already works)

The `_base` snapshot stored at forward-view time reflects what the ETL last
read from HubSpot. When enrichment lands, `updated_at` advances and field
values change. On the next ETL run:

1. `_fwd_hubspot` picks up the new values.
2. `_resolved_company` recomputes — enriched fields replace their previous
   (usually NULL) values.
3. `_rev_erp` projects the resolved record back to ERP shape.
4. `_delta_erp` compares against `_base` — only changed fields appear.
5. Noop suppression prevents re-emitting values that were already written.

**No engine change required for the basic case.** The pipeline treats
enrichment exactly like any other source update.

---

## Failure modes

### 1. Partial enrichment and atomic groups

For ungrouped fields, `coalesce` and `last_modified` both use
`FILTER (WHERE F IS NOT NULL)` — NULLs are excluded before picking a winner.
A NULL from a priority-1 source **cannot** shadow a non-NULL from priority 2.
With plain per-field resolution, partial enrichment is harmless: unenriched
fields are simply absent from the winner set, and lower-priority sources
fill the gap.

The risk surfaces with **atomic resolution groups** (`group:` property). The
`DISTINCT ON` CTE picks the single best *row* by group priority. If HubSpot
has `priority: 1` and its row has some group fields populated and others
NULL, the entire group comes from that row — including the NULLs:

```
_fwd_hubspot:  { street: "123 Main", city: NULL, _priority_street: 1 }
_fwd_erp:      { street: "456 Oak",  city: "Oslo", _priority_street: 2 }

Group CTE (DISTINCT ON, ORDER BY LEAST(priority)):
  → picks HubSpot row (priority 1)
  → street = "123 Main", city = NULL

ERP's complete address is lost because the group is resolved atomically.
```

The WHERE clause requires at least one group field to be non-NULL, so an
entirely-NULL row won't win. But a partially-populated row will.

**Impact:** Good data from lower-priority sources is replaced by NULLs for
the NULL fields within the group. Not a concern for ungrouped fields.

**Severity:** Medium — only affects `group:` fields where enrichment is
sparse. Ungrouped coalesce/last_modified fields are safe by construction.

### 2. Enrichment corrects an identity field → cluster merge

**Mechanism:** HubSpot creates company A with `domain: "acme.io"`. ERP
independently has company B with `domain: "acme.com"`. These are separate
clusters (different domain). Enrichment corrects A's domain to `"acme.com"`.

```
T0 — Before enrichment:
  _fwd_hubspot:  { _src_id: HS-1, domain: "acme.io", name: "Acme" }
  _fwd_erp:      { _src_id: ERP-5, domain: "acme.com", name: "ACME Inc" }
  _id_company:   two clusters (domains differ)
  _delta_erp:    noop for ERP-5; insert for HS-1's entity

T1 — After enrichment corrects domain:
  _fwd_hubspot:  { _src_id: HS-1, domain: "acme.com", name: "Acme" }
  _fwd_erp:      { _src_id: ERP-5, domain: "acme.com", name: "ACME Inc" }
  _id_company:   ONE cluster (transitive closure merges on domain)
  _resolved:     one golden record, resolution picks between "Acme" / "ACME Inc"
```

This is not a normal field update — it's a **merge event**. The T0 insert
into ERP for HS-1's entity is now wrong: that entity no longer exists as a
separate cluster. The delta must now produce:

- **delete** for the phantom record inserted at T0 (if one was created), or
- **update** on ERP-5 if the merged resolution differs from ERP's current data.

**Impact:** If the ETL already inserted a record into ERP for the pre-merge
HS-1 entity, there's now a duplicate in ERP. The next delta cycle will emit
a delete for one of the two ERP records (the one not selected as the
cluster's canonical `_src_id` for that mapping), but the transient state is
dangerous for systems that trigger workflows on insert.

**Severity:** High. This is the same phantom-insert problem described in
[EVENTUAL-CONSISTENCY-PLAN §2](EVENTUAL-CONSISTENCY-PLAN.md), but caused by
enrichment delay rather than visibility delay.

### 3. Enrichment populates a `references` FK before the parent exists

**Mechanism:** HubSpot creates a company with `parent_company_id: NULL`.
Enrichment fills `parent_company_id: HS-99`. The forward view now has a
`references: company` field pointing to HS-99. The reverse view resolves this
FK through the identity graph to find ERP's local ID for that parent entity.

If HS-99's entity has already been synced to ERP (`cluster_members` feedback
exists), the FK resolves: `parent_company_id → ERP-42`. No problem.

If HS-99 has **not** been synced to ERP yet, the LEFT JOIN in the reverse
view yields NULL:

```sql
-- _rev_erp reverse FK resolution
LEFT JOIN _id_company parent_id
  ON parent_id._entity_id_resolved = r._ref_parent_company_id
  AND parent_id._mapping = 'erp_companies'

-- parent_id._src_id is NULL → FK writes as NULL
```

**Impact:** Delta emits an update with `parent_company_id = NULL` (or whatever
the COALESCE fallback is), potentially overwriting a correct FK that the ERP
already had from another source. On the next cycle, after the parent is
synced, the FK resolves correctly — but the intermediate write may violate
referential integrity or trigger business logic on orphan records.

**Severity:** Medium-high for systems with enforced FK constraints. The
[FK-REFERENCES-PLAN](FK-REFERENCES-PLAN.md) LEFT JOIN design means
unresolved FKs silently become NULL rather than causing SQL errors, but NULL
overwrites are still harmful.

### 4. Multiple enrichment providers overwrite each other

**Mechanism:** Provider A enriches at T1, setting `industry: "Technology"`.
Provider B enriches at T2, setting `industry: "Software"`. Both update the
same HubSpot row's `updated_at`.

With `last_modified` on a single mapping, T2 wins (latest timestamp). This
is correct if the providers are sequential and the latest is most accurate.

With `coalesce` on a single mapping, priority is fixed at mapping time.
Provider B's update is invisible to the resolution strategy — both values
arrive through the same `_fwd_hubspot` with the same `_priority`, and the
engine picks non-deterministically within a priority tier (whichever row
appears first in the forward view).

With two separate mappings (`hubspot_enrichment_a`, `hubspot_enrichment_b`)
and different priorities, the fixed priority always wins regardless of which
provider ran last.

**Impact:** Non-deterministic or stale resolution when multiple providers
update the same field through the same mapping.

**Severity:** Medium. The problem is silent — no delta oscillation, just the
wrong value winning.

### 5. Downstream systems act on unenriched partial records

**Mechanism:** HubSpot row appears with only user-entered fields (`name`,
`email`). The ETL runs before enrichment completes. Delta emits an insert
into ERP with `industry: NULL, employee_count: NULL`. ERP triggers a
"new customer" workflow with incomplete data. 2 minutes later, enrichment
completes and the next cycle pushes updates — but the workflow already fired.

**Impact:** Poor UX (welcome emails with "[UNKNOWN INDUSTRY]") or incorrect
business logic (routing to wrong sales team because `employee_count` is NULL,
defaulting to "SMB" tier).

**Severity:** Context-dependent. High if workflows are irreversible.

---

## Interaction with pipeline stages

| Stage | Enrichment effect | Failure mode |
|-------|-------------------|--------------|
| Forward view | New field values, `_base` advances | §1 (group NULL bleed), §4 (provider conflicts) |
| Identity view | Changed identity field → cluster merge | §2 (phantom merge) |
| Resolution view | Inherits from forward + identity | §1, §2, §4 |
| Reverse view | FK resolution depends on parent existence | §3 (dangling FK) |
| Delta view | Updates for changed fields; merge → delete + update | §2 (phantom insert/delete), §5 (premature action) |

---

## Design: split-mapping pattern

Model the enriched and unenriched data as **two separate source mappings**
over the same physical table, distinguished by a filter. This gives per-field
control over priority, strategy, and visibility.

```yaml
sources:
  hubspot:
    table: hubspot_companies
    primary_key: hs_id

targets:
  company:
    fields:
      domain:
        strategy: identity
      name:
        strategy: coalesce
      industry:
        strategy: last_modified
      employee_count:
        strategy: last_modified
      parent_company_id:
        strategy: coalesce
        references: company

mappings:
  # User-entered data — always visible, high confidence
  - name: hubspot_raw
    source: { dataset: hubspot }
    target: company
    sync: true
    fields:
      - source: domain
        target: domain
      - source: name
        target: name
        priority: 1

  # Enriched data — only visible after enrichment completes
  - name: hubspot_enriched
    source: { dataset: hubspot }
    target: company
    filter: "enrichment_complete = true"
    sync: true
    fields:
      - source: domain
        target: domain
      - source: industry
        target: industry
        last_modified: enrichment_updated_at
      - source: employee_count
        target: employee_count
        last_modified: enrichment_updated_at
      - source: parent_company_id
        target: parent_company_id
```

### How this addresses each failure mode

**§1 — Group NULL bleed:** For ungrouped fields this is a non-issue —
`coalesce` and `last_modified` both skip NULLs. For grouped fields, the
`filter` on `hubspot_enriched` prevents partially-enriched rows from entering
the forward view at all. Once the filter passes, enrichment is complete and
group fields should be fully populated. If a group field can legitimately
remain NULL after enrichment, consider leaving it ungrouped so per-field
resolution handles it safely.

**§2 — Identity merge:** The `domain` identity field appears on both mappings,
but both point to the same source row. Before enrichment, `hubspot_raw`
contributes the original domain. After enrichment corrects it,
`hubspot_enriched` contributes the new domain (once the filter passes). The
merge still happens — but it happens only after enrichment is confirmed, not
during a speculative intermediate state.

**§5 — Premature action:** The filter on `hubspot_enriched` delays enriched
fields until they're confirmed. Downstream systems that need complete data
can add their own `reverse_filter: "industry IS NOT NULL"` or similar.

**§3 — Dangling FK:** Not solved by the split. This is an ETL-layer concern:
process entity inserts topologically and re-queue records with unresolved
FK translations (see [FK-REFERENCES-PLAN](FK-REFERENCES-PLAN.md)).

**§4 — Provider conflicts:** With `last_modified` on enriched fields, the
latest enrichment timestamp always wins, regardless of which provider wrote
it. This is the correct default for enrichment (newer data is more accurate).
If providers are not interchangeable, model them as separate sources with
explicit priorities.

---

## Alternative: single mapping with `last_modified`

If splitting is too heavy, use a single mapping with `last_modified` on
enrichment-owned fields:

```yaml
mappings:
  - name: hubspot
    source: { dataset: hubspot }
    target: company
    sync: true
    fields:
      - source: domain
        target: domain
      - source: name
        target: name
        priority: 1
      - source: industry
        target: industry
        last_modified: updated_at
      - source: employee_count
        target: employee_count
        last_modified: updated_at
```

This handles §4 (latest timestamp wins). For ungrouped fields, §1 is a
non-issue — NULLs never win with `last_modified` either. For grouped fields,
this approach does not gate on enrichment completeness (§5), so a partially-
enriched row could win a group CTE. It also cannot distinguish user-entered
from enrichment-owned fields for priority purposes.

**Trade-off:** Simpler mapping, less control. Acceptable when enrichment is
fast (< 1 ETL cycle) and downstream systems tolerate partial data.

---

## ETL-layer requirements

These apply regardless of mapping pattern:

1. **Topological insert ordering.** When enrichment populates FK references,
   the ETL must ensure the parent entity is written before children that
   reference it. Re-queue records with unresolved FKs rather than writing
   NULL.

2. **Merge handling.** When the identity graph merges clusters (§2), the
   delta produces deletes for "loser" records. The ETL must handle these
   gracefully: soft-delete the loser record, re-parent its children, and
   avoid re-inserting it.

3. **Enrichment SLA awareness.** If enrichment typically completes within
   2 minutes but the ETL runs every 15 minutes, most companies arrive fully
   enriched on the first cycle. If enrichment can take hours, the `filter`
   approach (split-mapping) is strongly preferred to avoid premature syncing.

4. **Webhook-triggered runs.** For HubSpot specifically, trigger an ETL run
   on the enrichment-complete webhook to minimize the window between
   enrichment and sync, rather than relying on fixed-interval polling.
