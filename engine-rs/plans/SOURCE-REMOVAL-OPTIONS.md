# Source removal and cluster splits

**Status:** Design

> What happens when a mapping is removed from a target — can this cause
> clusters to split because a transitive link disappears?

## The Problem

Consider three systems connected transitively through identity fields:

```
CRM ←(email)→ Billing ←(phone)→ ERP
```

CRM and ERP are in the same cluster only because Billing bridges them. If
Billing is removed from the mapping:

```
CRM          ERP       ← no connection, cluster splits
```

One entity has become two. The delta view may now produce **inserts** where
before there were **updates**. The ETL could create duplicate records in target
systems.

## How Each Edge Type Is Affected

### Identity-field edges — highest risk

The connection was implicit and structural. Nobody explicitly declared "CRM and
ERP are the same entity" — they were linked only through Billing's shared
values. Removing Billing removes all its rows from the forward view, which
removes every edge that passed through it.

### Link edges (`links` with or without `link_key`)

A linking table `xref(entity_id, crm_id, billing_id, erp_id)` generates edges
CRM↔Billing and Billing↔ERP. Remove the Billing mapping and those edges
vanish. BUT — if the xref still has `crm_id` and `erp_id` on the same row,
the engine can still produce a CRM↔ERP edge from that row directly. So link
tables are **more resilient** than identity-field edges because the xref table
itself carries the transitivity.

### Cluster-ID edges (`cluster_members`, `cluster_field`)

If the ETL had previously written `_cluster_id` feedback for rows in this
cluster, the feedback values create cluster-ID edges that survive independently
of the removed source. Two Forward-view rows sharing the same `_cluster_id`
form an edge even if the bridge source is gone.

This is because feedback creates **edges**, not assignments — the `_cluster_id`
value just needs to match at least one other forward-view row's value to
maintain connectivity.

**Feedback-driven clusters are the most resilient to source removal.**

## Architectural Constraint

The engine is stateless. It must not implicitly persist cluster memory across
configuration changes. Any continuity across source-removal events must come
from explicit data inputs (`links`, `cluster_members`, `cluster_field`), not
hidden engine snapshots.

## Options

### 1. The split is semantically correct — do nothing

If you remove a source, you're saying "this source doesn't participate anymore."
The engine has no evidence CRM and ERP are the same entity. The split is correct
given the remaining data.

**Downside**: the user may not realize the transitive impact. They thought they
were removing Billing, not splitting CRM↔ERP.

### 2. Validation warning at mapping parse time

The engine detects when removing a source would break transitive chains and
warns:

```
WARNING: Removing mapping 'billing' from target 'customer' may split
clusters that are currently connected only through 'billing'.
Consider adding direct links between 'crm' and 'erp' if they should
remain connected.
```

This requires comparing graph connectivity with vs. without the source, which
the engine can already do (connected components). Purely informational — the
engine still does what the mapping says.

### 3. Decommission mode (`active: false`)

Add a flag on a mapping:

```yaml
mappings:
  billing:
    source: billing
    target: customer
    active: false      # keeps identity edges, drops field contributions
    fields: [...]
```

The source still contributes to identity resolution (its rows appear in the
identity view, creating edges), but it doesn't contribute fields to resolution
and doesn't appear in delta output. It's a "ghost" mapping — preserving cluster
structure while being operationally inert.

**Pro**: clean gradual decommissioning. **Con**: the source table must still
exist and be queryable. You're not really "removing" it.

### 4. Bridge-link generation (one-time migration)

Before removing a source, run a tool that:
1. Identifies all clusters that depend on transitivity through the source.
2. Generates explicit pairwise links between the remaining sources.
3. Outputs a migration artifact the user adds to their mapping.

```sql
-- Generated: bridge links for removing 'billing'
CREATE TABLE bridge_crm_erp AS
SELECT crm._src_id AS crm_id, erp._src_id AS erp_id
FROM _id_customer crm
JOIN _id_customer erp
  ON crm._entity_id_resolved = erp._entity_id_resolved
WHERE crm._mapping = 'crm' AND erp._mapping = 'erp';
```

Then add as a linkage-only mapping:

```yaml
mappings:
  bridge_crm_erp:
    source: bridge_crm_erp
    target: customer
    links:
      - { field: crm_id, references: crm }
      - { field: erp_id, references: erp }
```

**Pro**: explicit, auditable, truly stateless. **Con**: requires a manual step.

### 5. Existing feedback as natural safety net

If `cluster_members` or `cluster_field` feedback was already active, the
persisted `_cluster_id` values already bridge surviving mappings. No explicit
migration needed — the feedback tables contain the transitive closure.

This only works if feedback was in place. It's a benefit to document, not a
guarantee to rely on.

### 6. Engine-managed cluster snapshots

The engine could persist a `_cluster_snapshot` table and reuse it to preserve
continuity when sources are removed.

**Rejected**: breaks stateless architecture. Introduces hidden lifecycle
complexity and governance burden.

## Recommendation

1. **Validation warning** (Option 2) as the default safety mechanism — always
   emit diagnostics about transitive dependency risk.
2. **Bridge-link generation** (Option 4) as the explicit decommission workflow
   for production systems requiring continuity.
3. **Document the feedback safety net** (Option 5) as a resilience benefit of
   running with `cluster_members` / `cluster_field`.
4. **Do not implement engine snapshots** (Option 6) unless the product
   direction intentionally moves from stateless to stateful.

The key principle: **the engine shouldn't silently preserve state from removed
sources** (that violates statelessness), but it should **make it easy for the
user to preserve it explicitly** when they choose to.

## Decommission Workflow

1. **Detect risk**: run validation to identify mappings that act as transitive
   bridges for a target.
2. **Preserve (if needed)**: generate bridge links among remaining mappings for
   clusters that would split. Or rely on existing feedback coverage.
3. **Stage**: run one cycle with both old source and new bridge links. Confirm
   cluster count and insert behavior stay within expected bounds.
4. **Remove**: apply config change only after bridge evidence is in place.
5. **Monitor**: track insert spikes and duplicate candidates for at least one
   cycle window post-removal.

## Relationship to `_cluster_id` Stability

The ORIGIN-PLAN already documents that derived `_cluster_id` (feedback values)
creates edges, not assignments. This means:

- Feedback-driven clusters survive source removal naturally.
- Feedback-driven clusters survive cluster merges and splits.
- The only scenario where removal causes a problem is when the **only** path
  between two sources was through identity-field edges on the removed source,
  AND no feedback had been propagated.
