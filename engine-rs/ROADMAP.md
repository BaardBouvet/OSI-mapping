# Roadmap

Phases toward a 0.1 release, plus post-0.1 plans. Each phase has a theme and
a clear set of deliverables.

## Principles

1. **Schema stability first.** Any change that alters the mapping YAML schema
   must land before 0.1 so external consumers can rely on the format.
2. **Security before features.** Expression safety is a prerequisite for
   trusting user-authored mappings in production.
3. **Examples prove designs.** Plans that are pure examples (no engine changes)
   can land at any time and should be used to validate assumptions early.
4. **Patterns don't block.** Plans with status `Pattern` document what already
   works — publish them independently.
5. **Defer what has workarounds.** If the existing engine can handle a scenario
   (even awkwardly), the "nice" version can wait until after 0.1.

## Phase 0 — Patterns and examples ✓ COMPLETE

| Plan | Status | Work |
|------|--------|------|
| PROPAGATED-DELETE-PLAN | **Done** | Example: GDPR deletion cascading via `bool_or` + `reverse_filter`. |
| MULTI-VALUE-PLAN | **Done** | Example: single-vs-multi-value cardinality mismatch. |
| HIERARCHY-MERGE-PLAN | **Done** | Example: merging 2-level and 3-level hierarchies. |
| DEPTH-MISMATCH-PLAN | **Done** | Example: asymmetric nesting depth across systems. Required engine fixes for qualified `parent_fields` and compound-identity reverse references. |
| NATURAL-KEYS-PLAN | **Done** | Investigation confirms natural keys work correctly — no engine changes needed. |
| DIAMOND-AVOIDANCE-PLAN | **Done** | Analysis of the reverse view's diamond dependency — accepted and documented. |

**Exit criteria:** All examples pass E2E.

## Phase 1 — Schema and safety ✓ COMPLETE

The two changes most likely to break existing mappings. Landed together.

| Plan | Status | Work |
|------|--------|------|
| PARENT-MAPPING-PLAN | **Done** | Unified `embedded` + `source.path` under `parent:` with `array`/`array_path` for nested arrays. |
| EXPRESSION-SAFETY-PLAN | **Done** | Expression validation (static + AST check). Cross-target `lookup:` superseded by COMPUTED-FIELDS-PLAN. |
| SCHEMA-VALIDATION-PLAN | **Done** | JSON Schema validation as Pass 0 — reports all structural errors before serde deserialization. |
| PRIMARY-KEYS-PLAN | **Done** | Replace synthetic `_row_id` with real source primary keys via `sources:` section. |
| SOURCE-TYPES-PLAN | **Done** | Source `fields:` with `type:` for PK casting; target field `type:` for forward view. |

**Exit criteria:** All examples pass with the new schema. Expression validator
rejects known-bad inputs. No mapping uses internal view names in expressions.

## Phase 2 — Core improvements ✓ COMPLETE

Engine features that improve correctness and expand what mappings can express,
without changing the schema surface locked in Phase 1.

| Plan | Status | Work |
|------|--------|------|
| PRECISION-LOSS-PLAN | **Done** | `normalize:` property on field mappings for lossy noop comparison and echo-aware `last_modified` resolution. |
| CRDT-ORDERING-PLAN | **Done** | `order: true` + optional prev/next CRDT links for nested array ordering; supersedes POSITIONAL-ARRAY-PLAN. |
| PASSTHROUGH-PLAN | **Done** | `passthrough:` list on mappings to carry unmapped columns to delta output. |
| ELEMENT-DELETION-PLAN | **Done** | Cross-source deletion-wins: when any source removes a nested array element (detected via `written_state`), the removal propagates to all deltas. |
| SOFT-DELETE-PLAN | **Done** | First-class support for source tombstones with resurrect behavior and strategy-specific undelete. |
| SOFT-DELETE-REFACTOR-PLAN | **Done** | Rename `tombstone:` → `soft_delete:` with strategy-based API (`timestamp`/`deleted_flag`/`active_flag`). Soft-deleted rows excluded from field resolution. |
| DELETION-AS-FIELD-PLAN | **Done** | `soft_delete.target` routes detection into a resolved field; `derive_tombstones` synthesizes `TRUE` for absent entities via `cluster_members`. |
| ELEMENT-SOFT-DELETE-PLAN | **Done** | Cross-source element-level soft-delete via tombstone — reuses `DeletionFilter` pipeline. |
| ELEMENT-TOMBSTONES-AS-FIELD-PLAN | **Done** | Unify `derive_tombstones` across entities and elements — one property at both levels. |
| SCALAR-ARRAY-DELETION-PLAN | **Done** | Detect element deletion in pure scalar arrays by modeling as child targets with `derive_tombstones`. |
| ETL-STATE-INPUT-PLAN | **Done** (Phase 1) | ETL-maintained `written_state` table + `written_noop` opt-in for target-centric noop detection. |
| DERIVED-TIMESTAMPS-PLAN | **Done** | Derive per-field `_ts_{field}` from `_written` JSONB comparison and `_written_at` timestamp. |
| ORIGIN-PLAN | **Done** | Track entity identity via `links` with optional `link_key` and ETL feedback for insert deduplication. |
| FK-REFERENCES-PLAN | **Done** | Explicit `references:` on field mappings for FK reverse resolution. Replaces LCP heuristic. |
| COMPOSITE-KEY-REFS-PLAN | **Done** | PK columns mapped to reference fields use COALESCE for insert rows. |
| JSON-FIELDS-PLAN | **Done** | `source_path` property for JSONB sub-field extraction with deep path support. |
| DEEP-NESTING-PLAN | **Done** | Forward + delta reconstruction at arbitrary depth using recursive tree-based CTEs. |
| ATOMIC-GROUPS-PLAN | **Done** | Atomic resolution groups (`group:` property) — all fields in a group resolve from same source. |
| COALESCE-PRIORITY-PLAN | **Done** | Upgrade coalesce strategy validation to require explicit priorities and detect duplicates. |
| NESTED-TYPED-NOOP-PLAN | **Done** | Apply `_osi_text_norm` to both sides of nested array noop comparison for type awareness. |
| ANALYTICS-VIEW-PLAN | **Done** | Consumer-friendly analytics view exposing resolved golden records with `_cluster_id`. |
| FORWARD-VIEWS-PLAN | **Done** | Restored separate forward views for debuggability and rollout. |
| MAPPING-CORRECTNESS-PLAN | **Done** | Audit and fix questionable expected data and missing type declarations in examples. |
| FIX-SOFT-DELETE-EXAMPLE-PLAN | **Done** | Fix soft-delete example test to demonstrate suppression correctly. |
| EXAMPLE-COVERAGE-PLAN | **Done** | Fill example gaps for all major schema features. |
| INSERT-PK-VISIBILITY-PLAN | **Done** | Expose source PK columns on insert rows for ETL feedback. |
| TEST-PROGRESS-PLAN | **Done** | Track and close E2E test coverage across all examples. |
| VIEW-CONSOLIDATION-PLAN | **Done** | Remove redundant views to shrink the generated SQL surface. |

**Progress:** 43 examples pass E2E (reduced from 55 via EXAMPLE-REDUCTION-PLAN).

**Exit criteria:** New examples for each feature. Noop suppression correct for
normalized fields. Ordered arrays round-trip through reverse views.
Nested array changes detected via written_noop on parent delta.
Soft-delete refactor lands: `tombstone:` removed, `soft_delete:` with
`timestamp`/`deleted_flag`/`active_flag` strategies, soft-deleted rows excluded
from field resolution.

## Phase 3 — Quality, docs, and 0.1 release

Hardening, documentation, CI/CD, and project identity before the 0.1 tag.

| Plan | Status | Work |
|------|--------|------|
| CODE-QUALITY-PLAN | **Done** | Enforce rustfmt + clippy + cargo-deny; one-time codebase cleanup. |
| CODE-COVERAGE-PLAN | **Done** | cargo-llvm-cov + Codecov; discover untested paths. |
| UNIT-TEST-PLAN | **Done** | Unit tests for render pipeline; reduce integration test reliance. |
| PROPTEST-PLAN | **Done** | Property-based fuzzing: random mapping generation, structural + execution phases. |
| MATERIALIZED-VIEW-INDEX-PLAN | **Done** | Opt-in `--materialize` flag with unique indexes for production deployments. |
| DOCS-SITE-PLAN | **Done** | mdBook documentation site with search, deployed to GitHub Pages. |
| NESTED-ARRAY-INSERT-PLAN | **Done** | Nested array reconstruction for insert rows — COALESCE fallback to `_entity_id_resolved` + `_cluster_id` join. Supports arbitrary nesting depth. |
| ENRICHED-EXPRESSIONS-PLAN | **Done** | Raw SQL enriched expressions with `LEFT JOIN LATERAL` rendering, automatic target name rewriting, DML/DDL blocking. Adds `_enriched_` view layer. |
| NESTED-ARRAY-SORT-PLAN | **Done** | `sort:` property on child mappings — custom `ORDER BY` in `jsonb_agg` for nested array reconstruction. |
| EXAMPLE-REDUCTION-PLAN | **Done** | Consolidate redundant examples (55 → 43) without loss of feature coverage. |
| CI-RELEASE-PLAN | Planned | GitHub Actions CI/CD, pre-built binaries via cargo-dist, crate publication. |
| LEARNING-GUIDE-PLAN | Planned | Progressive 7-chapter learning guide teaching mapping concepts. |
| CONSUMER-NAMING-PLAN | Planned | Rename consumer-facing `_delta_` → `sync_` and `_cluster_members_` → `cluster_members_` for naming consistency. |
| DELTA-RESERVED-COLUMNS-PLAN | Proposed | Namespace engine metadata columns (`__osi_*`) to prevent collisions with user data columns. Bundle with CONSUMER-NAMING-PLAN. |
| CLI-TEST-COMMAND-PLAN | Proposed | `osi-engine test` subcommand — execute embedded test cases against PostgreSQL. Extract from `tests/integration.rs`. |
| NAMING-PLAN | Design | Rename project (recommended: "Crossfold"). Update crate, binary, repo, docs. |

**Exit criteria:** CI pipeline green on every push. Pre-built binaries on
GitHub Releases. Documentation site live. Proptest harness runs in CI.
Project name settled and applied across all artifacts.
Consumer-facing naming consistency applied (`sync_{source}` and
`cluster_members_{mapping}`). Delta metadata columns use `__osi_*` namespace.
`osi-engine test` runs embedded test cases against a PostgreSQL database.
Nested array insert rows include child array data at arbitrary depth.
Enriched expressions documented and tested (sesam-annotated example covers
full DTL parity). Nested array sort via `sort:` on child mappings.

## Post-0.1

Plans deferred to after 0.1. All add new capabilities with existing workarounds
(Principle 5) or are additive schema surface that won't break 0.1 consumers.

### Engine features

| Plan | Status | Work |
|------|--------|------|
| ANALYTICS-PROVENANCE-PLAN | Planned | `_provenance_` and `_contributions_` views for source-tracing and stewardship. |
| COMPUTED-FIELDS-PLAN | Design | Cross-target aggregation (`from:` + `match:`), recursive self-traversal (`traverse:`). New schema surface. Partially covered by ENRICHED-EXPRESSIONS-PLAN. |
| DOT-PATH-EXPRESSIONS-PLAN | Design | Dot-path traversal in expressions for cross-target reference navigation (references-only). |
| SQL-SAFETY-VALIDATION-PLAN | Proposed | Extended SQL safety analysis — statement-level classification beyond current token-based checks. |
| COMPOSITE-TYPES-PLAN | Proposed | Replace JSONB with PostgreSQL composite types for typed nested array output. |
| TARGET-ARRAYS-PLAN | Planned | Array-typed fields on targets (`text[]`) — eliminates child targets for simple value lists. |
| NULL-WINS-PLAN | Maybe | Allow NULL from authoritative sources to override non-NULL (sentinel pattern works today). |
| SOURCE-GROUPING-PLAN | Design | Optional `system` property on sources for visual grouping in DOT graph output. |
| TYPE-HIERARCHY-PLAN | Design | `hierarchy:` on target fields for IS-A type relationships. |
| TIME-RANGE-RESOLUTION-PLAN | Design | Support `last_modified` as time range (min/max) for batch import sources. |
| HUMAN-CONFIRMATION-PLAN | Design | Human-in-the-loop approval gates for reverse ETL at system, action, field, and pattern levels. |
| DEPENDENT-INSERT-PLAN | Design | Reference-gated inserts — only emit when another entity references the inserted entity. |

### Alternative output modes

| Plan | Status | Work |
|------|--------|------|
| DBT-OUTPUT-PLAN | Design | Generate a dbt project from mapping YAML. |
| PGTRICKLE-OUTPUT-PLAN | Design | External post-processor that rewrites engine views as pg_trickle stream tables. |
| POLYGLOT-SQL-PLAN | Design | Multi-dialect SQL rendering for Snowflake and BigQuery support. |
| OUTPUT-CONTRACT-PLAN | Maybe | Document and optionally make configurable the hardcoded consumer-facing output columns. |

### Analysis and design documents

| Plan | Status | Summary |
|------|--------|---------|
| ASYMMETRY-ANALYSIS | Design | Read/write asymmetry — mapping concern vs ETL concern. |
| COMBINED-ETL-REVERSE-ETL-ANALYSIS | Design | Which stateful features belong in engine vs combined ETL runtime. |
| SOURCE-REMOVAL-OPTIONS | Design | Cluster split risk when mappings removed; mitigation strategies. |
| EVENTUAL-CONSISTENCY-PLAN | Design | Write-read visibility delays and failure modes in eventually consistent sources. |
| MULTI-DEPLOYMENT-LOOP-PREVENTION-PLAN | Design | Infinite-loop prevention across independent deployments. |
| HUBSPOT-DELAYED-ENRICHMENT-PLAN | Design | Delayed enrichment from external providers — failure modes and patterns. |
| CLUSTER-SEED-FORMAT-PLAN | Design | `_cluster_id` seed format specification for nested-array disambiguation in tests. |
| TARGET-PATH-PLAN | Design | Analysis of `target_path` (dotted notation) — recommends output formatting instead. |
| YAML-VS-DSL-PLAN | Design | YAML vs custom DSL analysis — recommends staying with YAML + JSON Schema. |

