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

## Machine learning and anomaly detection

The rule-based gates above (phases 1–4) require humans to anticipate which operations are risky. ML flips this: learn what "normal" looks like from historical data, flag deviations automatically. Anomaly detection is a natural fit because the engine already computes explicit changesets — every delta row is a structured record of what changed, by how much, from which source.

### Where ML fits in the architecture

The same asymmetry principle applies: the mapping tool declares policy and annotates output; ML models run in an adjacent layer that feeds signals back.

```
Engine (delta views)
    │
    ├─ _requires_confirmation (rule-based, from mapping schema)
    │
    ▼
ML scoring layer (external)
    │
    ├─ _anomaly_score (0.0–1.0)
    ├─ _anomaly_reasons (text[])
    │
    ▼
Confirmation gate (ETL / UI)
    │
    ├─ Rule says confirm? → queue
    ├─ Anomaly score > threshold? → queue
    └─ Otherwise → auto-approve
```

The ML layer sits between engine output and the confirmation gate. It reads delta rows, scores them, and the ETL layer combines rule-based and ML-based signals to decide whether to queue for review.

### What the mapping schema could declare

The mapping tool doesn't run ML models — but it can declare that anomaly detection applies and configure its parameters:

```yaml
mappings:
  - name: salesforce_contacts
    source: salesforce
    target: customer
    anomaly_detection:
      enabled: true
      threshold: 0.8             # score above this → flag for confirmation
      features:                  # which fields to monitor
        - email                  # domain change, format change
        - annual_revenue         # magnitude change
      baseline: written_state    # learn "normal" from historical written values
```

Or at the top level for centralized policy:

```yaml
confirmation:
  anomaly_detection:
    enabled: true
    threshold: 0.8
    exclude_systems: [internal_*]    # don't score internal staging
  rules:
    - match:
        anomaly_score: "> 0.9"       # very high anomaly → always confirm
      confirm: true
    - match:
        anomaly_score: "> 0.7"
        action: delete               # moderate anomaly + delete → confirm
      confirm: true
```

The engine compiles this into metadata that downstream consumers (the ML scorer and the ETL) interpret. The engine itself does not run inference.

### Anomaly detection models that fit this architecture

The delta views provide structured, tabular changesets. These properties map well to specific ML approaches:

**1. Statistical anomaly detection (no training required)**

The simplest approach. Compute statistics over `_written_{mapping}` history and flag outliers.

| Signal | Method | Example |
|--------|--------|---------|
| Value magnitude change | Z-score on `new_value - written_value` | Revenue jumps from 50k to 5M (z > 3) |
| Batch size spike | Count of non-noop deltas per cycle | 10× more updates than usual |
| Delete ratio | `count(delete) / count(*)` per cycle | Normally 0.1%, suddenly 15% |
| Field change frequency | How often a field changes for an entity | Email changed 3× in 24 hours |
| Null introduction rate | `count(NULL)` where field was non-NULL | Mass nullification of phone numbers |

These can be expressed as SQL window functions over the `_written_{mapping}` history table. The engine could emit a `_anomaly_stats_{mapping}` view that computes these signals — pure SQL, no external dependencies.

```sql
-- Engine-generated anomaly stats view
CREATE VIEW _anomaly_stats_salesforce_contacts AS
SELECT
    _cluster_id,
    -- Revenue magnitude change
    ABS(annual_revenue - (_written->>'annual_revenue')::numeric)
        / NULLIF(STDDEV_POP((_written->>'annual_revenue')::numeric)
          OVER (), 0) AS revenue_zscore,
    -- Email domain changed
    CASE WHEN SPLIT_PART(email, '@', 2) !=
              SPLIT_PART(_written->>'email', '@', 2)
         THEN 1.0 ELSE 0.0 END AS email_domain_changed,
    -- Composite score
    ...
FROM _delta_salesforce_contacts
LEFT JOIN _written_salesforce_contacts USING (_cluster_id);
```

This is the most interesting option for the mapping tool because it stays within the SQL-generation boundary. The engine already has all the inputs: current resolved values, previous written values, and the full delta. Statistical scoring is a view over these — no external ML runtime needed.

**2. Change-pattern models (lightweight training)**

Learn per-entity or per-field change patterns from `_written_{mapping}` history.

| Model | Input | Detects |
|-------|-------|---------|
| Isolation Forest | Feature vector per delta row (field changes, magnitudes, timing) | Rows that don't look like typical changes |
| Autoencoder | Same feature vector | Reconstruction error = anomaly score |
| DBSCAN clustering | Change vectors over time | Clusters of "normal" change patterns; outliers = anomalies |

These require a training step on historical `_written_{mapping}` snapshots. The ML layer trains offline, deploys a scorer, and the scorer reads delta views in real time. The mapping tool's role: declare which fields are features, point to the baseline data source.

**3. Batch-level anomaly detection**

Instead of scoring individual rows, score the entire delta batch. This catches systemic issues:

- A mapping that normally produces 50 updates suddenly produces 5,000 → likely a source data issue, not 5,000 legitimate changes.
- Delete ratio spikes from 0.1% to 40% → likely a source system outage reporting everything as deleted.
- All values for a field become NULL → likely a schema change upstream.

Batch-level detection is the highest-value, lowest-cost ML capability. It prevents catastrophic bulk operations — the kind that are hardest to reverse and most dangerous to auto-approve.

```yaml
mappings:
  - name: salesforce_contacts
    source: salesforce
    target: customer
    anomaly_detection:
      batch_limits:
        max_deletes_pct: 5           # halt if >5% of batch is deletes
        max_updates_pct: 50          # halt if >50% of batch is updates
        max_null_introduction_pct: 2 # halt if >2% of fields go NULL
```

These thresholds are simple enough to declare in the mapping schema and compile into SQL aggregations. The ETL checks thresholds before executing any row in the batch.

### What the mapping tool should generate

For ML integration, the engine could produce additional views:

| View | Purpose |
|------|---------|
| `_anomaly_stats_{mapping}` | Per-row statistical signals (z-scores, change flags) computed in SQL |
| `_batch_stats_{mapping}` | Aggregate batch statistics (counts, ratios, distributions) |
| `_feature_vector_{mapping}` | Structured feature vector for external ML model input |

The first two are pure SQL — the engine generates them like any other view. The third is a convenience: a flat, typed row that an external scorer can consume without parsing delta semantics.

### Feedback loop

ML anomaly detection improves with feedback. When a human approves or rejects a flagged operation, that decision is training data:

```
Delta row (anomaly_score = 0.85)
    → Queued for review
    → Human approves
    → Label: "not actually anomalous"
    → Model retrains → threshold adjusts
```

The mapping tool doesn't manage feedback — but it can declare that feedback should be collected:

```yaml
anomaly_detection:
  enabled: true
  feedback: true    # ETL should persist approval/rejection for model retraining
```

This is metadata for the ETL layer. The engine includes it in compiled output; the ETL uses it to decide whether to store labeled examples.

### Recommended approach for ML

Extend the phased plan:

**Phase 5 — SQL-based statistical anomaly detection.** The engine generates `_anomaly_stats_{mapping}` and `_batch_stats_{mapping}` views using SQL window functions over `_written_{mapping}` history. No external dependencies. Thresholds declared in mapping schema. Immediately useful for catching bulk data issues and magnitude outliers.

**Phase 6 — External ML scorer integration.** The engine generates `_feature_vector_{mapping}` views. An external ML service reads these, computes `_anomaly_score`, and writes scores to a table the ETL reads alongside `_requires_confirmation`. The mapping schema declares feature fields and baseline source. Requires ML infrastructure but the engine's role is just view generation.

**Phase 7 — Feedback-driven model improvement.** The ETL stores approval/rejection labels. An offline training pipeline reads labels + feature vectors and retrains the scorer. Entirely outside the mapping tool's boundary — but `feedback: true` in the schema signals the ETL to collect the data.

Phase 5 is the sweet spot: useful anomaly detection with zero external dependencies, staying within the engine's SQL-generation boundary. Phases 6–7 are valuable but require ML infrastructure that most teams won't have on day one.

## Open questions

1. **Should rejected operations be retried on next cycle?** If the resolved value hasn't changed, "the same" delta will reappear next run. The ETL layer needs a rejection record to suppress re-presentation. This is ETL-layer state — but the engine might need a `_rejected_{mapping}` input table analogous to `_written_{mapping}`.

2. **Partial approval for multi-field updates.** If an update changes both `name` (auto-approved) and `email` (needs confirmation), should the engine split into two operations? This adds significant complexity. Simpler: if any field in the changeset requires confirmation, the entire row is held.

3. **Confirmation for inserts vs updates to new systems.** When a mapping targets a system that has never seen this entity, the first write is an insert. If only updates/deletes require confirmation, the initial insert flows through — but it's effectively "writing data for the first time to an external system," which some orgs may want to gate differently.

4. **Audit trail.** Should the mapping schema declare that confirmation decisions are logged? Or is audit logging always an ETL-layer concern? Leaning toward the latter — the mapping declares *what*, not *how* or *whether to log*.

5. **Anomaly threshold tuning.** Who sets the anomaly score threshold — the mapping author, or the ML system? If the mapping schema declares `threshold: 0.8`, it becomes a static policy. If the ML system determines the threshold dynamically via precision/recall tuning, the schema value is just a default. Leaning toward schema-as-default, ML-as-override.

6. **Cold start for statistical baselines.** `_anomaly_stats` views need historical `_written_{mapping}` data to compute meaningful z-scores. On day one there is no history. The engine could either skip anomaly scoring when history is insufficient (< N cycles) or use conservative fixed thresholds until enough data accumulates.

7. **Anomaly detection for identity resolution.** Cluster membership changes (entity merges/splits) are a distinct anomaly class. A customer suddenly merging with 50 other records is suspicious. Should the engine compute cluster-level anomaly signals alongside field-level ones?
