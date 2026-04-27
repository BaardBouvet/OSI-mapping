# IVM consistency under eventual consistency

**Status:** Design

The view DAG always contains a diamond: every `<M>_reverse` depends on
the same `<target>` resolved view. Under eventual-consistent IVM
(PG materialised views with `REFRESH CONCURRENTLY`, pg_ivm, Materialize,
RisingWave, ksqlDB, …), the leaves can refresh at different times
relative to the centre, opening windows where downstream consumers see
inconsistent deltas.

---

## The diamond

```
crm ──┐         ┌── crm_reverse ── crm_{updates,inserts,deletes}
       ▼         ▼
       contact_identified ── contact ──
       ▲         ▲
erp ──┘         └── erp_reverse ── erp_{updates,inserts,deletes}
```

Centre = `<target>` resolved view. Two diamond paths (one per source).
Any IVM that refreshes nodes independently can have leaves out of sync
with the centre.

## What can go wrong

### 1. Phantom delete + phantom insert

`contact` has refreshed and re-keyed an entity (its identity value
changed across a tick). `crm_reverse` has refreshed and sees the new
entity; `erp_reverse` is still on the old refresh. A consumer reading
`erp_deletes` sees a delete that the centre has actually re-keyed.
Subsequent refresh repairs the view, but a downstream system that
acted on the delete has done the wrong thing irreversibly.

### 2. Asymmetric updates / echo

CRM observes the resolved value before ERP does. Consumer fires a
write to ERP. ERP refresh propagates the write back through the
forward view, into the centre, back out through `erp_reverse`. Now
ERP sees its own write reflected from the centre — possibly as a
*second* update event (echo).

v1 mitigated this with `derive_noop` / `written_state`. v2 has
neither yet (slice 6 in [SPARQL-IMPLEMENTATION-PLAN.md](SPARQL-IMPLEMENTATION-PLAN.md)).

### 3. Cluster-merge tearing

Two source rows with the same identity value, ingested in separate
ticks. Until the centre's refresh observes both, each looks like a
singleton cluster. A consumer in this window sees an insert that the
centre will later turn into a merge (and a corresponding delete of
the temporary canonical). Net effect: spurious insert + spurious
delete in the consumer's history.

### 4. Half-resolved canonical

The centre is mid-refresh: some fields have been resolved against the
new source data, others still reflect the old state. Reverse views
project a row that never existed as a coherent entity. Race window;
healed on the next centre refresh.

## Mitigations, in order of cost

### A. Atomic snapshots (no engine work)

Wrap consumer reads in `SET TRANSACTION ISOLATION LEVEL REPEATABLE
READ`, query `<M>_changes`
([DELTA-CHANGES-VIEW-PLAN.md](DELTA-CHANGES-VIEW-PLAN.md)) instead of
the three primitives. Eliminates *intra-mapping* drift but not
*inter-view* drift between centre and leaves.

### B. Topological refresh order (small engine work)

Refreshes always go centre → leaves: `<M>_forward` →
`<T>_identified` → `<T>` → `<M>_reverse` → `<M>_{updates,inserts,
deletes}`. Emit a `REFRESH MATERIALIZED VIEW CONCURRENTLY` script in
topological order; document that consumers must not query during a
script run.

If refreshes run in parallel, leaves wait for centre. If serialised
sequentially, the script handles it. Already half-sketched in v1's
`MATERIALIZED-VIEW-INDEX-PLAN`.

Limits: PG's `REFRESH CONCURRENTLY` does not block readers, so a
consumer can still observe an in-flight diamond. Mitigates inter-view
drift only when the consumer reads at end-of-script.

### C. Generation tokens (medium engine work)

Every refresh of the centre stamps a `_gen` column on the canonical
row. Reverse views carry `_gen` through. Delta views require
`_gen >= source._gen_seen` before emitting an action. Stale leaves
emit nothing rather than wrong things.

This is the eventual-consistency-safe contract. Prerequisite for any
IVM target. Composes with `written_state` (slice 6 in the SPARQL
plan) since both need gen-like reasoning.

Implementation:

1. Add `_gen` column to forward views: `clock_timestamp()` snapshot at
   refresh time, or a sequence advanced once per scheduler tick.
2. Centre carries `MAX(_gen)` from contributing rows.
3. Reverse view carries centre's `_gen`.
4. Delta views compare against `<source>._gen_seen` (a column on the
   source dataset, ETL-maintained) — emit only when centre is at
   least as fresh as the source observation.

Cost: extra column threading through every view; consumers must learn
to read `_gen`.

### D. Idempotent + commutative downstream writes (no engine work)

If consumers treat updates as "set value to X" rather than "increment
by N", echoes are no-ops. Combined with `written_state`, defensible.
Cheapest mitigation but pushes responsibility onto every consumer —
wrong default.

### E. Versioned snapshot artifact (heavy engine work)

Don't expose deltas as live views; produce a versioned snapshot
table (`<M>_changes_v{gen}`) atomically per pipeline run. Consumers
read whichever version is committed. This is what production CDC
systems do (Debezium snapshots, dbt incremental models). Strongest
contract, heaviest implementation.

## Recommendation

| Tier | Mitigation | When |
|---|---|---|
| Default (PG views) | A + B | Slice 0 — already partially in `MATERIALIZED-VIEW-INDEX-PLAN` |
| IVM targets (Materialize, pg_ivm, RisingWave) | C (`_gen` tokens) required | New slice between SPARQL slices 5 and 6 |
| High-stakes deployments | E (versioned snapshots) | Out of scope for the engine; provide a recipe |

## SPARQL backend

Named graphs are atomic per-update by construction in Oxigraph
(in-process), and any conformant triplestore supports SPARQL UPDATE
in a single transaction. The diamond problem is mitigated by running
the four UPDATEs (lift, identity, forward, reverse-CONSTRUCT
materialisation) as one logical transaction.

The `_gen` story still matters for cross-deployment IVM (e.g. when
the canonical graph is shared between two regions and consumers are
reading from a replica), but is not a within-engine concern. Defer
to the same slice as the PG side.

## Open questions

1. **`_gen` granularity.** Per-target `_gen`, per-row `_gen`, or one
   global pipeline `_gen`? Per-target is the cheapest contract that
   actually catches diamond skew; per-row enables fine-grained
   filtering at consumer cost.

2. **`_gen` source.** `clock_timestamp()` is monotonic-per-session
   only; `pg_current_xact_id()` is monotonic across transactions but
   wraps. A dedicated sequence is simplest. Decide before starting
   the slice.

3. **Conformance testing.** How do we deterministically test
   eventual-consistency mitigations? One option: a harness that
   refreshes views in adversarial orderings and checks that consumer
   contracts hold. Substantial test infrastructure.

4. **Interaction with `written_state`.** `written_state` already
   solves the echo problem in the steady state. `_gen` solves the
   *transient* diamond problem during refresh. They are complementary
   but the column count grows. Consider folding `_gen` into the
   written-state schema instead of adding a parallel concept.

## Scope

This plan is design-only. Implementation lands as a dedicated SPARQL
plan slice (probably "5.5") and a parallel `pg-runtime` change.
Prerequisite: `<M>_changes` view from
[DELTA-CHANGES-VIEW-PLAN.md](DELTA-CHANGES-VIEW-PLAN.md), and
`written_state` from SPARQL slice 6. No code changes from this
document.
