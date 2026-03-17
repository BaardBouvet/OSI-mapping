# Roadmap to 1.0

**Status:** Planned

Sequence the remaining non-Done plans into phases that build toward a 1.0
release. Each phase has a theme and a clear set of deliverables.

## Principles

1. **Schema stability first.** Any change that alters the mapping YAML schema
   must land before 1.0 so external consumers can rely on the format.
2. **Security before features.** Expression safety is a prerequisite for
   trusting user-authored mappings in production.
3. **Examples prove designs.** Plans that are pure examples (no engine changes)
   can land at any time and should be used to validate assumptions early.
4. **Patterns don't block.** Plans with status `Pattern` document what already
   works — publish them independently.
5. **Defer what has workarounds.** If the existing engine can handle a scenario
   (even awkwardly), the "nice" version can wait until after 1.0.

## Phase 0 — Patterns and examples (no engine changes)

Land immediately in any order. These require only new example directories and
documentation.

| Plan | Status | Work |
|------|--------|------|
| PROPAGATED-DELETE-PLAN | ~~Pattern~~ **Done** | Example: GDPR deletion cascading via `bool_or` + `reverse_filter`. Example exists and passes. |
| MULTI-VALUE-PLAN | ~~Pattern~~ **Done** | Example: single-vs-multi-value cardinality mismatch. Example exists and passes. |
| HIERARCHY-MERGE-PLAN | ~~Planned~~ **Done** | Example: merging 2-level and 3-level hierarchies. Example exists and passes. |
| DEPTH-MISMATCH-PLAN | ~~Planned~~ **Done** | Example: asymmetric nesting depth across systems. Required engine fixes for qualified `parent_fields` and compound-identity reverse references. |

**Progress:** 4/4 examples done (propagated-delete, multi-value, hierarchy-merge, depth-mismatch). 42 examples total now pass E2E.

**Exit criteria:** ~~Five~~ Four new examples passing E2E tests.

## Phase 1 — Schema and safety ✓ COMPLETE

The two changes most likely to break existing mappings. Landed together.

| Plan | Status | Work |
|------|--------|------|
| PARENT-MAPPING-PLAN | ~~Planned~~ **Done** | Unified `embedded` + `source.path` under `parent:` with `array`/`array_path` for nested arrays. |
| EXPRESSION-SAFETY-PLAN | ~~Planned~~ **Phase 1–2 done** | Expression validation (static + AST check) and `lookup:` for cross-target access. |

**Exit criteria:** ~~All 35+ examples pass with the new schema.~~ All 39 examples pass. Expression
validator rejects known-bad inputs. No mapping uses internal view names in
expressions.

## Phase 2 — Core improvements

Engine features that improve correctness and expand what mappings can express,
without changing the schema surface locked in Phase 1.

| Plan | Status | Work |
|------|--------|------|
| PRECISION-LOSS-PLAN | Planned | `normalize:` property on field mappings for lossy noop comparison (Phase 1 only: truncation, rounding, case folding). |
| CRDT-ORDERING-PLAN | Planned | `order: true` + optional prev/next CRDT links for nested array ordering; supersedes POSITIONAL-ARRAY-PLAN. |
| PASSTHROUGH-PLAN | Planned | `passthrough:` list on mappings to carry unmapped columns to delta output. |

**Exit criteria:** New examples for each feature. Noop suppression correct for
normalized fields. Ordered arrays round-trip through reverse views.

## Phase 3 — Richer types and output

Larger features that expand the type system and analytics layer.

| Plan | Status | Work |
|------|--------|------|
| TARGET-ARRAYS-PLAN | Planned | Array-typed target fields (`text[]`, `integer[]`). Eliminates child targets for simple value lists. Full pipeline impact. |
| ANALYTICS-PROVENANCE-PLAN | Planned | `_provenance_` and `_contributions_` views for source-tracing and stewardship. |

**Exit criteria:** Array fields work in forward, identity, resolution, reverse,
and delta views.

## Phase 4 — Quality, docs, and release

Hardening, documentation, CI/CD, and project identity before the 1.0 tag.

| Plan | Status | Work |
|------|--------|------|
| CODE-QUALITY-PLAN | ~~Planned~~ **Done** | Enforce rustfmt + clippy + cargo-deny; one-time codebase cleanup. |
| CODE-COVERAGE-PLAN | ~~Planned~~ **Done** | cargo-llvm-cov + Codecov; discover untested paths. |
| UNIT-TEST-PLAN | ~~Planned~~ **Done** | Unit tests for render pipeline; reduce integration test reliance. |
| PROPTEST-PLAN | Planned | Property-based fuzzing: random mapping generation, structural + execution phases. |
| CI-RELEASE-PLAN | Planned | GitHub Actions CI/CD, pre-built binaries via cargo-dist, crate publication. |
| MATERIALIZED-VIEW-INDEX-PLAN | Design | Opt-in `--materialize` flag with unique indexes for production deployments. |
| PGTRICKLE-OUTPUT-PLAN | Design | External post-processor rewriting views as pg_trickle stream tables. |
| LEARNING-GUIDE-PLAN | Planned | Progressive 7-chapter learning guide teaching mapping concepts. |
| DOCS-SITE-PLAN | Planned | mdBook documentation site with search, deployed to GitHub Pages. |
| NAMING-PLAN | Design | Rename project (recommended: "Crossfold"). Update crate, binary, repo, docs. |

**Exit criteria:** CI pipeline green on every push. Pre-built binaries on
GitHub Releases. Documentation site live. Proptest harness runs in CI.
Project name settled and applied across all artifacts.

## Post-1.0

Plans that have workarounds today, are explicitly deferred, or require more
design. They may ship as 1.x minor releases.

| Plan | Status | Reason deferred |
|------|--------|-----------------|
| COMPOSITE-TYPES-PLAN | Proposed | Replace JSONB with PostgreSQL composite types. JSONB works today; typed output is additive. |
| SOURCE-GROUPING-PLAN | Design | `system:` property on sources for visual DOT grouping. Pure cosmetic; no functional impact. |
| DBT-OUTPUT-PLAN | Design | Generate a dbt project from mapping YAML. Current `psql -f` workflow works; dbt is additive. |
| POLYGLOT-SQL-PLAN | Design | Multi-dialect SQL rendering. PostgreSQL-only is fine for 1.0; other dialects via dbt adapters. |
| COMPUTED-FIELDS-PLAN | Design | Cross-target aggregation (`from:` + `match:`), recursive traversal (`traverse:`), and missing-bottom example. |
| TYPE-HIERARCHY-PLAN | Design | Existing `CASE` expressions handle it today. |
| NULL-WINS-PLAN | Maybe | Sentinel pattern works. Proper implementation deferred until PRECISION-LOSS lands. |
| SOURCE-REMOVAL-OPTIONS | Design | Validation-only; bridge-link tooling is additive. |
| TARGET-PATH-PLAN | Design | Explicitly recommends NOT implementing. Output formatting is a consumer concern. |
| YAML-VS-DSL-PLAN | Design | Analysis concluded: stay with YAML. No action needed. |

## Dependency graph

```
Phase 0 (examples/patterns)        ← COMPLETE (4/4)
    │
    ▼
Phase 1                            ← COMPLETE
    ├── PARENT-MAPPING-PLAN ✓
    └── EXPRESSION-SAFETY-PLAN ✓
            │
            ▼
Phase 2
    ├── PRECISION-LOSS-PLAN ──▶ unblocks: NULL-WINS (post-1.0)
    ├── CRDT-ORDERING-PLAN
    └── PASSTHROUGH-PLAN
            │
            ▼
Phase 3
    ├── TARGET-ARRAYS-PLAN ──▶ simplifies MULTI-VALUE pattern
    └── ANALYTICS-PROVENANCE-PLAN
            │
            ▼
Phase 4
    ├── CODE-QUALITY-PLAN (fmt + clippy + deny)
    ├── CODE-COVERAGE-PLAN
    ├── UNIT-TEST-PLAN
    ├── PROPTEST-PLAN
    ├── CI-RELEASE-PLAN
    ├── MATERIALIZED-VIEW-INDEX-PLAN
    ├── PGTRICKLE-OUTPUT-PLAN
    ├── LEARNING-GUIDE-PLAN ──▶ DOCS-SITE-PLAN
    └── NAMING-PLAN
            │
            ▼
        1.0 release
            │
            ▼
Post-1.0
    ├── COMPOSITE-TYPES-PLAN
    ├── SOURCE-GROUPING-PLAN
    ├── COMPUTED-FIELDS-PLAN (aggregation + traversal + missing-bottom example)
    ├── DBT-OUTPUT-PLAN
    ├── POLYGLOT-SQL-PLAN
    └── ...
```

## Summary

| Phase | Plans | Engine changes | Theme | Progress |
|-------|-------|---------------|-------|----------|
| 0 | 4 | 0 | Prove patterns with examples | **COMPLETE** |
| 1 | 2 | 2 | Lock the schema, secure expressions | **COMPLETE** |
| 2 | 3 | 3 | Precision, CRDT ordering, passthrough | Not started |
| 3 | 2 | 2 | Rich types and provenance | Not started |
| 4 | 11 | 1 | Quality, docs, CI/CD, naming, deployment | Not started |
| Post | 11 | — | Deferred or not implementing | — |
| **Total** | **30** | **9** | | |
