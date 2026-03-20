# Combined ETL + reverse ETL responsibility analysis

**Status:** Design

Several mapping-language features depend on persistent state tables that the
ETL must maintain between sync cycles.  These same features could instead be
handled by a combined ETL + reverse ETL runtime that owns the feedback loop,
freeing the engine from history-dependent behaviour.  This analysis inventories
every such feature, evaluates the trade-offs, and recommends which to keep in
the engine, which to mark experimental, and which to eventually extract.

## Background

The engine is designed as a stateless SQL compiler: YAML in, deterministic
views out.  But several features break this model by LEFT JOINing
ETL-maintained tables into the generated SQL:

| Feature | Mapping property | External table | What ETL must do |
|---------|-----------------|----------------|-----------------|
| Target-centric noop | `derive_noop` | `_written_{mapping}` | Store acknowledged field values after each write |
| Element-level deletion | `derive_tombstones` | `_written_{parent}` | Store full JSONB including nested arrays |
| Per-field timestamps | `derive_timestamps` | `_written_{mapping}` + `_written_ts` | Store per-field timestamps and written-at clock |
| Hard-delete detection | `cluster_members` or `written_state` | `_cluster_members_{mapping}` or `_written_{mapping}` | Record insert feedback or written entities |
| Insert feedback | `cluster_members` / `cluster_field` | `_cluster_members_{mapping}` or source column | Write generated ID back after INSERT |

Contrast with features that are fully deterministic from current source data
and need no external state:

| Feature | Deterministic? | Why |
|---------|---------------|-----|
| `tombstone` (soft-delete) | Yes | Detection from current source column value |
| `_base` noop (source-centric) | Yes | Rebuilt from source table each run |
| `expression` / `reverse_expression` | Yes | Pure transformation of current values |
| `reverse_filter` / `reverse_required` | Yes | Predicate on current resolved values |
| Strategy resolution (coalesce, etc.) | Yes | Current contributions only |

The deterministic features are unambiguously engine concerns.  This analysis
focuses on the non-deterministic set.

## Feature-by-feature assessment

### 1. `written_state` table wiring

**What it does:** Declares a `_written_{mapping}` table. The engine LEFT JOINs
it into the delta view on `_cluster_id`.  Row existence alone enables
hard-delete detection (entity was previously synced → suppress re-insert).

**Engine or runtime?**  The JOIN itself is mechanical SQL plumbing — easy to
generate, easy to test, deterministic given the table contents.  The engine
doesn't write to the table; it just reads.

**Verdict: engine.**  The wiring is simple, side-effect-free, and users benefit
from declaring the dependency in the mapping YAML so the DAG is self-describing.

### 2. `derive_noop` (target-centric noop)

**What it does:** Adds a second noop branch comparing resolved values against
`_written` JSONB instead of `_base`.  Answers "does the target need to change?"
rather than "did the source change?"

**Engine or runtime?**  The SQL is deterministic given the table contents.
However, correctness depends on the assumption that the ETL is the sole writer
to the target.  If an external actor modifies the target, `_written` becomes
stale and the engine silently produces false noops.

**Verdict: engine, with explicit opt-in (current design).**  The `derive_noop:
true` flag documents the sole-writer assumption in the mapping.  A runtime
could achieve the same thing by filtering delta rows post-query, but that would
discard the SQL-level optimisation (skipping the entire row before it reaches
the ETL) and lose declarative transparency.

### 3. `derive_tombstones` (element-level deletion)

**What it does:** Compares array elements in the parent's `_written` JSONB
against the current forward view.  Elements present in written state but absent
from all current sources are excluded from every source's reconstructed array,
producing element-level delete semantics.

**Engine or runtime?**  This is the most complex consumer of written state.  It
generates `_element_delta_{child}` CTEs with `jsonb_array_elements` extraction,
anti-joins, and UNION ALL paths.  The logic is deterministic given the written
JSONB, but:

- It is fragile if the JSONB schema drifts (field renames, nested key changes).
- It assumes the written JSONB is always a faithful representation of what the
  target accepted — not guaranteed for lossy targets.
- Debugging requires understanding both the engine-generated SQL and the ETL's
  written-state contents.

A combined ETL tool could compute element-level diffs in application code,
comparing the previous write payload against the current delta output.  This
would be simpler to debug, could handle schema drift more gracefully, and
would keep the engine's generated SQL focused on the stateless merge.

**Verdict: candidate for extraction.**  Mark experimental.  The engine
generates correct SQL today, but the feature is tightly coupled to ETL
behaviour and may be better served by runtime-side diffing long term.

### 4. `derive_timestamps` (per-field timestamps from written state)

**What it does:** Derives `_ts_{field}` columns by comparing current source
values against `_written` JSONB.  Changed fields get `_written_at`; unchanged
fields carry forward their timestamp from `_written_ts`.  Enables
`last_modified` resolution for sources that have no native timestamps.

**Engine or runtime?**  Like `derive_noop`, the SQL is deterministic given the
table contents.  But the feature has a bootstrap problem: on first run with no
written state, all timestamps are NULL, so the source cannot win any
`last_modified` resolution until the second cycle.  A runtime that maintains
its own change-detection log could assign timestamps on first observation
regardless of whether a write has occurred.

**Verdict: engine for now, but watch.**  The SQL generation is clean and
well-bounded.  If bootstrap edge cases multiply, consider a runtime-side
timestamp tracker.

### 5. `cluster_members` (insert feedback + hard-delete detection)

**What it does:** Two roles in one table:

1. **Insert feedback:** After the ETL inserts a new entity, it writes the
   generated source PK back to `_cluster_members_{mapping}`.  The forward view
   LEFT JOINs this to assign a stable `_cluster_id` on the next cycle.

2. **Hard-delete detection:** The delta UNION ALLs a "vanished entities" branch
   that finds `_cluster_members` rows with no corresponding resolved entity,
   emitting `'delete'` actions.

**Engine or runtime?**  The insert-feedback JOIN is mechanical plumbing
identical to `written_state` wiring — belongs in the engine.  Hard-delete
detection is also deterministic SQL given the table contents.

However, the **policy** of what to do when an entity vanishes (suppress
re-insert, propagate delete, allow re-insert via `resurrect`) is inherently a
business decision that varies by source system and deployment.  Today this is
controlled by the `resurrect` flag, which is a binary toggle.  The
[HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md) proposes richer
provenance tracking (native/inserted/vanished state machine) that is squarely
a runtime concern.

**Verdict: engine owns detection; runtime owns policy.**  The engine should
continue generating the vanished-entity SQL.  Provenance state-machine logic
(origin-wins vs. always-propagate vs. suppress-only) belongs in the ETL.

### 6. Hard-delete propagation provenance

**What it does (planned):** Track per-entity, per-mapping provenance (absent →
native → inserted → vanished) so the ETL can make informed deletion decisions.

**Engine or runtime?**  This is a state machine that evolves across sync cycles
based on write acknowledgements and source row existence.  It depends on:

- Whether the ETL inserted the entity or it appeared natively
- Whether a deletion was intentional or accidental
- Operator-defined business policy per connector

None of these are expressible as a SQL view.

**Verdict: runtime only.**  Mark any mapping-level hooks for this as
experimental until the runtime contract stabilises.

## Summary matrix

| Feature | Engine | Runtime | Experimental? |
|---------|--------|---------|--------------|
| `written_state` table wiring | **Yes** | Maintains table | No |
| `derive_noop` | **Yes** (opt-in) | — | No |
| `derive_tombstones` | Yes (today) | Candidate for extraction | **Yes** |
| `derive_timestamps` | **Yes** | — | No |
| `cluster_members` wiring | **Yes** | Writes feedback | No |
| Hard-delete detection SQL | **Yes** | — | No |
| Hard-delete propagation policy | — | **Yes** | **Yes** |
| Provenance state machine | — | **Yes** | **Yes** |
| Cluster membership repair | — | **Yes** | **Yes** |

## What "experimental" means

Features marked experimental:

- Are documented with an `Experimental` label in schema-reference and
  ai-guidelines.
- May change semantics or move to a runtime-only contract before 1.0.
- Are fully functional and tested today — "experimental" signals boundary
  uncertainty, not quality.

Features **not** marked experimental are considered stable engine surface
regardless of whether a runtime eventually wraps them.

## Risks of a combined ETL + reverse ETL runtime

The analysis above assumes a runtime will eventually exist.  Risks if it
doesn't materialise:

1. **Features stay in the engine anyway.**  The experimental label becomes
   permanent.  This is acceptable — the engine already generates correct SQL
   for all features.

2. **Runtime fragments behaviour.**  If multiple ETL tools adopt the mapping
   language, each may implement runtime policies differently.  Keeping
   detection SQL in the engine and limiting runtime to policy decisions
   minimises this risk.

3. **Over-extraction.**  Moving too much into the runtime makes the mapping
   YAML incomplete — users can't understand the pipeline from YAML alone.  The
   boundary rule: if the behaviour is deterministic from current data + declared
   inputs, it belongs in the engine.

## Conclusion

The engine should continue owning all SQL-deterministic behaviour:
`written_state` wiring, `derive_noop`, `derive_timestamps`, `cluster_members`
joins, and hard-delete detection queries.  These are well-bounded, testable,
and declaratively transparent.

Mark `derive_tombstones` and hard-delete propagation policies as experimental.
Element-level deletion via written-state JSONB extraction is the strongest
candidate for eventual runtime extraction — it is complex, schema-sensitive,
and tightly coupled to ETL write behaviour.  Hard-delete provenance (the
native/inserted/vanished state machine) is inherently a runtime concern and
should not enter the mapping schema.

No immediate code changes required.  Next step: add experimental labels to
docs for the identified features.
