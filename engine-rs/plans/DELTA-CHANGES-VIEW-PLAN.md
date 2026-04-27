# Unified `<M>_changes` view

**Status:** Proposed

v1 emitted a single `_delta_<source>` view with an `_action` column
(`update`/`insert`/`delete`). v2 splits this into three views per mapping:
`<M>_updates`, `<M>_inserts`, `<M>_deletes`. This plan keeps the three as
the canonical contract and adds **one** convenience view on top for
consumers who want a stream.

---

## Why three views are the right primitive

- **Different shapes per kind.** Inserts carry `_canonical_id` and have
  a NULL PK; deletes are essentially the source row; updates need PK +
  diffed columns. A single view forces every kind into a
  lowest-common-denominator shape with NULL columns meaning "doesn't
  apply" — a category error.
- **Different costs.** Inserts and deletes are cheap set operations
  over the reverse view; updates need column-by-column `IS DISTINCT
  FROM` against source. Forcing them into one CTE means every consumer
  pays for the most expensive kind even when subscribing to one.
- **Conformance contract is per-kind.** `expected.{updates,inserts,deletes}`
  in tests are independent lists. Three views map 1:1; one view forces
  every test harness to filter by `_action`.
- **Selective subscription.** Real ETL frequently consumes one kind
  ("fire on inserts to enqueue welcome emails"). Three views = three
  independent dependencies; one view = always tail the merged stream.

## Why one view was attractive in v1

- **Atomicity.** A single `SELECT _action, … FROM delta_M` over a
  snapshot is guaranteed to see a consistent set of changes. Three
  views queried at three different times can drift if the source
  mutates between queries.
- **Stream APIs.** Kafka-style "all change events ordered" is
  naturally one topic, not three.
- **Re-keying.** A delete of one canonical and an insert of another
  for the same source row is one ordered event in a stream; in three
  views the consumer must reconstruct the ordering.

## Recommendation

Add `<M>_changes` as a UNION ALL on top of the three primitives:

```sql
CREATE VIEW <M>_changes AS
  SELECT 'update'::text AS _action, * FROM <M>_updates
  UNION ALL
  SELECT 'insert',                * FROM <M>_inserts
  UNION ALL
  SELECT 'delete',                * FROM <M>_deletes;
```

Properties:

- **Free** — no extra computation, just a relabelling.
- **Atomic** — querying `<M>_changes` once gives a consistent snapshot
  of all three kinds.
- **Backward compatible** — primitives are untouched; the conformance
  harness keeps comparing per-kind.
- **Wide row** — columns absent from a given kind are NULL. This is
  the cost the v1 design always paid; it is now confined to one
  optional convenience view, not the canonical interface.

For the SPARQL backend, `<M>_changes` is an artifact-level concept
only: the `Deltas` struct already carries the three kinds separately,
and the harness already compares per-kind. If a SPARQL deployment
wants a single CONSTRUCT, the slice can emit one that wraps each kind
in a `?_action` literal; this is the SPARQL analogue of the PG
convenience view.

## Open questions

1. **Action ordering for re-keying.** Naïve UNION ALL has no defined
   order. If a consumer expects "delete-then-insert" semantics for
   re-keying, the view needs an `_order` column. Could derive from
   `(canonical_id, _action)` with `delete < update < insert`, but this
   is a v1.1+ refinement.

2. **Metadata columns.** `_canonical_id` (inserts), `_gen` (eventual
   consistency, see [IVM-CONSISTENCY-PLAN.md](IVM-CONSISTENCY-PLAN.md))
   need to be in the wide row. Decide which are part of the contract
   vs. opt-in.

3. **Materialized variant.** When the user opts into materialized
   views, `<M>_changes` should be materialized too; refresh order
   topologically follows its three primitives.

## Scope

- Add `<M>_changes` view to PG renderer output (always emitted).
- Update PG section banner in the SQL artifact.
- Document the wide-row semantics in `docs/reference/`.
- No new conformance tests beyond what the three primitives already
  cover; add one smoke test that asserts the union view exists and
  has all three `_action` values.
