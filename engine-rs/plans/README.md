# Plans

Design plans and architectural decision records for the OSI mapping engine.

| Plan | Status | Summary |
|------|--------|---------|
| [ASYMMETRY-ANALYSIS.md](ASYMMETRY-ANALYSIS.md) | Design | Read/write asymmetry: mapping concern vs ETL concern — analysis of where shape differences belong. |
| [PLAN.md](PLAN.md) | Done | Original implementation plan — Rust engine compiling YAML to a DAG of PostgreSQL views. |
| [PRIMARY-KEYS-PLAN.md](PRIMARY-KEYS-PLAN.md) | Done | Replace synthetic `_row_id` with real source primary keys via `sources:` section. |
| [ANALYTICS-VIEW-PLAN.md](ANALYTICS-VIEW-PLAN.md) | Done | Consumer-friendly analytics view exposing resolved golden records. |
| [ORIGIN-PLAN.md](ORIGIN-PLAN.md) | Done | Origin tracking and insert feedback to prevent duplicate inserts. |
| [DIAMOND-AVOIDANCE-PLAN.md](DIAMOND-AVOIDANCE-PLAN.md) | Done | Analysis of the reverse view's diamond dependency — accepted and documented. |
| [FORWARD-VIEWS-PLAN.md](FORWARD-VIEWS-PLAN.md) | Done | Restored separate forward views for debuggability and rollout. |
| [FK-REFERENCES-PLAN.md](FK-REFERENCES-PLAN.md) | Done | Explicit `references:` on field mappings for FK reverse resolution. Replaces LCP heuristic. |
| [DEEP-NESTING-PLAN.md](DEEP-NESTING-PLAN.md) | Done | Forward + delta reconstruction at arbitrary depth (recursive tree-based CTEs). |
| [TEST-PROGRESS-PLAN.md](TEST-PROGRESS-PLAN.md) | Done | Generic test runner — 35/35 examples passing E2E. |
| [NESTED-TYPED-NOOP-PLAN.md](NESTED-TYPED-NOOP-PLAN.md) | Done | Fix `_osi_text_norm` to normalize both sides of nested noop comparison for type-aware fields. |
| [ATOMIC-GROUPS-PLAN.md](ATOMIC-GROUPS-PLAN.md) | Done | Implement atomic resolution groups (`group:` property) using DISTINCT ON CTEs. |
| [MAPPING-CORRECTNESS-PLAN.md](MAPPING-CORRECTNESS-PLAN.md) | Done | Audit of questionable expected data: type declarations, REGEXP_REPLACE, embedded identity. |
| [COMPOSITE-KEY-REFS-PLAN.md](COMPOSITE-KEY-REFS-PLAN.md) | Done | PK columns mapped to reference fields use COALESCE for insert rows. |
| [VIEW-CONSOLIDATION-PLAN.md](VIEW-CONSOLIDATION-PLAN.md) | Done | Changes 1-3 rejected for debuggability; change 4 (naming) kept. |
| [REFERENCE-HEURISTIC-PLAN.md](REFERENCE-HEURISTIC-PLAN.md) | Superseded | LCP heuristic — replaced by [FK-REFERENCES-PLAN](FK-REFERENCES-PLAN.md). |
| [NAMING-PLAN.md](NAMING-PLAN.md) | Design | Project + binary naming: recommends "Crossfold"; availability checked across crates.io/GitHub. |
| [SOURCE-TYPES-PLAN.md](SOURCE-TYPES-PLAN.md) | Done | Source `fields:` with `type:` for PK casting; target field `type:` covers forward view. |
| [SOURCE-GROUPING-PLAN.md](SOURCE-GROUPING-PLAN.md) | Design | Visual grouping for related datasets in DOT output. |
| [SOURCE-REMOVAL-OPTIONS.md](SOURCE-REMOVAL-OPTIONS.md) | Design | Cluster split risk when mappings removed; mitigation strategy needed. |
| [JSON-FIELDS-PLAN.md](JSON-FIELDS-PLAN.md) | Done | `source_path` property for JSONB sub-field extraction with deep path support. |
| [COMPOSITE-TYPES-PLAN.md](COMPOSITE-TYPES-PLAN.md) | Proposed | Replace JSONB with PostgreSQL composite types for typed nested array output. |
| [PARENT-MAPPING-PLAN.md](PARENT-MAPPING-PLAN.md) | Done | Unify `embedded` + `source.path` under `parent:` with `array`/`array_path` for nested arrays. |
| [HIERARCHY-MERGE-PLAN.md](HIERARCHY-MERGE-PLAN.md) | Done | Example: merge 2-level and 3-level project hierarchies across systems. |
| [DEPTH-MISMATCH-PLAN.md](DEPTH-MISMATCH-PLAN.md) | Done | Example: merge when one system has a deeper intermediate level than the other. |
| [COMPUTED-FIELDS-PLAN.md](COMPUTED-FIELDS-PLAN.md) | Design | Cross-target aggregation (`from:` + `match:`), recursive self-traversal (`traverse:`), and missing-bottom example. |
| [POSITIONAL-ARRAY-PLAN.md](POSITIONAL-ARRAY-PLAN.md) | Superseded | Superseded by CRDT-ORDERING-PLAN — used position as identity, fragile for multi-source. |
| [CRDT-ORDERING-PLAN.md](CRDT-ORDERING-PLAN.md) | Done | CRDT ordering for array elements: `order: true` + optional `order_prev`/`order_next` linked-list merge. |
| [PROPAGATED-DELETE-PLAN.md](PROPAGATED-DELETE-PLAN.md) | Done | GDPR-style deletion propagation using regular target fields + `reverse_filter` — no engine changes. |
| [ELEMENT-DELETION-PLAN.md](ELEMENT-DELETION-PLAN.md) | Done | Element-level deletion for array targets — `_element_delta_{child}` views via parent `written_state`. |
| [HARD-DELETE-PROPAGATION-PLAN.md](HARD-DELETE-PROPAGATION-PLAN.md) | Design | Hard-delete propagation via ETL-layer provenance tracking — prevents re-insertion loops. |
| [ETL-STATE-INPUT-PLAN.md](ETL-STATE-INPUT-PLAN.md) | Done (Phase 1) | ETL-maintained state as engine input — `written_state` table + `written_noop` opt-in for target-centric noop detection. |
| [EVENTUAL-CONSISTENCY-PLAN.md](EVENTUAL-CONSISTENCY-PLAN.md) | Design | Write-read visibility delays: failure modes and ETL-layer mitigation strategies for eventually consistent sources. |
| [PRECISION-LOSS-PLAN.md](PRECISION-LOSS-PLAN.md) | Done | `normalize` property on field mappings for lossy noop comparison and echo-aware `last_modified` resolution. |
| [MULTI-VALUE-PLAN.md](MULTI-VALUE-PLAN.md) | Done | Cardinality mismatch (single vs. multi-value fields) — mapping patterns, no engine changes. |
| [EXPRESSION-SAFETY-PLAN.md](EXPRESSION-SAFETY-PLAN.md) | Done | Validate expressions as safe SQL snippets; cross-target `lookup:` superseded by COMPUTED-FIELDS-PLAN. |
| [TARGET-ARRAYS-PLAN.md](TARGET-ARRAYS-PLAN.md) | Maybe | Array-typed fields on targets (`text[]`) — eliminates child targets for simple value lists. |
| [PROPTEST-PLAN.md](PROPTEST-PLAN.md) | Done | Property-based testing harness using `proptest` to fuzz the engine with random mapping documents. |
| [ANALYTICS-PROVENANCE-PLAN.md](ANALYTICS-PROVENANCE-PLAN.md) | Planned | Provenance + contributions views — trace golden records back to source data. |
| [PASSTHROUGH-PLAN.md](PASSTHROUGH-PLAN.md) | Done | Carry unmapped source columns through to delta output for ETL context. |
| [NULL-WINS-PLAN.md](NULL-WINS-PLAN.md) | Maybe | `null_wins` expression on field mappings — may not implement; sentinel pattern works today. |
| [OUTPUT-CONTRACT-PLAN.md](OUTPUT-CONTRACT-PLAN.md) | Maybe | Tracks hardcoded consumer-facing output columns (`_cluster_id`, `_action`, `_src_id`); configurable aliases via `output.columns`. |
| [NATURAL-KEYS-PLAN.md](NATURAL-KEYS-PLAN.md) | Done | Natural keys (email, business codes, composite PKs) work correctly today — no engine changes needed. |
| [TYPE-HIERARCHY-PLAN.md](TYPE-HIERARCHY-PLAN.md) | Design | `hierarchy:` on target fields for IS-A type relationships; `type_matches` helper in reverse_filter. |
| [TARGET-PATH-PLAN.md](TARGET-PATH-PLAN.md) | Design | Analysis of `target_path` (dotted notation on targets) — recommends output formatting over pipeline changes. |
| [DBT-OUTPUT-PLAN.md](DBT-OUTPUT-PLAN.md) | Design | Generate a dbt project from mapping YAML; `ViewOutput` refactor; compatible with custom materialisations. |
| [MATERIALIZED-VIEW-INDEX-PLAN.md](MATERIALIZED-VIEW-INDEX-PLAN.md) | Done | Opt-in materialized views with unique indexes; `NULLS NOT DISTINCT` for delta/reverse layers. |
| [POLYGLOT-SQL-PLAN.md](POLYGLOT-SQL-PLAN.md) | Design | Multi-dialect SQL rendering via polyglot-sql; expression dialect choice; phased adoption plan. |
| [UNIT-TEST-PLAN.md](UNIT-TEST-PLAN.md) | Done | Unit tests for render pipeline; reduce reliance on slow integration suite. |
| [LEARNING-GUIDE-PLAN.md](LEARNING-GUIDE-PLAN.md) | Planned | Progressive learning guide teaching mapping concepts from first principles. |
| [DOCS-SITE-PLAN.md](DOCS-SITE-PLAN.md) | Done | Publish documentation as a static site using mdBook (`book.toml`) with GitHub Pages. |
| [CI-RELEASE-PLAN.md](CI-RELEASE-PLAN.md) | Planned | GitHub Actions CI/CD, pre-built binaries via cargo-dist, crate publication. |
| [CONSUMER-NAMING-PLAN.md](CONSUMER-NAMING-PLAN.md) | Planned | Rename `_delta_` → `sync_` and `_cluster_members_` → `cluster_members_` for consumer-facing consistency. |
| [CODE-COVERAGE-PLAN.md](CODE-COVERAGE-PLAN.md) | Done | Code coverage via cargo-llvm-cov with Codecov reporting. |
| [CODE-QUALITY-PLAN.md](CODE-QUALITY-PLAN.md) | Done | Enforce rustfmt, clippy, cargo-deny; one-time codebase cleanup. |
| [PGTRICKLE-OUTPUT-PLAN.md](PGTRICKLE-OUTPUT-PLAN.md) | Design | External post-processor that rewrites engine views as pg_trickle stream tables; per-view config. |
| [YAML-VS-DSL-PLAN.md](YAML-VS-DSL-PLAN.md) | Design | Analysis of YAML vs custom DSL for the mapping format; recommends staying with YAML. |
| [DERIVED-TIMESTAMPS-PLAN.md](DERIVED-TIMESTAMPS-PLAN.md) | Design | Derive `_last_modified` from `_written_at` column for sources without intrinsic timestamps. |
| [TIME-RANGE-RESOLUTION-PLAN.md](TIME-RANGE-RESOLUTION-PLAN.md) | Design | Support `last_modified` as a time range (min/max) instead of a single point; range resolution strategies. |
