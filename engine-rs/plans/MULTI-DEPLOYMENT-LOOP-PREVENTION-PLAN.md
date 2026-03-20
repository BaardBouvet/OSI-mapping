# Multi-deployment loop prevention

**Status:** Design

When two or more independent OSI mapping deployments synchronize overlapping
systems, each deployment treats the other's writes as source data. Neither
deployment has visibility into the other's internal linking tables, resolution
priorities, or `_base` snapshots. This creates a feedback path that can
produce infinite write oscillation — even though each deployment converges
correctly in isolation.

Relates to [EVENTUAL-CONSISTENCY-PLAN](EVENTUAL-CONSISTENCY-PLAN.md)
(single-deployment visibility delays),
[ORIGIN-PLAN](ORIGIN-PLAN.md) (`_cluster_id` and insert feedback), and
[HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md) (provenance
state machine for ETL-inserted records).

---

## The problem

### Single deployment: convergence by construction

Within a single deployment, `_base` noop suppression guarantees convergence.
The delta view compares resolved values against the source's own `_base`
snapshot. If the resolved value matches what the source already has, no delta
is emitted. After one write cycle, the source staging table reflects the
written value, `_base` matches resolved, and the delta goes silent.

```
Deployment A:
  CRM  ──_fwd──→ _resolved ──_rev──→ _delta_erp
                                       ↓
                   (ETL writes to ERP)
                                       ↓
                   ERP staging now matches resolved
                                       ↓
              Next cycle: _base == resolved → noop ✓
```

### Two deployments: the feedback loop

```
Deployment A (owns CRM ↔ ERP sync):
  CRM  ──→ _resolved_A ──→ _delta_erp_A ──→ writes to ERP

Deployment B (owns ERP ↔ Billing sync):
  ERP  ──→ _resolved_B ──→ _delta_billing_B ──→ writes to Billing
                        ──→ _delta_erp_B ──→ writes back to ERP (!)
```

When deployment B writes to ERP, deployment A sees ERP's staging table change.
A's `_base` for its ERP source reflects what **A** last read — not what B
wrote. If B's resolution produced a different value (different sources,
different priorities, different active mappings), A sees a mismatch and emits
an update. A writes to ERP, B sees a change, and the cycle repeats.

### Concrete scenario

```
T0 — Initial state:
  CRM:     { id: 1, name: "Acme Corp" }
  ERP:     { id: 100, name: "ACME" }
  Billing: { id: B-1, name: "Acme" }

Deployment A (CRM priority 1, ERP priority 2):
  _resolved_A: name = "Acme Corp" (CRM wins)
  _delta_erp_A: update ERP.name → "Acme Corp"

Deployment B (ERP priority 1, Billing priority 2):
  _resolved_B: name = "ACME" (ERP wins — before A's write lands)

T1 — A writes to ERP:
  ERP: { id: 100, name: "Acme Corp" }

  Deployment B runs:
  _resolved_B: name = "Acme Corp" (ERP now has A's value, ERP still priority 1)
  _delta_erp_B: noop (ERP already has "Acme Corp")
  _delta_billing_B: update Billing.name → "Acme Corp"   ← propagates correctly
```

This case converges. But consider what happens if deployment B also has a
source with higher priority that overrides ERP:

```
Deployment B (Billing priority 1, ERP priority 2):
  _resolved_B: name = "Acme" (Billing wins)
  _delta_erp_B: update ERP.name → "Acme"

T2 — B writes to ERP:
  ERP: { id: 100, name: "Acme" }

  Deployment A runs:
  _resolved_A: name = "Acme Corp" (CRM still priority 1)
  _delta_erp_A: update ERP.name → "Acme Corp"   ← overwrites B's write

T3 — A writes to ERP:
  ERP: { id: 100, name: "Acme Corp" }

  Deployment B runs:
  _delta_erp_B: update ERP.name → "Acme"        ← overwrites A's write

→ Infinite loop: A writes "Acme Corp", B writes "Acme", forever.
```

The root cause is **conflicting authority**: both deployments write to ERP
with different resolution outcomes, and neither recognizes the other's writes
as authoritative.

### Linking table variant

The same loop can occur through linking tables. Deployment A inserts a record
into ERP and records insert feedback in `_cluster_members_erp_A`. Deployment
B doesn't see A's cluster-member table — it sees the new ERP row as a native
record, assigns its own `_entity_id`, and may emit a conflicting insert or
update. If B writes cluster feedback, A doesn't see B's cluster-member table
either.

---

## Why `_base` noop suppression is insufficient

`_base` stores the raw source value at read time **within one deployment's
session**. It does not encode:

- Which deployment wrote the value.
- Whether the value is the output of another deployment's resolution.
- Whether the source value has been overwritten since the last read.

A second deployment's `_base` for the same source table reflects a different
read point. The two `_base` snapshots are independent — each deployment
compares against its own, and both can simultaneously conclude they need to
write.

---

## Insert loops: the catastrophic case

Update loops are annoying — they waste API quota and cause field oscillation.
But they're bounded: N entities × 2 writes/cycle. Insert loops are
**unbounded** and can take down the target SaaS system.

### Mechanism

The delta view classifies a row as `'insert'` when `_src_id IS NULL` — the
resolved entity has no member from this source. In a single deployment with
correct `cluster_members` feedback, this fires exactly once per entity per
target, then feedback links the new row and the delta goes silent.

With two independent deployments, the feedback is invisible across the
boundary:

```
T0 — Entity exists in CRM only:
  Deployment A runs:
  _delta_erp_A: _action = 'insert', _cluster_id = X
  ETL A writes ERP row id=100, records feedback in _cluster_members_erp_A

T1 — Deployment B sees ERP row 100 as native:
  B's _fwd_erp: { _src_id: 100, name: "Acme", _cluster_id: md5('erp:100') }
  B doesn't see A's _cluster_members_erp_A table.
  B assigns its own entity ID from identity resolution.
  B resolves entity → _delta_billing_B: _action = 'insert'
  ETL B writes Billing row id=B-1

T2 — But what if B also emits a delta for ERP?
  If B's mapping has sync: true on a second ERP-touching mapping, and
  identity resolution produces a DIFFERENT entity cluster than A's
  (different identity fields, different link tables), B may emit:
  _delta_erp_B: _action = 'insert', _cluster_id = Y  (different cluster!)
  ETL B writes ERP row id=101  ← A SECOND ROW in ERP for the same
                                  real-world entity
```

Now the situation escalates:

```
T3 — Deployment A sees ERP row 101 as a new source row:
  A's _fwd_erp: { _src_id: 101, ... }
  A resolves it. If identity doesn't link 100 and 101 (different cluster
  IDs, no shared identity field value), A sees TWO entities.
  _delta_erp_A: possibly a THIRD insert for yet another cluster.

T4, T5, ... — Each cycle can create new rows. Row count grows linearly
  or worse per cycle. At ETL intervals of seconds, a SaaS API receiving
  hundreds of insert calls per minute will rate-limit or fail.
```

### Why this is worse than update loops

| | Update loop | Insert loop |
|---|---|---|
| **Row count** | Fixed — same rows oscillate | **Grows** — new rows every cycle |
| **API impact** | N updates/cycle | N **inserts**/cycle, cumulative |
| **Reversibility** | Stop the ETL, values settle | Stop the ETL, **orphan rows remain** |
| **Rate limits** | Consumes update quota | Consumes **create** quota (often stricter) |
| **Blast radius** | Field values flicker | **Fills tables**, triggers workflows, sends emails |
| **Detection** | Field diff shows oscillation | New PKs appear in source tables every cycle |

A SaaS system with a 100-record/minute create rate limit will be saturated
within cycles. Systems without rate limits (self-hosted databases, data
warehouses) accumulate unbounded orphan rows.

### Root cause

The delta view's `WHEN _src_id IS NULL THEN 'insert'` branch is the correct
behavior for a single deployment — it's how new records propagate. The
problem is that across deployments, both sides independently conclude
`_src_id IS NULL` because they don't share cluster membership state. Each
side's insert creates a source row visible to the other side, which may
trigger further inserts if identity resolution doesn't converge.

The organizational mitigations described later (authority partitioning,
origin tagging) prevent loops **when correctly configured**. But
misconfiguration, new deployment onboarding, or identity resolution drift
can silently re-enable the loop. A runtime safety net is needed that works
**without organizational overview** — a circuit breaker.

---

## Insert circuit breaker

A per-mapping, per-cycle rate limiter in the ETL layer that detects runaway
insert volume and halts writes before damage accumulates.

### Design

The circuit breaker has three states:

```
                 inserts < threshold
           ┌─────────────────────────────┐
           ▼                             │
      ┌─────────┐  inserts ≥ threshold  ┌──────────┐
      │ CLOSED  │ ─────────────────────→ │  OPEN    │
      │ (allow) │                        │ (reject) │
      └─────────┘                        └──────────┘
                                           │      ▲
                            cooldown       │      │
                            expires        ▼      │ still over
                                        ┌──────────┐
                                        │HALF-OPEN │
                                        │ (probe)  │
                                        └──────────┘
                                           │
                                           │ inserts < threshold
                                           ▼
                                        ┌─────────┐
                                        │ CLOSED  │
                                        └─────────┘
```

**CLOSED:** Normal operation. ETL executes all insert deltas. Tracks insert
count per mapping per cycle in a counter table.

**OPEN:** Insert count for a mapping exceeded the threshold in a single cycle
(or across a sliding window of K cycles). All subsequent inserts for that
mapping are **dropped** (not written to the target). Updates and deletes
continue normally. The ETL logs the dropped inserts with full payload for
later replay.

**HALF-OPEN:** After a cooldown period, the ETL allows a small probe batch of
inserts (e.g., 1–5) to execute. If the probe succeeds and the next full cycle
stays under threshold, transition back to CLOSED. If the probe cycle again
exceeds threshold, transition back to OPEN.

### Parameters

```sql
CREATE TABLE _circuit_breaker (
    mapping       text NOT NULL PRIMARY KEY,
    state         text NOT NULL DEFAULT 'closed',
                  -- 'closed' | 'open' | 'half_open'
    threshold     integer NOT NULL DEFAULT 50,
                  -- max inserts per cycle before tripping
    window_cycles integer NOT NULL DEFAULT 3,
                  -- sustained high volume across N cycles to trip
    cooldown_s    integer NOT NULL DEFAULT 300,
                  -- seconds before half-open probe
    opened_at     timestamptz,
    total_dropped integer NOT NULL DEFAULT 0,
    last_cycle    integer,
    insert_counts integer[] NOT NULL DEFAULT '{}'
                  -- rolling window of per-cycle insert counts
);
```

**Threshold selection:** For most SaaS targets, the threshold should be
derived from the **known entity population**. If the target system has 500
companies, a single cycle producing 200 inserts (40% of population) is almost
certainly a loop. A reasonable default:

```
threshold = max(50, 0.1 × known_entity_count)
```

The 50-record floor prevents tripping during legitimate bulk imports.
`known_entity_count` can be estimated from `SELECT count(*) FROM source_table`
or configured statically per mapping.

**Window:** A burst of inserts in a single cycle can be a legitimate import.
Sustained high insert volume across 3+ consecutive cycles is the loop signal.
The circuit breaker should track a rolling window of per-cycle insert counts:

```
Trip condition:
  ALL of the last window_cycles cycles had insert_count ≥ threshold
```

This avoids false trips on one-time bulk loads while catching sustained loops.

### ETL integration

The circuit breaker lives entirely in the ETL layer. The engine's delta views
are unaffected. The ETL wraps delta consumption:

```python
def process_deltas(mapping, deltas):
    cb = get_circuit_breaker(mapping)
    inserts = [d for d in deltas if d.action == 'insert']
    updates = [d for d in deltas if d.action == 'update']
    deletes = [d for d in deltas if d.action == 'delete']

    # Updates and deletes always execute
    execute(updates)
    execute(deletes)

    # Circuit breaker gates inserts
    if cb.state == 'open':
        log_dropped(mapping, inserts)
        if cb.cooldown_expired():
            cb.transition('half_open')
            probe = inserts[:cb.probe_size]
            execute(probe)
            log_dropped(mapping, inserts[cb.probe_size:])
        return

    if cb.state == 'half_open':
        if len(inserts) < cb.threshold:
            cb.transition('closed')
            execute(inserts)
        else:
            cb.transition('open')
            log_dropped(mapping, inserts)
        return

    # state == 'closed'
    cb.record_insert_count(len(inserts))
    if cb.window_exceeded():
        cb.transition('open')
        log_dropped(mapping, inserts)
        alert("Circuit breaker tripped for {mapping}: "
              f"{len(inserts)} inserts in cycle, "
              f"sustained over {cb.window_cycles} cycles")
    else:
        execute(inserts)
```

### Dropped insert log

Dropped inserts must be recoverable. The ETL writes them to a dead-letter
table:

```sql
CREATE TABLE _circuit_breaker_dropped (
    id            bigserial PRIMARY KEY,
    mapping       text NOT NULL,
    cycle         integer NOT NULL,
    cluster_id    text NOT NULL,
    payload       jsonb NOT NULL,     -- full delta row
    dropped_at    timestamptz NOT NULL DEFAULT now(),
    replayed      boolean NOT NULL DEFAULT false
);
```

After the root cause is fixed (mapping corrected, authority partitioning
applied, identity resolution tuned), an operator replays the dropped inserts:

```sql
-- Replay after fix
INSERT INTO target_system (...)
SELECT (payload->>'field1'), (payload->>'field2'), ...
FROM _circuit_breaker_dropped
WHERE mapping = 'erp_companies'
  AND NOT replayed
ORDER BY dropped_at;

UPDATE _circuit_breaker_dropped
SET replayed = true
WHERE mapping = 'erp_companies' AND NOT replayed;
```

### Alerting

When the circuit breaker trips, the ETL must emit an alert with enough
context to diagnose the loop:

```
ALERT: Insert circuit breaker OPEN for mapping 'erp_companies'
  Cycle: 47
  Insert count (last 3 cycles): [312, 287, 305]
  Threshold: 50
  Sample cluster_ids: [X1, X2, X3, ...]
  Sample payloads: [...]
  Action: inserts halted, updates/deletes continue
  Dropped inserts logged to _circuit_breaker_dropped
```

The sample `cluster_id` values are critical for diagnosis — they reveal
whether the inserts are for genuinely new entities (legitimate) or for
entities that already exist under different cluster IDs (loop).

### Why this works without organizational overview

The circuit breaker doesn't need to know about other deployments. It observes
a single signal: **how many inserts is this mapping producing per cycle?** A
mapping that has been running for weeks and suddenly produces 200 inserts per
cycle is almost certainly in a loop, regardless of cause.

The threshold adapts to the mapping's expected behavior:

- A mapping syncing a 10,000-row CRM can tolerate higher insert volume.
- A mapping syncing a 50-row reference table should trip at much lower
  thresholds.
- During initial sync (backfill), the circuit breaker starts OPEN with a
  manual override to CLOSED — or the threshold is temporarily raised.

### Interaction with existing mechanisms

| Mechanism | Prevents loops | Catches loops | Stops damage |
|-----------|---------------|---------------|--------------|
| Authority partitioning | ✓ | — | — |
| Write-origin tagging | ✓ (self-echo) | — | — |
| Convergence tests | — | ✓ (design time) | — |
| Anomaly detection | — | ✓ (runtime) | — |
| **Insert circuit breaker** | — | ✓ (runtime) | **✓** |

The circuit breaker is the only mechanism that **stops damage in progress**.
All other mechanisms are preventive or detective — they require correct
configuration or human intervention. The circuit breaker acts autonomously.

---

## Mitigation mechanisms

### 1. Write-origin tagging (breaks the simplest loop)

When the ETL writes to any target system, include origin metadata:

```sql
INSERT INTO erp (id, name, _osi_origin, _osi_cycle)
VALUES (100, 'Acme Corp', 'deployment_a', 42);
```

Each deployment's mapping adds a `reverse_filter` or source `filter` that
suppresses rows originated by itself:

```yaml
# Deployment A's ERP source mapping
- name: erp_companies
  source: { dataset: erp }
  target: company
  filter: "_osi_origin IS DISTINCT FROM 'deployment_a'"
  fields:
    - source: name
      target: name
      priority: 2
```

**Mechanism:** Rows that deployment A wrote to ERP are excluded from A's
forward view. A never sees its own writes as source data → no echo. B sees
A's writes (since `_osi_origin = 'deployment_a'` ≠ `'deployment_b'`), so B
can still act on them.

**Limitation:** Only prevents self-echo. Does not prevent the T2/T3 ping-pong
above, because A's filter suppresses A-originated rows but still sees B's
writes to ERP and resolves differently.

### 2. Authority partitioning (prevents conflicting writes)

Designate one deployment as the sole writer for each (target_system, entity)
pair. Other deployments may read from that system but do not generate deltas
for it.

```
Deployment A: writes to ERP (sync: true), reads from Billing (sync: false)
Deployment B: writes to Billing (sync: true), reads from ERP (sync: false)
```

In mapping YAML, this means deployment B's ERP mapping omits `sync: true`:

```yaml
# Deployment B
- name: erp_companies
  source: { dataset: erp }
  target: company
  # sync: false (default) — no _rev_ or _delta_ generated for ERP
  fields:
    - source: name
      target: name
```

**Mechanism:** Only one deployment ever writes to each system. No conflicting
writes → no loop. Data flows unidirectionally through each system-deployment
pair.

**Limitation:** Requires organizational agreement on write ownership. Cannot
model scenarios where both deployments need to write to the same system.

### 3. Linking table ownership (prevents identity conflicts)

A links-producing mapping must be owned by exactly one deployment. Other
deployments consume the published links as a read-only source:

```yaml
# Deployment B — consumes A's links, read-only
- name: deployment_a_links
  source: { dataset: deployment_a_link_table }
  target: company
  # sync: false — B doesn't write links back to A's table
  links:
    - mapping: erp_companies
      column: erp_id
    - mapping: crm_companies
      column: crm_id
```

**Mechanism:** A single deployment is the authority on which records are
linked. Other deployments accept its identity decisions without contributing
conflicting edges.

### 4. Convergence tests (catches loops at design time)

A two-cycle idempotency test asserts that the system converges:

```yaml
tests:
  - name: convergence
    description: "Second cycle produces empty deltas"
    # Cycle 1 — initial sync
    input:
      crm: [{ id: 1, name: "Acme Corp" }]
      erp: [{ id: 100, name: "ACME" }]
    expected:
      erp_companies:
        updates: [{ id: 100, name: "Acme Corp" }]

  - name: convergence_cycle_2
    description: "After applying cycle 1, no further changes"
    input:
      crm: [{ id: 1, name: "Acme Corp" }]
      erp: [{ id: 100, name: "Acme Corp" }]   # ← reflects cycle 1 write
    expected:
      erp_companies: {}   # ← empty deltas
```

For multi-deployment scenarios, simulate both deployments' outputs:

- Run deployment A on initial state → collect deltas.
- Apply A's deltas to the source data.
- Run deployment B on the updated state → collect deltas.
- Apply B's deltas.
- Run both again → assert empty deltas.

**Mechanism:** Catches loop risk in CI before production. Any scenario that
fails the second-cycle assertion reveals a convergence bug or an authority
conflict.

**Limitation:** Requires modeling both deployments' mappings in the test. The
existing single-deployment test infrastructure supports this — each
deployment is a separate mapping file — but the test harness doesn't
natively run two files in sequence. This is a convention and tooling concern.

### 5. Runtime anomaly detection (detects loops in production)

Instrument the ETL harness (not the engine) to track:

| Signal | Detection | Response |
|--------|-----------|----------|
| `update_count(entity, cycle)` | Same entity updated on N consecutive cycles | Alert after N > 3 |
| `delta_volume(mapping, cycle)` | Sustained non-zero delta count | Alert when volume doesn't decay over K cycles |
| `origin_echo(deployment, cycle)` | Record stamped `_osi_origin = X` reappears in X's source data | Log loop warning |

```sql
-- Example: detect oscillating entities
SELECT _entity_id_resolved, count(DISTINCT _osi_cycle)
FROM etl_write_log
WHERE _osi_cycle >= current_cycle - 5
GROUP BY _entity_id_resolved
HAVING count(DISTINCT _osi_cycle) >= 4;
```

**Mechanism:** Detects loops caused by configuration drift, new source
additions, or priority changes that weren't caught by convergence tests.

### 6. Causal ordering with vector clocks (future scope)

Each entity carries a vector clock `{ A: n, B: m }` incremented by the
writing deployment. Before emitting a delta, the engine checks whether the
incoming source value's vector is dominated by the resolved value's vector.
If so, the source has already seen this write (via the other deployment) and
the delta is suppressed.

**Assessment:** Powerful — solves the T2/T3 scenario without authority
partitioning. But requires:

- A metadata column on every synced record carrying the vector clock.
- Engine awareness of vector comparison in delta computation.
- Agreement on clock semantics across deployments.

This is significant complexity. Not recommended as a first step.

---

## Recommended approach

| Phase | Mechanism | Addresses | Engine change |
|-------|-----------|-----------|---------------|
| 1 | **Insert circuit breaker** | Insert loops — stops damage in progress | None — ETL layer |
| 2 | Convergence tests (two-cycle idempotency) | All loops — at design time | None — test convention |
| 3 | Authority partitioning (`sync: true` on one deployment per system) | Conflicting writes | None — mapping convention |
| 4 | Write-origin tagging + source `filter` | Self-echo | None — ETL convention |
| 5 | Linking table ownership | Identity conflicts | None — mapping convention |
| 6 | Runtime anomaly detection (updates + inserts) | Production drift | None — ETL instrumentation |
| 7 | Vector clocks | All loops — at runtime | Significant — engine + ETL |

Phase 1 is the safety net that prevents catastrophic damage regardless of
configuration correctness. It should be deployed **before** any multi-system
sync goes live, and it requires no knowledge of other deployments.

Phases 2–5 are preventive measures that eliminate loops by design. Phase 6
detects update-level oscillation. Phase 7 is future scope.

---

## Scope boundary

The engine compiles to deterministic, stateless SQL views. Loop prevention is
fundamentally a **multi-deployment coordination concern** that lives in:

- **ETL runtime** — insert circuit breaker, origin tagging, anomaly detection.
- **Mapping conventions** — who writes to which system, who owns links.
- **Test infrastructure** — multi-deployment convergence assertions.

The insert circuit breaker is the critical runtime safety net. It requires no
engine changes, no knowledge of other deployments, and no organizational
coordination. It should be the first mechanism deployed for any multi-system
sync.

No engine code changes are required for phases 1–6.
