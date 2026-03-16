# Origin and insert feedback

**Status:** Done

> **Abstract**: Defines how the engine tracks entity identity across sources and
> how ETL pipelines feed back generated IDs to prevent duplicate inserts.
> Introduces `links` (with optional `link_key`) on mappings for external
> identity edges, `cluster_members` for ETL feedback via a separate table,
> `cluster_field` for feedback stored directly in the source, `_cluster_id`
> as a stable entity handle on delta insert rows, and two operating modes —
> IVM-safe (with `link_key`, `cluster_members`, or `cluster_field`) and
> batch-safe (without).

## Problem

When the delta view produces an **insert** — a resolved entity that has no
corresponding row in a particular source — the consumer ETL pipeline needs to:

1. Write the new row to the target system.
2. Capture the generated ID from the target system.
3. Link that generated ID back to the entity so the next sync run doesn't
   produce a duplicate insert.

## Responsibility Split

| Concern | Owner | Stateful? |
|---------|-------|-----------|
| Identify entities, compute inserts | **Engine** (views) | No — pure SQL |
| Track which inserts have been performed, link generated IDs back | **ETL** (runtime) | Yes — persistent state |

The engine is stateless. It emits `_cluster_id` on insert rows so the ETL has a
handle for the entity. Everything else — recording origins, managing clusters,
feeding back generated IDs — is ETL runtime state.

## Design Decisions

### Links on mappings (not references on sources)

Identity edges from linking tables are declared as `links` on the mapping, not
as `references` on the source. Rationale:

- **Scoped to target**: a link is an instruction about entity resolution for a
  specific target. A reference on a source is a structural fact that doesn't
  know which target it applies to.
- **Unambiguous**: if the same source maps to multiple targets, `links` on each
  mapping make it clear which target gets the edges.
- **Familiar pattern**: a mapping is the unit of "how source data participates
  in a target." Links are part of that participation.

A mapping with `links` and no `fields` is a "linkage-only" mapping — it
contributes identity edges but no business data.

### IVM-safe architecture (push cluster join into forward view)

With Incremental View Maintenance (IVM), the forward view and identity view
update independently. If a new source row arrives and its cluster link is
written as a separate transaction, there's a window where the identity view
sees the source row but not yet the link — producing a phantom singleton
entity (and a phantom insert in the delta).

**Solution**: LEFT JOIN cluster membership data directly in the forward view,
before the identity layer. A source row and its cluster membership arrive as
a single composite row. IVM sees both facts atomically.

The forward view becomes:

```sql
SELECT
  s._row_id AS _src_id,
  '{mapping}' AS _mapping,
  COALESCE(cm._cluster_id, md5('{mapping}' || ':' || s._row_id::text)) AS _cluster_id,
  ...
FROM {source_table} s
LEFT JOIN _cluster_members_{target} cm
  ON cm._mapping = '{mapping}' AND cm._src_id = s._row_id::text
```

`_cluster_members_{target}` is built from:
- Unpivoted `links` rows (when `link_key` is present)
- `cluster_members` tables (one per mapping that declares them)

If neither `links` with `link_key` nor any `cluster_members` are declared for
the target, the LEFT JOIN is omitted entirely — no overhead for simple cases.

**Key insight**: if it works for IVM, it works for batch. Design for IVM.

Note: curated merges (human adds a link to already-existing source rows) do
NOT have the race problem — the source rows are already materialized when the
link arrives. A single write (the link) triggers recomputation. The forward-
view LEFT JOIN handles this correctly too, so one mechanism covers both cases.

### `cluster_members` — ETL feedback (Pattern A)

For ETL-driven feedback where the process only knows `_cluster_id` + the new
record's PK. Declared per-mapping — each mapping gets its own table because
source PKs may differ in type or be composite.

| Field | Default | Description |
|-------|---------|-------------|
| `table` | `_cluster_members_{mapping}` | Table name |
| `cluster_id` | `_cluster_id` | Cluster ID column |
| `source_key` | `_src_id` | Source PK column |

Usage from minimal to fully custom:

```yaml
mappings:
  # Minimal — all defaults
  # → table: _cluster_members_billing, columns: _cluster_id, _src_id
  billing:
    source: billing
    target: customer
    cluster_members: true
    fields:
      - { source: account_name, target: name }

  # Custom table/column names
  legacy:
    source: legacy
    target: customer
    cluster_members:
      table: legacy_feedback
      cluster_id: entity_id
      source_key: record_id
    fields:
      - { source: legacy_name, target: name }
```

The ETL writes 2 columns: `(_cluster_id, _src_id)` — no need to know anything
about other sources. The engine injects `_mapping` as a literal when building
`_cluster_members_{target}`:

```sql
SELECT cm._cluster_id, 'billing' AS _mapping, cm._src_id
FROM _cluster_members_billing cm
UNION ALL
SELECT cm.entity_id, 'legacy' AS _mapping, cm.record_id
FROM legacy_feedback cm
```

Per-mapping tables are the only mode — no shared-table option. Reason:
source PKs differ in type across mappings (integer, UUID, text, etc.) and
can't cleanly share a column. Per-mapping tables also align naturally with
security and data mesh boundaries.

**IVM-safe**: yes — same LEFT JOIN mechanism as `links` with `link_key`.

### `cluster_field` — cluster ID stored in the source

Some target systems support storing custom properties on records. If the ETL
writes `_cluster_id` as a field on the target record, the source table itself
carries the cluster identity on the next run — no separate feedback table
needed.

```yaml
mappings:
  billing:
    source: billing
    target: customer
    cluster_field: entity_cluster_id      # column in billing table
    fields:
      - { source: account_name, target: name }
```

The engine uses `cluster_field` directly in the forward view:

```sql
SELECT
  s._row_id AS _src_id,
  'billing' AS _mapping,
  COALESCE(s.entity_cluster_id, md5('billing' || ':' || s._row_id::text)) AS _cluster_id,
  ...
FROM billing s
```

If `entity_cluster_id` is populated, the row joins its cluster directly. If
NULL (pre-existing rows not written by the ETL), it falls back to the default
singleton cluster ID. No LEFT JOIN needed — the cluster ID is part of the
source row.

**IVM-safe**: yes — the source row carries its own cluster identity atomically.

**Trade-offs vs `cluster_members`**:

| | `cluster_field` | `cluster_members` |
|---|---|---|
| **Separate table** | No — data in source | Yes — per-mapping table |
| **ETL complexity** | Include `_cluster_id` when writing the record | Write to feedback table after writing |
| **Requirement** | Target system must support custom fields | Works with any target system |
| **Source purity** | Adds engine metadata to source data | Metadata stays in engine tables |

Both produce the same result: a forward-view row carrying `_cluster_id`.
`cluster_field` is simpler when the target system supports it; `cluster_members`
is the universal fallback. A mapping should declare one or the other, not both.

### Flat membership tables from external systems

External systems (MDM, entity resolution) that produce a global flat
`(cluster_id, source, source_pk)` membership table don't fit `cluster_members`
(which is per-mapping). The mapping author pivots the flat table into columnar
form and maps it with `links` + `link_key`:

```sql
CREATE VIEW membership_xref AS
SELECT
  cluster_id,
  MAX(source_key) FILTER (WHERE mapping = 'crm') AS crm_id,
  MAX(source_key) FILTER (WHERE mapping = 'billing') AS billing_id
FROM flat_membership
GROUP BY cluster_id;
```

Four mechanisms, clear roles:
- **`links`** — external systems that know about multiple sources (MDM, curation, record linkage)
- **`cluster_members`** — ETL feedback via separate table: just `_cluster_id` + one new PK
- **`cluster_field`** — ETL feedback via source column: `_cluster_id` stored on the target record itself
- **Identity fields** — shared natural keys across sources

### Three identity edge types

The identity view has three sources of edges, all feeding into the same
connected-components algorithm:

1. **Identity-field edges**: two forward rows with the same non-NULL value
   for an identity field → edge.
2. **Cluster-ID edges**: two forward rows with the same `_cluster_id`
   (from unpivoted `links` with `link_key`) → edge. These appear
   naturally because the forward view carries `_cluster_id` from the
   LEFT JOIN.
3. **Link edges**: explicit pairwise edges from `links` declarations —
   a linking-table row that references two sources creates an edge
   between them. Present in **both** `link_key` and no-`link_key` modes.
   With `link_key`, these are redundant with cluster-ID edges but harmless;
   without `link_key`, these are the **only** mechanism connecting linked rows.

Edge types compose freely. A target can use any combination.

### Supported curation patterns

Four common curation/linking patterns exist in practice:

| Pattern | Description | Supported via |
|---------|-------------|---------------|
| **A — Flat membership** | `(cluster_id, source, source_pk)` | `cluster_members` (per-mapping) or pivot to columnar → `links` + `link_key` |
| **B — Pairwise decisions** | `(source_a_pk, source_b_pk, decision)` | `links` without `link_key` (batch-safe) |
| **C — Columnar xref** | `(match_id, crm_id, billing_id, ...)` | `links` + `link_key` |
| **D — Golden record** | Curated master table with overrides | Out of scope — curated master is a source like any other |

Patterns A, B, and C are all first-class. Pattern A uses `cluster_members`
(per-mapping ETL feedback) or a pivoted view with `links` + `link_key`.
Pattern C uses `links` + `link_key` (IVM-safe). Pattern B uses `links` without
`link_key` (batch-safe). Pattern D needs no special support.

### Two modes for `links`: IVM-safe vs batch

The engine supports `links` in two modes, depending on whether `link_key` is
present:

| | `links` **with** `link_key` | `links` **without** `link_key` |
|---|---|---|
| **Cluster ID source** | Pre-computed — the `link_key` column value | Derived — engine computes connected components in the identity layer |
| **Forward-view behaviour** | LEFT JOIN `_cluster_members_{target}` carries `_cluster_id` into the forward view | No LEFT JOIN — forward view stays simple |
| **IVM-safe?** | Yes — source row + cluster membership arrive atomically | **No** — link arriving after source row causes a stale identity view until next full refresh |
| **Best for** | ETL feedback, MDM platforms, entity resolution services | Record linkage tools (Splink, Dedupe), manual curation UIs, probabilistic matchers |
| **When to choose** | Continuous/IVM pipelines, or when the source already provides cluster IDs | Batch/scheduled pipelines consuming pairwise decisions |

#### Why `link_key` enables IVM safety

Three properties of `link_key` make the IVM-safe path possible:

1. **Cluster ID in the forward view.** The LEFT JOIN needs a `_cluster_id`
   column to join on. `link_key` provides it directly — no graph computation
   needed before the forward view.
2. **No circular dependency.** Without `link_key`, the forward view would
   need cluster IDs → cluster IDs come from connected components over links →
   links reference forward-view rows. `link_key` breaks the cycle by providing
   cluster IDs from outside the view DAG.
3. **No recursive CTE in the hot path.** Connected components via recursive
   CTE is O(V + E) and non-incremental (a single new edge can merge two
   large clusters). Some IVM implementations may support incremental
   recursive CTEs, but this is not universal — `link_key` avoids the
   dependency entirely.

#### How `links` without `link_key` works

The engine generates the same pairwise edge SQL it already generates for
`links` with `link_key`, but instead of unpivoting into `_cluster_members`,
it feeds those edges directly into the identity layer's connected-components
algorithm — alongside identity-field edges.

```sql
-- Engine-generated: pairwise link edges in the identity layer
_link_edges AS (
  SELECT
    a._entity_id AS entity_a,
    b._entity_id AS entity_b
  FROM linking_table lt
  JOIN _id_numbered a
    ON a._mapping = 'crm' AND a._src_id = lt.crm_id::text
  JOIN _id_numbered b
    ON b._mapping = 'billing' AND b._src_id = lt.billing_id::text
  WHERE lt.crm_id IS NOT NULL AND lt.billing_id IS NOT NULL
)
```

These edges are UNIONed with identity-field edges before the recursive
connected-components CTE runs. The result is `_entity_id_resolved` — which
becomes `_cluster_id` on delta insert rows, same as the `link_key` path.

**Trade-off**: This path is **not IVM-safe**. If a link row is inserted
after the source rows are already materialised, IVM may not recompute the
recursive CTE (depends on the IVM implementation — some support incremental
recursive CTEs, many don't). The identity view may stay stale until the next
full refresh. For batch/scheduled pipelines this is fine — the entire view
chain is recomputed each run.

#### Insert feedback with `links` without `link_key`

The batch-safe path produces `_cluster_id` on delta insert rows — the
connected-components result works the same as the `link_key` path. But `links`
without `link_key` only provides **identity resolution** (connecting existing
rows via pairwise edges). It does NOT provide **insert feedback**.

If a target uses `links` without `link_key` and has insert-producing mappings,
those mappings must declare their own feedback mechanism — either
`cluster_members` or `cluster_field`:

```yaml
mappings:
  splink_matches:
    source: splink_output
    target: customer
    links:                              # batch-safe — no link_key
      - { field: crm_id, references: crm }
      - { field: billing_id, references: billing }

  billing:
    source: billing
    target: customer
    cluster_members: true               # OR: cluster_field: _cluster_id
    fields:
      - { source: account_name, target: name }
```

The `links` mapping provides pairwise identity edges. The data mapping's
feedback mechanism (`cluster_members` or `cluster_field`) provides the loop.
On the next run, the new row gets the same `_cluster_id` as the original
source → the insert disappears from the delta.

#### Cluster ID stability without `link_key`

Without `link_key`, `_cluster_id` on delta insert rows is derived —
`_entity_id_resolved` = `MIN()` of `md5(_mapping || ':' || _src_id)` across
the connected component. This looks fragile, but the feedback mechanism is
more resilient than it first appears.

**Key insight**: the `_cluster_id` value in feedback (`cluster_members` or
`cluster_field`) creates a **cluster-ID edge**, not an assignment. Two
forward-view rows sharing the same `_cluster_id` value form an edge in the
identity layer. The feedback row doesn't need its `_cluster_id` to equal the
current `_entity_id_resolved` — it just needs to match at least one other
forward-view row's `_cluster_id` to stay connected.

**How this works in practice**:

```
Setup:
  CRM has CRM1 (Alice). Billing has BILL1 (A. Smith).
  Splink pair: CRM1 ↔ BILL1 → links without link_key.
  No row in ERP.

Run 1:
  Identity layer: CRM1 ↔ BILL1 via link edge → one entity.
  Delta for ERP: insert | _cluster_id = MIN(md5('crm:CRM1'), md5('billing:BILL1'))
  Let's say _cluster_id = md5('billing:BILL1') (whichever hashes lower).

  ETL writes to ERP → gets ERP-42.
  ETL writes feedback: (_cluster_id = md5('billing:BILL1'), src_id = 'ERP-42')

Run 2:
  ERP's forward view: ERP-42 gets _cluster_id = md5('billing:BILL1') (from feedback).
  BILL1's forward view: _cluster_id = md5('billing:BILL1') (its default — no feedback).
  Cluster-ID edge: ERP-42 ↔ BILL1 (same _cluster_id value).
  Link edge: CRM1 ↔ BILL1 (from Splink).
  Full component: {CRM1, BILL1, ERP-42}. Insert gone. ✓
```

**Stable across cluster merges**:

```
Splink adds a new pair: CRM1 ↔ CRM2. Clusters merge.

Before: {CRM1, BILL1, ERP-42}
After:  {CRM1, CRM2, BILL1, ERP-42}

ERP-42's feedback still says _cluster_id = md5('billing:BILL1').
BILL1's default is still md5('billing:BILL1').
Cluster-ID edge ERP-42 ↔ BILL1 still exists.
BILL1 ↔ CRM1 via link edge, CRM1 ↔ CRM2 via link edge.
ERP-42 reaches the merged cluster transitively. ✓

The _entity_id_resolved changes (new MIN across larger component),
but that's an output, not an input. Feedback is not affected.
```

**Stable across cluster splits**:

```
Splink removes the CRM1 ↔ BILL1 pair (false match).

Cluster-ID edge: ERP-42 ↔ BILL1 (still exists — from feedback, not links).
No link edge between CRM1 and BILL1 anymore.
Result: {BILL1, ERP-42} stay connected. CRM1 becomes a singleton.

This is correct: ERP-42 was written to ERP for the BILL1 entity.
The link system said CRM1 doesn't belong. ERP-42 stays with BILL1. ✓
```

**Anchor row deletion — not a problem**:

If the original source row that seeded the `_cluster_id` hash is deleted, the
feedback rows that reference it don't break:

- **Multiple feedback rows** (A→B, A→C, then A deleted): B and C both carry
  `_cluster_id = md5('a:A1')` from feedback. They still share the value →
  cluster-ID edge → connected. The anchor is gone but the feedback rows
  anchor each other.
- **Single feedback row** (only ERP-42 has `md5('billing:BILL1')`, BILL1
  deleted): ERP-42 is a singleton. But a singleton is just a record that
  exists in one system — that's the correct state, not an error. The delta
  for ERP shows an update (the record exists). Other systems may show inserts
  for this entity, which is correct behaviour.

**Summary**: derived `_cluster_id` (without `link_key`) is stable across
merges, splits, and anchor deletions. No orphan-detection or cleanup logic
is needed.

**Post-merge duplicates**: when two clusters merge, a target system that
already has a row for each ends up with two rows in the same entity. The
engine correctly reports both as updates with the same resolved values — both
rows get synced. It won't insert a third (the entity already has rows there)
and it won't de-duplicate them (detecting and deleting post-merge duplicates
is outside the engine's scope). The system owner decides whether to merge or
keep both.

**Contrast with `link_key`**: when `link_key` is present, `_cluster_id` is
the external identifier from the linking table (e.g. MDM entity ID). It's
as stable as the external system — if it merges or splits entities, it updates
the xref table and all forward-view rows reflect the change atomically. The
derived path is equally stable for a different reason: the feedback value
creates edges between forward-view rows, and those edges persist regardless
of what happens to the original anchor.
change atomically.

### Cluster IDs in practice — who provides them and who doesn't

| Category | Examples | Provides cluster ID? | Notes |
|----------|----------|---------------------|-------|
| **MDM platforms** | Informatica MDM, Reltio, Semarchy, Tibco EBX, Ataccama | Yes | Core concept — the "golden record ID" or "entity ID" is the cluster identifier. |
| **Entity resolution services** | Senzing, AWS Entity Resolution, Quantexa | Yes | Output is `(entity_id, record_id)` — exactly Pattern A. |
| **ETL feedback tables** | Custom ETL state (our own reference pattern) | Yes | The ETL writes `(_cluster_id, src_pk)` per mapping via `cluster_members`. |
| **Record linkage libraries** | Splink, Dedupe.io, Zingg, RecordLinkage (R) | **No** — pairwise only | Output is scored pairs: `(record_a, record_b, score)`. Use `links` without `link_key`. |
| **Manual curation UIs** | Custom match/reject UIs, crowd-sourcing platforms | **No** — pairwise only | A human says "these two match" — the decision is a pair. Use `links` without `link_key`. |
| **Probabilistic matchers** | Fellegi-Sunter implementations, fastLink | **No** — pairwise only | Output is pairs with match probabilities. Use `links` without `link_key`. |

**IVM-safe path** (`link_key`, `cluster_members`, or `cluster_field` present):
MDM platforms, entity resolution services, and ETL feedback tables.

**Batch-safe path** (no `link_key`): Record linkage tools, manual curation UIs,
and probabilistic matchers. These use `links` without `link_key` — the engine
generates connected-components SQL in the identity layer.

Users who need IVM safety with pairwise-only systems can still pre-compute
cluster IDs themselves (e.g., a materialised view that pivots pairs into
columnar xref) and use `link_key`. But this is no longer required — the
engine handles the common case directly.

### Primary key representation

With one table per link mapping, source PKs no longer need a universal string
representation for cross-mapping compatibility.

**Internal pipeline (`_src_id`)**: the engine still canonicalizes PKs to TEXT
for `_src_id` — the internal scalar that flows through forward → identity →
resolution → reverse → delta. This is necessary because `GROUP BY`, recursive
CTEs, and joins across views need a single consistent type.

- **Single PK**: `column::text` → `'P4'`
- **Composite PK**: `jsonb_build_object('col_a', col_a, 'col_b', col_b)::text`
  with keys sorted alphabetically for determinism.

**External joins (links, cluster_members)**: with per-mapping tables the join
columns can keep native PK types. The `links` mechanism joins a link field
directly against the source's declared PK columns — no text coercion required
at that boundary.

**Summary**: `_src_id` is TEXT internally. PK types are preserved at mapping-
specific table boundaries.

## Two Insert Scenarios

| Scenario | When | Insert mechanism |
|----------|------|------------------|
| **Single natural key** | Exactly 1 identity field on the target | Identity value self-relinks. No feedback needed. |
| **Cluster identity** | 0 or 2+ identity fields, or curated/ETL links | `_cluster_id` + feedback (`cluster_members`, `cluster_field`, or `links` + `link_key`). |

The single-natural-key shortcut only works when there is **exactly one**
identity field on the target. With multiple identity fields, transitive
closure can accumulate multiple distinct values for the same field — see
"Multi-value hazard" below. Everything else uses `_cluster_id` + a feedback
mechanism (`cluster_members`, `cluster_field`, or `links` with `link_key`).

### Multi-value hazard (why multiple identity fields break inserts)

Consider three source rows with two identity fields (`email` and `phone`):

```
System A: {email: "alice@x.com", phone: NULL,       name: "Alice"}
System B: {email: "alice@x.com", phone: "555-1234", name: "A. Smith"}
System C: {email: "bob@x.com",   phone: "555-1234", name: "Bob"}
```

Transitive closure merges all three:
- A ↔ B via email
- B ↔ C via phone

The entity now has two email values: `alice@x.com` and `bob@x.com`. The
resolution view picks one (e.g. `min()` → `alice@x.com`). If we insert this
into System D:

```
Insert into D: {email: "alice@x.com", phone: "555-1234"}
```

This inserted row is a **synthetic composite** — it carries identity values
that never co-occurred in a single source row. On the next run, it becomes a
hub that creates edges via both email AND phone, which may transitively connect
unrelated entities that happen to share one of those values.

With a single identity field, this can't happen: all rows in the cluster share
the same value for that field (that's what made them a cluster). The resolved
value is unambiguous.

**Conclusion**: the "no feedback needed" shortcut only applies when the target
has exactly one identity field. Multiple identity fields require `_cluster_id`
+ linking table feedback, same as the "no natural keys" case.

### Scenario 1 — Single natural key

Both sources share exactly one identity field. No linking table needed.

```yaml
sources:
  hr_west:
    table: hr_west
    primary_key: [emp_no]
  hr_east:
    table: hr_east
    primary_key: [emp_no]

targets:
  employee:
    fields:
      employee_number: identity    # THE single identity field
      name: coalesce
      department: coalesce

mappings:
  hr_west:
    source: hr_west
    target: employee
    fields:
      - { source: emp_no, target: employee_number }
      - { source: full_name, target: name, priority: 1 }
      - { source: dept, target: department, priority: 1 }

  hr_east:
    source: hr_east
    target: employee
    fields:
      - { source: emp_no, target: employee_number }
      - { source: name, target: name, priority: 2 }
      - { source: dept, target: department, priority: 2 }

tests:
  - description: "Shared employee_number — auto-merge, insert carries the key"
    input:
      hr_west:
        - { emp_no: "E42", full_name: "Alice Johnson", dept: "Engineering" }
        - { emp_no: "E43", full_name: "Bob Smith", dept: "Sales" }
      hr_east:
        - { emp_no: "E42", name: "A. Johnson", dept: "Eng" }
    expected:
      hr_west:
        updates:
          - { emp_no: "E42", full_name: "Alice Johnson", dept: "Engineering" }
          - { emp_no: "E43", full_name: "Bob Smith", dept: "Sales" }
      hr_east:
        updates:
          - { emp_no: "E42", name: "Alice Johnson", dept: "Engineering" }
        inserts:
          # The identity field value IS the merge key. ETL writes it.
          # Next run: hr_east has E43 → identity match → done.
          - { employee_number: "E43", name: "Bob Smith", department: "Sales" }
```

**No `_cluster_id` needed.** The ETL writes `employee_number=E43` to hr_east.
Next run, the identity view matches on it automatically.

### Scenario 2 — Cluster identity (curated, ETL-driven, or multi-field)

Applies to:
- No shared identity fields between sources
- Multiple identity fields (transitive closure → multi-value hazard)
- Any case where a human or ETL process manages linkages

All use the same mechanism: a linking table with `links`.

```yaml
sources:
  crm:
    table: crm_contacts
    primary_key: [crm_id]
  billing:
    table: billing_accounts
    primary_key: [billing_id]
  curated_matches:
    table: curated_matches
    primary_key: [match_id]

targets:
  customer:
    fields:
      name: coalesce
      email: coalesce
      # Clean business model. No source PKs.

mappings:
  crm:
    source: crm
    target: customer
    fields:
      - { source: contact_name, target: name, priority: 1 }
      - { source: contact_email, target: email }

  billing:
    source: billing
    target: customer
    fields:
      - { source: account_name, target: name, priority: 2 }
      - { source: billing_email, target: email }

  curated_matches:
    source: curated_matches
    target: customer
    link_key: match_id
    links:
      - { field: crm_id, references: crm }
      - { field: billing_id, references: billing }
    # No fields — pure linkage mapping (IVM-safe via link_key)

tests:
  - description: "Curated merge via linking table — target model has no source PKs"
    input:
      crm_contacts:
        - { crm_id: "CRM1", contact_name: "John Doe", contact_email: "john@example.com" }
        - { crm_id: "CRM2", contact_name: "Jane Smith", contact_email: "jane@example.com" }
      billing_accounts:
        - { billing_id: "BILL1", account_name: "J. Doe", billing_email: "john@example.com" }
      curated_matches:
        - { match_id: "M1", crm_id: "CRM1", billing_id: "BILL1" }
    expected:
      crm:
        updates:
          - { crm_id: "CRM1", contact_name: "John Doe", contact_email: "john@example.com" }
          - { crm_id: "CRM2", contact_name: "Jane Smith", contact_email: "jane@example.com" }
      billing:
        updates:
          - { billing_id: "BILL1", account_name: "John Doe", billing_email: "john@example.com" }
        inserts:
          # Jane has no billing record. The insert carries _cluster_id.
          - { name: "Jane Smith", email: "jane@example.com",
              _cluster_id: "..." }
```

**What happens next**: the ETL writes Jane to billing, gets BILL-99, and adds
`{ match_id: "M2", crm_id: "CRM2", billing_id: "BILL-99" }` to the
curated_matches table. Next run, the link connects them → no more insert.

### Propagation — same mechanism, ETL writes the links

The mapping above works identically when the ETL populates `curated_matches`
instead of a human. The only difference is who writes the rows.

```
Run 1:
  crm_contacts:  [{ crm_id: "CRM1", contact_name: "Alice", ... }]
  billing:       [] (empty)
  curated_matches: [] (empty)

  Identity view: CRM1 is a singleton entity.
  Delta for billing: insert | _cluster_id=md5("crm:CRM1") | name=Alice | ...

  ETL writes to billing → gets BILL-99.
  ETL writes to etl_billing_members: { cluster_id: md5("crm:CRM1"), src_id: "BILL-99" }

Run 2:
  billing now has BILL-99. cluster_members links it to cluster md5("crm:CRM1").
  Delta for billing: update (not insert).

  If a third system (erp) also needs Alice:
    Delta for erp: insert | _cluster_id=md5("crm:CRM1") | ...
    ETL writes to erp → gets ERP-42.
    ETL writes to etl_erp_members: { cluster_id: md5("crm:CRM1"), src_id: "ERP-42" }

Run 3:
  All three systems linked. No more inserts.
```

The key insight: **`cluster_members` is the ETL feedback mechanism; `links` is
for external linking systems.** Scenario 1 (single natural key) needs neither.

## `links` Spec Design

### Syntax

```yaml
mappings:
  # IVM-safe: link_key provides pre-computed cluster identity
  mdm_links:
    source: mdm_xref
    target: customer
    link_key: entity_id                  # column providing cluster identity
    links:
      - field: crm_id
        references: crm
      - field: billing_id
        references: billing

  # Batch-safe: no link_key — engine computes clusters in identity layer
  splink_matches:
    source: splink_output
    target: customer
    links:
      - field: crm_id                    # single PK — string
        references: crm
      - field: [region, billing_id]      # composite PK — list (names match)
        references: billing
      - field:                           # composite PK — map (names differ)
          src_region: region
          src_billing_id: billing_id
        references: billing
```

`link_key` is **optional**. When present, it names a column in the linking
table whose value serves as the cluster identifier — enabling the IVM-safe
path (LEFT JOIN `_cluster_members_{target}` in the forward view).

When `link_key` is **omitted**, the engine falls back to the batch-safe path:
pairwise link edges are computed directly in the identity layer's connected-
components algorithm. No `_cluster_members` join, no forward-view LEFT JOIN.
This is simpler but not IVM-safe (see "Two modes for links" above).

The engine unpivots each `links` row into `_cluster_members_{target}`:

```sql
-- For a link row with link_key=M1, crm_id=CRM1, billing_id=BILL1:
-- unpivots to:
--   (_cluster_id='M1', _mapping='crm',     _src_id='CRM1')
--   (_cluster_id='M1', _mapping='billing',  _src_id='BILL1')
```

`field` is polymorphic:

| Type | Meaning |
|------|---------|
| `string` | Single PK, link column name = PK column name |
| `list` | Composite PK, link column names = PK column names |
| `map` | Composite PK, link column → PK column name mapping |

### Semantics

Each row in the linking table produces an identity edge for every **pair** of
referenced sources. If a row has `crm_id=CRM1` and `billing_id=BILL1`, the
edge is: "the crm row with PK CRM1 and the billing row with PK BILL1 are the
same entity for this target."

A NULL value in a link field means "no reference for this source in this row" —
no edge produced for that pair. This allows partial links (e.g., an xref row
that links crm↔billing but not yet erp).

### Identity view integration

The identity view gains a second edge source alongside identity-field matching:

```sql
WITH
_link_edges AS (
  -- Pre-computed edges from links declarations
  SELECT
    crm._entity_id AS entity_a,
    billing._entity_id AS entity_b
  FROM curated_matches md
  JOIN _id_numbered crm
    ON crm._mapping = 'crm' AND crm._src_id = md.crm_id::text
  JOIN _id_numbered billing
    ON billing._mapping = 'billing' AND billing._src_id = md.billing_id::text
  WHERE md.crm_id IS NOT NULL AND md.billing_id IS NOT NULL
)
```

These edges feed into the same connected-components algorithm alongside
identity-field edges. Both edge types compose cleanly.

A mapping with `links` and no `fields` does NOT produce a forward view — it
only contributes identity edges (and, when `link_key` is present, cluster
membership rows for the LEFT JOIN).

## Multi-valued xref columns

In practice, columnar xref tables sometimes contain multi-valued columns —
comma-separated strings, arrays, or JSON arrays — representing internal merges:

```
match_id | crm_ids          | billing_id
M1       | "CRM1,CRM2"      | BILL1
M2       | "CRM3"           | BILL2
```

Here `crm_ids` holds two CRM identifiers that have been merged within CRM.
To produce correct cluster membership, the engine would need to unnest
`"CRM1,CRM2"` into two separate edges.

### Recommendation: push to data layer

The mapping language does **not** handle unnesting of multi-valued columns.
This is a pre-processing concern:

1. **Create a view** that normalizes the xref table before mapping:
   ```sql
   CREATE VIEW curated_matches_normalized AS
   SELECT match_id, unnest(string_to_array(crm_ids, ',')) AS crm_id, billing_id
   FROM curated_matches;
   ```
2. **Map against the view** instead of the raw table.

**Rationale**:
- Multi-valued encoding is diverse (CSV strings, PG arrays, JSON arrays,
  pipe-delimited, etc.). Supporting all formats would add significant
  complexity to the mapping language for a niche concern.
- A normalizing view is a one-liner in SQL and can be validated independently.
- The engine expects each row in a linking table to represent one link per
  referenced source. If a row logically contains multiple links, the source
  should present them as multiple rows.
- The mapping author controls what the engine sees via `sources.table`, which
  can be a view. Normalising multi-valued columns is a natural fit for this
  boundary.

## What the Engine Provides

### `_cluster_id` on insert rows

The delta view emits `_cluster_id` for every insert row:

```sql
CASE
  WHEN src._row_id IS NULL THEN rev._cluster_id
  ELSE NULL
END AS _cluster_id
```

Where `_cluster_id` comes from the identity view's `_entity_id_resolved`.

### Deterministic cluster identity

The identity view assigns each entity a deterministic identifier:

- **Without linking state**: `md5(_mapping || ':' || _src_id)`, propagated
  through connected-components → `MIN()` across the cluster. Same input data
  → same `_cluster_id`. Changes if the canonical (minimum) member changes.

- **With linking state** (ETL feeds back a linking table as a source): the
  link edges produce larger, stable clusters. The `_cluster_id` is as stable
  as the linking table's data.

### Provenance view (optional)

```sql
CREATE OR REPLACE VIEW _provenance_{target} AS
SELECT
  _entity_id_resolved AS _cluster_id,
  _mapping,
  _src_id
FROM _id_{target};
```

Lists all source rows belonging to each cluster. Useful for the ETL to
understand cluster composition, but not required for basic operation.

## What the Engine Does NOT Provide

- **Persistent cluster state** — the ETL manages its own state
- **Insert tracking** — the ETL knows what it has written
- **Generated ID feedback** — the ETL feeds this back via a linking table that
  the author maps as a regular source with `links`

## Current Spec Pattern (to be replaced)

The spec currently uses per-source `_origin_{mapping}_{pk}` columns on insert
rows in test expectations:

```yaml
inserts:
  - { person_id: "P4", _origin_crm_a_person_id: ["P4"], customer_id: "CUST-002" }
```

### Problems

| Issue | Severity |
|-------|----------|
| Column explosion (N sources → N columns) | High |
| Schema instability (adding a source changes all delta views) | High |
| Column name length (63-char PG limit risk) | Medium |
| Composite key encoding (objects in arrays) | Medium |
| **Forces source PKs onto the target model** (e.g. `crm_id: identity`) | **High** |

The last problem is the most insidious: the `merge-curated` and
`merge-generated-ids` examples require every source system's PK to be a field
on the canonical target. Adding a 4th source means adding a 4th identity field,
changing every view in the pipeline. The `links` mechanism solves this by
keeping source PKs in the linking mapping, off the target model entirely.

### Proposed replacement

Replace all `_origin_*` columns with a single `_cluster_id`:

```yaml
inserts:
  - person_id: "P4"
    customer_id: "CUST-002"
    _cluster_id: "a7f3b2..."
```

One column, stable schema, scalar value. The ETL joins against the provenance
view if it needs to know which sources contributed.

## Spec Changes Required

### 1. Replace `_origin_*` with `_cluster_id` in test expectations

All examples with `_origin_*` columns update to use `_cluster_id`.

### 2. Add `sources:` section (shared with Phase 8)

```yaml
sources:
  crm_a:
    table: crm_a_contacts
    primary_key: [person_id]
```

Needed for:
- Real primary keys (Phase 8)
- Decoupling mapping names from table names
- `links` references (must know the referenced source's PK)

### 3. Add `links`, `link_key`, `cluster_members`, and `cluster_field` to mapping schema

```yaml
# External linking (MDM, curation, record linkage)
link_key: <column_name>           # optional — enables IVM-safe path
links:
  - field: <string | list | map>
    references: <source_name>

# ETL feedback — per-mapping table, all fields have defaults
cluster_members: true | <object>
  table: <table_name>             # default: _cluster_members_{mapping}
  cluster_id: <column_name>       # default: _cluster_id
  source_key: <column_name>       # default: _src_id

# ETL feedback — cluster ID stored on the source record
cluster_field: <column_name>        # column in source table holding _cluster_id
```

### 4. Deterministic `_entity_id` in identity view

Replace `ROW_NUMBER()` with `md5(_mapping || ':' || _src_id)` so `_cluster_id`
is deterministic across runs.

## Implementation Phases

1. **Switch identity view to deterministic hashing** — `render/identity.rs`.
   Replace `ROW_NUMBER()` with `md5()`.

2. **Add `_cluster_id` to delta view** — emit `_entity_id_resolved` on insert
   rows in `render/delta.rs`.

3. **Add `cluster_members` and `cluster_field` support** — parse on mappings.
   For `cluster_members`: generate per-mapping contributions to
   `_cluster_members_{target}` and forward-view LEFT JOIN. For `cluster_field`:
   use `COALESCE(s.{cluster_field}, md5(...))` directly in the forward view.

4. **Add `links` support** — parse on mappings, generate link-edge CTEs in
   identity view. When `link_key` is present, also generate unpivoted
   `_cluster_members_{target}` view and forward-view LEFT JOIN (IVM-safe path).
   When `link_key` is absent, only generate pairwise link edges (batch path).

5. **Add provenance view** — optional `_provenance_{target}` view.

6. **Update test expectations** — replace `_origin_*` with `_cluster_id` in
   all examples that have insert rows.

7. **Validator rules**:
   - Warn when a target has 2+ identity fields and also has insert-producing
     mappings (the multi-value hazard).
   - Info when `links` is present without `link_key` — "batch-safe only;
     add `link_key` for IVM safety."
   - Error when a mapping declares both `cluster_members` and `cluster_field`.
   - Warn when `links` without `link_key` is used but no insert-producing
     mapping for the same target has `cluster_members` or `cluster_field`.
   - Warn when a `links` mapping also has `fields` (unusual but allowed).

## ETL Feedback Pattern (Documentation)

The engine doesn't implement ETL logic, but the docs should describe both
feedback paths:

**Path A — `cluster_members` (separate feedback table):**

```
┌──────────────────────────────────────────────────────┐
│  1. Read delta view → find inserts with _cluster_id    │
│  2. Write to target system → capture generated ID      │
│  3. Write (_cluster_id, new_id) to cluster_members     │
│  4. Next run: forward view LEFT JOINs the table        │
│     → row gets _cluster_id → insert disappears         │
└──────────────────────────────────────────────────────┘
```

**Path B — `cluster_field` (cluster ID stored on the target record):**

```
┌──────────────────────────────────────────────────────┐
│  1. Read delta view → find inserts with _cluster_id    │
│  2. Write to target system, including _cluster_id      │
│     as a custom property on the record                 │
│  3. Next run: source reads the row with cluster_field    │
│     populated → forward view uses it → insert gone     │
└──────────────────────────────────────────────────────┘
```

Path B is simpler (no separate table) but requires the target system to
support storing `_cluster_id` as a custom field. Path A works universally.

The ETL feedback table (Path A) is per-mapping and minimal:

```sql
CREATE TABLE etl_billing_members (
  cluster_id  TEXT NOT NULL,
  src_id      TEXT NOT NULL,
  PRIMARY KEY (src_id)
);
```

The ETL only needs: the `_cluster_id` from the delta insert row + the ID it
got back from the target system. No knowledge of other sources' PKs required.

This is outside the engine's scope — it's a reference pattern for ETL authors.
