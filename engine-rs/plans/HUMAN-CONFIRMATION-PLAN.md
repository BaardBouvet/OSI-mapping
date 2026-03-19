# Human confirmation for reverse ETL

**Status:** Design

Analysis of what role the mapping tool should play in human-in-the-loop approval for reverse ETL operations.

## Problem

Reverse ETL writes resolved data back to source systems. Some writes are high-risk: overwriting a CRM contact's email, deleting a record from an ERP, writing a financial value back to a billing system. Organizations need human confirmation gates — but the question is where those gates should live and how granular they need to be.

Today the engine produces delta views with `_action` (insert / update / delete / noop) and the ETL layer blindly executes them. There is no mechanism to pause, review, or selectively approve individual operations before they reach the target system.

## Design space

There are three layers where confirmation could live:

| Layer | Role | Example |
|-------|------|---------|
| **Mapping schema** | Declare *what* needs confirmation | "Deletes to Salesforce require approval" |
| **Engine output** | Annotate deltas with confirmation metadata | `_requires_confirmation: true` column in delta view |
| **ETL / UI** | Present pending changes, collect approval, execute | Dashboard with approve/reject buttons |

The mapping tool's natural role is the first two: **declare policy** and **annotate output**. The ETL/UI layer handles execution and user interaction — that boundary is consistent with the asymmetry principle from [ASYMMETRY-ANALYSIS.md](ASYMMETRY-ANALYSIS.md): semantic control belongs in the mapping, mechanical execution belongs in the ETL.

## What the mapping schema should declare

### 1. Mapping-level gates

The coarsest control: gate all reverse operations for an entire mapping (i.e. one source system).

```yaml
mappings:
  - name: salesforce_contacts
    source: salesforce
    target: customer
    confirm: true                    # all reverse operations need approval
```

This is the simplest useful gate. An ETL tool reading the compiled output sees the `confirm` flag and knows to route all deltas for this mapping through an approval queue instead of executing directly.

### 2. Action-level gates

Different risk levels per action type. Inserts into a clean system are low-risk; deletes are high-risk.

```yaml
mappings:
  - name: salesforce_contacts
    source: salesforce
    target: customer
    confirm:
      insert: false
      update: true
      delete: true
```

This covers the common case where organizations trust new record creation but want to review modifications and deletions.

### 3. Field-level gates

Some fields are more sensitive than others. Updating a display name is low-risk; updating an email address or financial value is high-risk.

```yaml
mappings:
  - name: salesforce_contacts
    source: salesforce
    target: customer
    fields:
      - source: name
        target: name
      - source: email
        target: email
        confirm: true              # changes to this field need approval
      - source: revenue
        target: annual_revenue
        confirm: true
```

The engine can compute which fields actually changed (it already does this for noop detection) and set `_requires_confirmation` only when a confirmed field is in the changeset.

### 4. Expression-based gates (custom filters)

For complex business rules: confirm when a value crosses a threshold, when the source is untrusted, when the entity belongs to a specific segment.

```yaml
mappings:
  - name: salesforce_contacts
    source: salesforce
    target: customer
    confirm_when: "annual_revenue > 1000000 OR is_key_account"
```

This is a SQL expression evaluated against the resolved row. When true, the delta row is flagged for confirmation. This covers the "per custom filter" aspect of the TODO — arbitrary business logic can gate confirmation.

### 5. Pattern-based allow/deny lists

Whitelist and blacklist patterns for bulk control across many mappings or fields.

```yaml
confirmation:
  rules:
    - match:
        system: salesforce         # per system
        action: delete             # per action type
      confirm: true

    - match:
        target: customer           # per target entity
        fields: [email, phone]     # per field
      confirm: true

    - match:
        system: internal_*         # glob pattern — deny list
      confirm: false               # trusted systems skip confirmation

    - match:
        expression: "priority = 'critical'"
      confirm: true
```

This is the most powerful option. A top-level `confirmation:` section with pattern-matching rules lets organizations express policies like:

- "All deletes to any external system require confirmation"
- "All changes to PII fields require confirmation"
- "All operations on internal staging systems are auto-approved"
- "All changes to entities marked critical require confirmation"

Rules evaluate in order; first match wins. This gives fine-grained control without cluttering individual mapping definitions.

## Engine output

Regardless of which declaration style is used, the engine's job is to compile the policy into a column on the delta view:

```sql
-- In _delta_{mapping} view
SELECT
    _cluster_id,
    _action,
    name,
    email,
    annual_revenue,
    -- Confirmation flag computed from schema policy
    CASE
        WHEN _action = 'delete' THEN true
        WHEN _action = 'update' AND (
            email IS DISTINCT FROM _written->>'email'
            OR annual_revenue::text IS DISTINCT FROM _written->>'annual_revenue'
        ) THEN true
        ELSE false
    END AS _requires_confirmation
FROM ...
```

The ETL layer then branches:

```
delta row
  ├─ _requires_confirmation = false → execute immediately
  └─ _requires_confirmation = true  → write to pending_changes queue
                                       → UI presents for review
                                       → on approve: execute + update written_state
                                       → on reject: mark as rejected
```

## What the mapping tool should NOT do

The mapping tool should not:

- **Build the approval UI** — that is an application concern, not a schema concern.
- **Manage approval state** — pending/approved/rejected status is ETL-layer state, like `written_state`.
- **Enforce timeouts or escalation** — SLA policies belong in the workflow layer.
- **Handle authentication or authorization** — who can approve is an identity concern.
- **Retry rejected operations** — retry policy is ETL-layer logic.

The mapping tool's boundary is: declare policy, annotate output. Everything downstream is the ETL's problem.

## Interaction with existing features

| Feature | Interaction |
|---------|-------------|
| `reverse_filter` | Evaluated before confirmation. If `reverse_filter` excludes a row, it never reaches the confirmation gate — it's already a delete or excluded. |
| `direction: forward_only` | No reverse delta generated, so no confirmation needed. |
| `written_state` | Confirmation integrates with written_state: approved rows update `_written_{mapping}`, rejected rows don't. |
| `on_hard_delete` | Hard-delete policy (`suppress` / `delete` / `propagate`) is evaluated first. If `suppress`, no delta row exists to confirm. If `delete` or `propagate`, the delete action can still require confirmation. |
| `normalize` | Normalization affects noop detection. A normalized match means no change, so no confirmation needed — correct behavior. |

## Recommended approach

Implement in phases:

**Phase 1 — Mapping-level flag.** Add `confirm: true` on mappings. Engine adds `_requires_confirmation` column to delta views (always true when flag is set). ETL tools can immediately use this to route deltas. Zero complexity, immediately useful.

**Phase 2 — Action-level granularity.** Extend `confirm` to accept `{ insert, update, delete }` booleans. Engine evaluates `_action` to compute the flag. Covers the 80% case.

**Phase 3 — Field-level and expression gates.** Add `confirm` on field mappings and `confirm_when` on mappings. Engine computes field-level change detection (already exists for noop) and evaluates the expression. Covers the remaining 20%.

**Phase 4 — Top-level pattern rules.** Add `confirmation:` section with match rules. This is a convenience layer over phases 1-3 — the engine compiles patterns into per-mapping/per-field flags. Only needed when organizations have many mappings and want centralized policy.

## Open questions

1. **Should rejected operations be retried on next cycle?** If the resolved value hasn't changed, "the same" delta will reappear next run. The ETL layer needs a rejection record to suppress re-presentation. This is ETL-layer state — but the engine might need a `_rejected_{mapping}` input table analogous to `_written_{mapping}`.

2. **Partial approval for multi-field updates.** If an update changes both `name` (auto-approved) and `email` (needs confirmation), should the engine split into two operations? This adds significant complexity. Simpler: if any field in the changeset requires confirmation, the entire row is held.

3. **Confirmation for inserts vs updates to new systems.** When a mapping targets a system that has never seen this entity, the first write is an insert. If only updates/deletes require confirmation, the initial insert flows through — but it's effectively "writing data for the first time to an external system," which some orgs may want to gate differently.

4. **Audit trail.** Should the mapping schema declare that confirmation decisions are logged? Or is audit logging always an ETL-layer concern? Leaning toward the latter — the mapping declares *what*, not *how* or *whether to log*.
