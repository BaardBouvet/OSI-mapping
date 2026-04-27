# Roadmap

The engine ships in numbered releases. Status of individual plans lives
in [plans/README.md](plans/README.md); this file only tracks what is in
or planned for each release.

## 0.1 — Current

Mapping → PostgreSQL views (stable) and SPARQL CONSTRUCT artifacts
(preview). One v2 schema, two backends, identical conformance results
on the supported subset.

**v2 schema (locked for 0.1.x):**
- `sources` with `primary_key`, `targets` with `identity` (single field
  or AND-tuple) and `fields` (`coalesce` / `last_modified` strategies),
  `mappings` with optional `parent` / `array` / `parent_fields` for
  shallow nested arrays, inline `tests`.
- See [docs/reference/schema-reference.md](../docs/reference/schema-reference.md).

**PostgreSQL backend (stable):**
- Forward / identified / resolved / reverse / delta views per mapping.
- 4 v2 examples passing E2E in the conformance suite (hello-world,
  composite-identity, last-modified, nested-arrays-shallow).

**SPARQL backend (preview):**
- CONSTRUCT-only artifact pipeline — see
  [SPARQL-CONSTRUCT-ARTIFACTS-PLAN.md](plans/SPARQL-CONSTRUCT-ARTIFACTS-PLAN.md).
- Same artifacts run in-process (Oxigraph, for tests) and on a deployed
  incrementally maintained triplestore.
- Same 4 conformance scenarios green: `hello_world`,
  `composite_identity`, `last_modified`, `nested_arrays_shallow`.
- Configurable base IRI via `--base-iri` / `render_sparql_with_base`
  (validated; must be an absolute IRI ending in `/`).

**42 v1 examples** exist in the repository under [`examples/`](../examples/);
they predate v2 and are not exercised by the v2 engine. Migration is
scheduled across 0.4 and 0.5.

**CLI:** `parse`, `validate`, `dot`, `render` (with `-b pg|sparql`,
`--out-dir`, `--base-iri`).

**Architecture history.** Per-feature design rationale and the v1
implementation history is preserved in [plans/](plans/); see the
**Lessons from v1** section below for the invariants the v2 engine
must continue to honour.

## 0.2 — Planned

Theme: **release engineering, safety hardening, and SPARQL polish.**

- [CI-RELEASE-PLAN.md](plans/CI-RELEASE-PLAN.md) — GitHub Actions, prebuilt
  binaries, crate publication.
- [LEARNING-GUIDE-PLAN.md](plans/LEARNING-GUIDE-PLAN.md) — progressive
  7-chapter guide.
- [CONSUMER-NAMING-PLAN.md](plans/CONSUMER-NAMING-PLAN.md) +
  [DELTA-RESERVED-COLUMNS-PLAN.md](plans/DELTA-RESERVED-COLUMNS-PLAN.md) —
  stabilise consumer-facing names and namespace metadata columns. **Must
  land before any 1.0 contract freeze.**
- [CLI-TEST-COMMAND-PLAN.md](plans/CLI-TEST-COMMAND-PLAN.md) —
  `osi-engine test` subcommand.
- [SCHEMA-VALIDATION-PLAN.md](plans/SCHEMA-VALIDATION-PLAN.md) — JSON
  Schema as Pass 0 so users see all structural errors at once instead of
  the first serde failure. Already proven in v1; needs porting.
- [EXPRESSION-SAFETY-PLAN.md](plans/EXPRESSION-SAFETY-PLAN.md) +
  [SQL-SAFETY-VALIDATION-PLAN.md](plans/SQL-SAFETY-VALIDATION-PLAN.md) —
  prerequisite before any `expression` strategy or user-authored SQL
  fragments land in 0.4. Validate column-level snippets, block subqueries
  / DDL / DML / internal view references; reject mapping names that
  collide with reserved view prefixes.
- [PROPTEST-PLAN.md](plans/PROPTEST-PLAN.md) — fuzz the parser /
  validator / DAG / renderers with random Doc inputs; cheap insurance
  against panics on malformed input.
- SPARQL hardening (from
  [SPARQL-CONSTRUCT-ARTIFACTS-PLAN.md](plans/SPARQL-CONSTRUCT-ARTIFACTS-PLAN.md)
  shortcomings review): `BASE` block in artifacts (replace verbose
  absolute IRIs with relative IRIs resolved against the renderer's
  base), dedupe `Display`/`write_artifacts` routing logic.

## 0.3 — Considered

Theme: **architecture polish and operational concerns.**

- [NAMING-PLAN.md](plans/NAMING-PLAN.md) — project rename.
- SPARQL artifact self-containment — drop `Doc` from `SparqlPlan` so the
  in-process executor and a deployed triplestore see the same inputs.
- Delta CONSTRUCTs — replace Rust `compute_deltas` with
  `delta_<M>.sparql` rules.
- [MATERIALIZED-VIEW-INDEX-PLAN.md](plans/MATERIALIZED-VIEW-INDEX-PLAN.md) —
  opt-in mat-views with `NULLS NOT DISTINCT` unique indexes (delta /
  reverse layers must include PK columns alongside `_canonical_id`
  because self-merges produce multiple rows per entity).
- [DELTA-CHANGES-VIEW-PLAN.md](plans/DELTA-CHANGES-VIEW-PLAN.md) —
  unified `<M>_changes` view (`UNION ALL` of inserts/updates/deletes
  with `_action` column) for stream consumers, recovering v1's single-
  view ergonomics.
- **Operational documentation** — port the v1 ops analyses into a
  runbook chapter of the learning guide:
  [EVENTUAL-CONSISTENCY-PLAN.md](plans/EVENTUAL-CONSISTENCY-PLAN.md)
  (write-read visibility delays cause delta oscillation; mitigation is
  ETL-side, not engine-side),
  [MULTI-DEPLOYMENT-LOOP-PREVENTION-PLAN.md](plans/MULTI-DEPLOYMENT-LOOP-PREVENTION-PLAN.md)
  (overlapping deployments need authority partitioning + insert circuit
  breakers),
  [HUBSPOT-DELAYED-ENRICHMENT-PLAN.md](plans/HUBSPOT-DELAYED-ENRICHMENT-PLAN.md)
  (NULL-bleed and cluster-merge failure modes from delayed enrichment),
  [COMBINED-ETL-REVERSE-ETL-ANALYSIS.md](plans/COMBINED-ETL-REVERSE-ETL-ANALYSIS.md)
  (which stateful features may eventually move out of the engine into
  a combined ETL runtime).

## 0.4 — Considered: high-value example migration (~30 examples)

Theme: **land the well-scoped features that unlock the most examples.**

Each feature requires: (a) schema update if the property name changed,
(b) engine implementation on both PG and SPARQL backends, (c) example
YAML migrated to v2 syntax, (d) conformance test added.

| Feature | Examples unlocked | Notes |
|---|---|---|
| `references:` — cross-mapping FK reverse resolution | nested-arrays-v2, nested-arrays, nested-arrays-deep, nested-array-path, crdt-ordering, crdt-ordering-linked, element-last-modified, element-priority, embedded-objects, embedded-vs-many-to-many, multi-value, reference-preservation, references, relationship-mapping, sesam-annotated, depth-mismatch, composite-keys, external-links, soft-delete-child, vocabulary-standard | **20 examples.** Biggest single unlock; already specced as slice 4 in [SPARQL-IMPLEMENTATION-PLAN.md](plans/SPARQL-IMPLEMENTATION-PLAN.md). |
| OR-identity — multiple single-field identity groups | merge-threeway, merge-internal, merge-curated | Both backends currently bail if `identity:` has more than one group. UNION of closures (SPARQL) / UNION ALL in identified view (SQL). |
| Noop / written-state — `_written` table and noop suppression | derive-noop, concurrent-detection | PG: existed in v1, needs v2 schema wiring. SPARQL: slice 6. |
| `normalize:` — lossy noop comparison | precision-loss | Normalisation function on both sides of delta comparison; prevents phantom updates on type-cast differences. |
| `source_path` — JSONB sub-field extraction | json-fields | Dotted / bracketed path into a JSONB source column. |
| `passthrough:` — carry-through columns | passthrough, asymmetric-io | Source columns in delta output not present in the canonical model. |
| `scalar: true` array expansion | scalar-array | Array column holds bare scalars; expands without named keys. |
| Misc (≤ 1 example each) | required-fields (`required:`), inserts-and-deletes (`reverse_required:`), json-opaque, flattened, value-groups, multiple-target-mappings | Small additions; likely share implementation with adjacent features. |

**Parallel work — example migration.** Every example YAML also needs
migrating from v1 syntax: move `strategy: identity` fields to
`targets.<T>.identity:`, rename `_cluster_id` → `_canonical_id`, lift
`last_modified` column name from per-field to per-mapping. Proceeds
incrementally as features land.

## 0.5 — Considered: complex features (full example parity)

Theme: **stateful, security-gated, and conditional features** — the
ones that need spec work or security review before implementation, and
several of which depend on `references:` from 0.4.

| Feature | Examples unlocked | Notes |
|---|---|---|
| `soft_delete:` — `deleted_flag` / `active_flag` / `timestamp` strategies | soft-delete, soft-delete-resurrect, hard-delete | soft-delete-child additionally gated on `references:` (0.4). Stateful; interacts with `_written` and tombstone logic. |
| `expression` strategy | value-defaults, asymmetric-io (+ sesam-annotated already in 0.4) | Needs a v2 expression spec before implementation. Security review required — user-authored SQL/SPARQL fragments. [EXPRESSION-SAFETY-PLAN.md](plans/EXPRESSION-SAFETY-PLAN.md) (0.2) is a prerequisite. |
| `route:` — conditional row routing | route, route-combined | Per-row predicate routing source rows to different target mappings. |
| `derive_timestamps:` — per-field timestamp derivation | derive-timestamps | Derives `_ts_{field}` from `_written` JSONB comparison. Depends on written-state (0.4). |

At the end of 0.5 all 47 examples (5 current + 42 v1 migrated) should
pass on both backends.

## Post-1.0 / unscheduled

Tracked in [plans/README.md](plans/README.md) under `Planned`, `Design`,
`Proposed`, `Maybe`. Releases 0.6+ pull from this pool when themes
solidify.

Notable items deliberately deferred:

- [POLYGLOT-SQL-PLAN.md](plans/POLYGLOT-SQL-PLAN.md) — multi-dialect
  SQL (Snowflake / BigQuery). PostgreSQL focus is correct for 0.x.
- [IVM-CONSISTENCY-PLAN.md](plans/IVM-CONSISTENCY-PLAN.md) — diamond
  consistency under eventually-consistent IVM (pg_ivm, Materialize,
  RisingWave). Strategic but not pre-1.0.
- [DBT-OUTPUT-PLAN.md](plans/DBT-OUTPUT-PLAN.md),
  [PGTRICKLE-OUTPUT-PLAN.md](plans/PGTRICKLE-OUTPUT-PLAN.md) —
  alternate output backends.
- [TYPE-HIERARCHY-PLAN.md](plans/TYPE-HIERARCHY-PLAN.md),
  [COMPUTED-FIELDS-PLAN.md](plans/COMPUTED-FIELDS-PLAN.md),
  [DOT-PATH-EXPRESSIONS-PLAN.md](plans/DOT-PATH-EXPRESSIONS-PLAN.md) —
  expressive features that depend on `expression` strategy and
  `references:` landing first.

## Lessons from v1 — invariants the v2 engine must honour

The pre-rewrite engine accumulated hard-won correctness fixes and
explicit rejections of tempting-but-wrong designs. The plans survived
the rewrite even though the code did not; this section captures the
non-obvious ones so v2 doesn't relearn them.

### Architectural decisions not to revisit

- **Diamond dependency in reverse views is accepted.**
  [DIAMOND-AVOIDANCE-PLAN.md](plans/DIAMOND-AVOIDANCE-PLAN.md) traded
  IVM purity for debuggability and shipped. The reverse layer joining
  back to identity is intentional, not an optimisation target.
- **Forward / identity / reverse stay as separate views.**
  [VIEW-CONSOLIDATION-PLAN.md](plans/VIEW-CONSOLIDATION-PLAN.md) +
  [FORWARD-VIEWS-PLAN.md](plans/FORWARD-VIEWS-PLAN.md): inlining as
  CTEs was tried and rejected. Per-stage views are queryable
  independently, which is worth more than the view count saved.
- **References are explicit, not heuristic.**
  [FK-REFERENCES-PLAN.md](plans/FK-REFERENCES-PLAN.md) replaced an
  LCP source-name heuristic ([REFERENCE-HEURISTIC-PLAN.md](plans/REFERENCE-HEURISTIC-PLAN.md))
  with explicit `references:` declarations. Don't reintroduce
  inference.
- **Internal vs consumer naming convention.** `_`-prefix means
  internal (subject to change); unprefixed views are the consumer
  contract. Output column names (`_canonical_id`, `_action`) keep the
  prefix to avoid collisions with user data. Locked in
  [CONSUMER-NAMING-PLAN.md](plans/CONSUMER-NAMING-PLAN.md).

### Correctness gotchas (multi-iteration fixes)

These bit the v1 engine repeatedly. The v2 conformance suite must
cover each before claiming feature parity.

- **Typed nested-array noop.**
  [NESTED-TYPED-NOOP-PLAN.md](plans/NESTED-TYPED-NOOP-PLAN.md) — when
  a nested-array field has a non-text `type:`, both sides of the
  delta comparison must run through the same normaliser. Forgetting
  one side produces phantom updates on every run.
- **Insert PK visibility.**
  [INSERT-PK-VISIBILITY-PLAN.md](plans/INSERT-PK-VISIBILITY-PLAN.md) —
  do **not** strip PK columns from insert rows. Natural / business
  keys resolve to real values on insert; surrogate keys show null.
  Both are honest and downstream relies on it.
- **Composite-key references on insert.**
  [COMPOSITE-KEY-REFS-PLAN.md](plans/COMPOSITE-KEY-REFS-PLAN.md) —
  when a source PK column is also a `references:` field, the reverse
  view needs `COALESCE(pk_extract, ref_subquery)` so updates use the
  PK and inserts use the reference lookup.
- **Nested-array reconstruction on insert.**
  [NESTED-ARRAY-INSERT-PLAN.md](plans/NESTED-ARRAY-INSERT-PLAN.md) —
  reverse reconstruction at depth ≥ 1 needs a COALESCE fallback to
  `_entity_id_resolved` plus a CASE join (PK for updates,
  `_canonical_id` for inserts). The pattern generalises to arbitrary
  nesting depth.
- **Mat-view unique indexes.**
  [MATERIALIZED-VIEW-INDEX-PLAN.md](plans/MATERIALIZED-VIEW-INDEX-PLAN.md) —
  use `NULLS NOT DISTINCT` (insert rows have null `_canonical_id`)
  and include PK columns alongside `_canonical_id` on delta /
  reverse layers (self-merges produce multiple rows per entity).

### Operational concerns the engine alone cannot fix

The engine is deterministic SQL/SPARQL. Several real-world failure
modes live outside its control and need ETL-runtime mitigation +
documentation, not engine code:

- **Eventual consistency** — write-read delays cause delta oscillation
  and stale-base noop suppression failure
  ([EVENTUAL-CONSISTENCY-PLAN.md](plans/EVENTUAL-CONSISTENCY-PLAN.md)).
- **Multi-deployment loops** — overlapping deployments treat each
  other's writes as source data
  ([MULTI-DEPLOYMENT-LOOP-PREVENTION-PLAN.md](plans/MULTI-DEPLOYMENT-LOOP-PREVENTION-PLAN.md)).
- **Delayed enrichment** — partial atomic groups, NULL bleed,
  identity corrections triggering cluster merges
  ([HUBSPOT-DELAYED-ENRICHMENT-PLAN.md](plans/HUBSPOT-DELAYED-ENRICHMENT-PLAN.md)).
- **Stateful vs stateless feature split** — `derive_noop`,
  `derive_tombstones`, `derive_timestamps` are tightly coupled to
  ETL behaviour and are candidates for a future combined-ETL runtime
  ([COMBINED-ETL-REVERSE-ETL-ANALYSIS.md](plans/COMBINED-ETL-REVERSE-ETL-ANALYSIS.md)).
  Wire them with simple LEFT JOINs on `written_state` so the option
  to extract them stays open.

These get a runbook chapter in 0.3 (see above), not engine changes.

## Principles

1. **Schema stability first.** Any change that alters the mapping YAML
   schema must land in 0.1 so external consumers can rely on the format.
2. **Security before features.** Expression safety is a prerequisite for
   trusting user-authored mappings in production.
3. **Examples prove designs.** Plans that are pure examples (no engine
   changes) can land at any time and validate assumptions early.
4. **Patterns don't block.** Plans with status `Pattern` document what
   already works — publish them independently.
5. **Defer what has workarounds.** If the existing engine can handle a
   scenario (even awkwardly), the "nice" version can wait.

## Release definition

A release is a git tag. To cut release `X.Y`:

1. All plans listed under that release have status `Done` in
   [plans/README.md](plans/README.md).
2. CI green on `main`.
3. `cargo fmt --check`, `cargo clippy --tests -- -D warnings`, and
   `cargo test` all green for both backends.
4. Bump `engine-rs/Cargo.toml` version, tag `vX.Y.Z`, push.
