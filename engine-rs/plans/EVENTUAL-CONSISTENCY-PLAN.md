# Eventual consistency: write-read visibility delays

**Status:** Design

Analysis of how eventual consistency in source and state tables affects the
mapping pipeline. Some systems make writes visible only after a delay (up to
~2 seconds). The engine compiles to stateless SQL views and assumes it reads
a **consistent snapshot** — when that assumption breaks, several failure modes
emerge across every pipeline stage.

Relates to [ASYMMETRY-ANALYSIS](ASYMMETRY-ANALYSIS.md) (which identifies the
"2-second delay" as an ETL-layer concern) and
[ETL-STATE-INPUT-PLAN](ETL-STATE-INPUT-PLAN.md) (`written_state` / `written_noop`).

---

## The assumption

Every generated view reads staging tables, cluster-member tables, and
written-state tables via ordinary `SELECT`. PostgreSQL guarantees
read-your-writes within a single session, but the engine has no control over
**when** the ETL populates those tables or whether the population is visible
to the session running the views.

Visibility delays appear when:

- The ETL writes to a **different database connection** (common in connection pools).
- The staging table lives in a **logical replica** with replication lag.
- An external API write returns success, but the next **read endpoint** serves
  a stale cache (cloud CRMs, SaaS platforms).
- The ETL uses **async workers** that commit at unpredictable times.

When the pipeline runs during such a window, the views see a mix of new and
stale rows — a partial snapshot.

---

## Failure modes

### 1. Delta oscillation (false updates)

**Mechanism:** ETL writes resolved value "Alice" to ERP, then immediately
re-runs the pipeline. The `_written_{mapping}` row isn't visible yet. The
delta view sees no written row → classifies the row as `insert` or `update`
instead of `noop`.

**Impact:** The ETL writes "Alice" again. On the next cycle the written row
is finally visible → `noop`. If the cycle is fast enough, this oscillates:
write → miss → write → miss.

**Severity:** High for systems with aggressive sync intervals (< 2 s). Causes:

- Wasted API calls / write-quota consumption.
- Spurious `_last_modified` bumps if the target system timestamps on write.
- Those bumps can cascade: a `last_modified` strategy field picks the
  spuriously-updated source, flipping resolution for *other* entities.

### 2. Phantom inserts and duplicate records

**Mechanism:** ETL inserts a new record into a source staging table. The
pipeline runs before the row is visible. The identity view doesn't see it, so
the entity cluster is incomplete. On the next run the row appears → a new
`_entity_id` edge connects what were previously two separate clusters.

**Impact:**

- First cycle: two separate entities with partial data → two inserts into the
  target.
- Second cycle: transitive closure merges them → one entity, the other becomes
  a delete.
- Net effect: a **phantom record** briefly exists in the target, then
  disappears.

**Severity:** Medium-high for targets that trigger downstream workflows on
insert (welcome emails, provisioning). The phantom entity can trigger
irreversible side effects.

### 3. Stale base → noop suppression failure

**Mechanism:** The `_base` snapshot (raw source columns at read time) is
compared against resolved values for noop detection. If the source staging
table still shows the *old* row version because the write hasn't propagated,
the engine sees a mismatch and emits `update` even though the target already
has the correct value.

**Impact:** Same as §1 — redundant writes. The `written_noop` mechanism
mitigates this *if* the `_written_` table is itself visible, creating a
dependency chain where *both* writes must be visible.

### 4. Foreign-key dangling references

**Mechanism:** Two related entities are written in sequence: parent first,
then child with FK. If the child's staging-table write is visible but the
parent's isn't, the forward view sees the child but the identity view can't
resolve the parent's `_entity_id`. The reverse view's FK translation
(`LEFT JOIN`) yields NULL for the parent reference.

**Impact:**

- Child written to target with NULL FK → constraint violation or orphan
  record.
- Next cycle corrects it → but the target may have already rejected the write
  or created an orphan.

**Severity:** High for systems with enforced referential integrity. Requires
either topological write ordering with a visibility wait, or upsert
idempotency at the ETL layer.

### 5. Incomplete cluster-membership feedback

**Mechanism:** `_cluster_members_{mapping}` is populated by the ETL after
writing. If the engine reads before the cluster feedback row is visible, the
forward view falls back to `md5(_mapping || ':' || _src_id)` singleton
clusters instead of the ETL-assigned cluster.

**Impact:** Entity resolution degrades to pre-feedback quality for one cycle.
Fields that should merge don't. Resolution produces incomplete golden records.
The delta emits spurious updates *reverting* target fields to single-source
values, then corrects on the next visible cycle.

**Severity:** Medium. The engine is designed for progressive refinement, but
regressing for one cycle (actively overwriting correct data) is worse than
simply not improving.

### 6. Array element ordering glitches (CRDT Tier 1)

**Mechanism:** An array element is inserted at position 3. The
`ORDINALITY`-based position (`lpad((idx-1)::text, 10, '0')`) is computed from
the current source array. If source A's write is visible but source B's
reorder isn't, the `_ordered_` view computes a mixed-consistency ordering.

**Impact:**

- Elements from source B appear in stale positions.
- Coalesced `step_order` picks stale ordinal from B, overwriting A's correct
  position.
- Reverse output reconstructs the array in wrong order for one cycle.

**Severity:** Low-medium. Self-corrects on the next consistent read. But for
source systems that diff array patches (not full replacement), the wrong
intermediate order can cascade element moves.

### 7. Delete / re-insert race (the "zombie" problem)

**Mechanism:** Source A deletes a record (sets `deleted_at`). ETL writes the
staging table. Pipeline runs before visibility → doesn't see the deletion →
`is_deleted` resolves to `false` → delta emits `noop` or an `update` that
overwrites other sources' delete flags (if `bool_or` hasn't seen the `true`
yet).

**Impact:** The record "comes back to life" in the target for one cycle. For
`reverse_filter`-gated targets (like the ERP in the propagated-delete
example), this means the record is briefly re-inserted then deleted again.

**Severity:** High for GDPR/compliance workflows where deletion must be
monotonic.

### 8. `written_noop` double-jeopardy

**Mechanism:** `written_noop: true` depends on `_written_{mapping}` being
current. The written-state table is itself subject to the visibility delay.
Two independent consistency windows must both close before the pipeline sees
correct state.

**Impact:** The probability of a false update is the *union* of the two
visibility windows, not just one. If both the source staging table and the
written-state table have independent 2-second delays, the effective
inconsistency window can be up to 4 seconds (worst case: source write at
t=0, written-state write at t=1, both visible at t=3).

---

## Interaction matrix

Which failure modes affect which pipeline stages:

| Stage | Affected by | Failure modes |
|-------|------------|---------------|
| Forward view | Source staging delay | §3 (stale base), §6 (ordering) |
| Identity view | Source staging delay, cluster-member delay | §2 (phantom inserts), §5 (incomplete clusters) |
| Resolution view | Inherits from forward + identity | §2, §3, §5, §6 |
| Reverse view | Inherits from resolution, FK target delay | §4 (dangling FKs) |
| Delta view | Written-state delay, source delay | §1 (oscillation), §7 (zombie), §8 (double-jeopardy) |

---

## Mitigation strategies

All mitigations live in the ETL layer. The engine compiles to pure SQL views
and should not embed timing logic.

### A. Visibility fence

After writing, `SELECT` in a loop until the written row is visible, then
trigger the next pipeline run.

**Guarantees:** Eliminates all eight failure modes.

**Trade-off:** Adds latency equal to the consistency window (typically 0–2 s).
Requires the ETL to know which tables were written and how to verify
visibility.

### B. Minimum cycle interval

Set the sync interval to at least 2× the maximum visibility delay of any
participating system (e.g., ≥ 5 s if worst-case delay is 2 s).

**Guarantees:** Eliminates all failure modes when the delay is bounded and
known.

**Trade-off:** Simple to implement, but wastes time for fast systems. The
delay bound must account for *all* systems, not just the slowest — the
pipeline reads all staging tables in a single snapshot.

### C. Read-your-writes session

Use the same database session/connection for ETL writes and the subsequent
pipeline read. PostgreSQL guarantees read-your-writes within a session.

**Guarantees:** Eliminates failure modes caused by staging-table delay (§1–§7)
when staging tables are in the same PostgreSQL instance.

**Trade-off:** Does not help when source data comes from external APIs with
independent consistency models. Does not help for logical replicas.

### D. Sequence token / version check

After writing, store a monotonic version token (e.g., WAL LSN, transaction
ID). Before reading, verify the token is visible in the read session. Skip
the pipeline run if not.

**Guarantees:** Strong — the pipeline only runs when all prerequisite writes
are visible.

**Trade-off:** Requires the data store to support version queries. PostgreSQL
exposes `pg_current_wal_lsn()` / `pg_last_wal_replay_lsn()` for replicas.
External APIs rarely offer this.

### E. Idempotent writes + self-correction

Accept that the first cycle may produce wrong deltas. Make all target writes
idempotent (upsert / on-conflict-do-update). Let the next consistent cycle
correct.

**Guarantees:** Eventual correctness. No data loss.

**Trade-off:** Requires targets to tolerate transient incorrect states. Not
viable when inserts trigger irreversible side effects (workflows, emails,
provisioning).

### F. Two-phase delta

Run the pipeline, collect the delta, wait for the visibility window, re-run,
diff the two deltas. Only apply actions that are stable across both runs.

**Guarantees:** Most robust against all failure modes.

**Trade-off:** Doubles compute cost. Adds a full extra cycle of latency.
Worthwhile only for high-stakes targets where correctness dominates.

---

## Recommended defaults

| System profile | Recommended strategy |
|---------------|---------------------|
| All staging in same PostgreSQL instance | **C** (same-session reads) — zero cost, full guarantee |
| PostgreSQL logical replica as read source | **D** (LSN check) — skip run until replica catches up |
| External SaaS APIs with caching | **B** (minimum interval ≥ 5 s) + **E** (idempotent writes) |
| GDPR / compliance workflows | **A** (visibility fence) or **F** (two-phase delta) |
| High-frequency sync (sub-second) | **A** (fence) — cannot rely on interval alone |

For mappings using `written_state: true`, strategy A or D is strongly
recommended. `written_noop` amplifies the consistency window (§8) and can
cause more false updates than it prevents if the written-state table is
subject to its own visibility delay.

---

## Engine-level recommendations

No engine code changes are proposed. The mitigation belongs entirely in the
ETL layer. However, the following documentation changes should be made:

1. **Schema reference** (`docs/reference/schema-reference.md`): document the
   snapshot-consistency assumption under `written_state`. Note that
   `written_noop` requires the written-state table to be visible before the
   next pipeline run.

2. **Design rationale** (`docs/design/design-rationale.md`): add a section on
   the consistency boundary — the engine assumes consistent reads, ETL is
   responsible for ensuring this.

3. **AI guidelines** (`docs/design/ai-guidelines.md`): add a one-liner noting
   the assumption so AI agents don't generate ETL patterns that violate it.

---

## Related plans

- [ASYMMETRY-ANALYSIS](ASYMMETRY-ANALYSIS.md) — identifies the visibility
  window as an ETL concern (§ "Temporal mechanics").
- [ETL-STATE-INPUT-PLAN](ETL-STATE-INPUT-PLAN.md) — `written_state` and
  `written_noop` design, directly affected by §8.
- [HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md) — hard
  deletes interact with §7 (zombie problem) when deletion visibility is
  delayed.
- [PROPAGATED-DELETE-PLAN](PROPAGATED-DELETE-PLAN.md) — soft-delete
  propagation relies on `bool_or` seeing all flags, affected by §7.
