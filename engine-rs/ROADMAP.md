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
| EXPRESSION-SAFETY-PLAN | ~~Planned~~ **Done** | Expression validation (static + AST check). Cross-target `lookup:` superseded by COMPUTED-FIELDS-PLAN. |

**Exit criteria:** ~~All 35+ examples pass with the new schema.~~ All 39 examples pass. Expression
validator rejects known-bad inputs. No mapping uses internal view names in
expressions.

## Phase 2 — Core improvements

Engine features that improve correctness and expand what mappings can express,
without changing the schema surface locked in Phase 1.

| Plan | Status | Work |
|------|--------|------|
| PRECISION-LOSS-PLAN | Planned | `normalize:` property on field mappings for lossy noop comparison (Phase 1 only: truncation, rounding, case folding). |
| CRDT-ORDERING-PLAN | ~~Planned~~ **Done** | `order: true` + optional prev/next CRDT links for nested array ordering; supersedes POSITIONAL-ARRAY-PLAN. |
| PASSTHROUGH-PLAN | Planned | `passthrough:` list on mappings to carry unmapped columns to delta output. |

**Exit criteria:** New examples for each feature. Noop suppression correct for
normalized fields. Ordered arrays round-trip through reverse views.

## Phase 3 — Richer types and output

Larger features that expand the type system and analytics layer.

| Plan | Status | Work |
|------|--------|------|
| ANALYTICS-PROVENANCE-PLAN | Planned | `_provenance_` and `_contributions_` views for source-tracing and stewardship. |

**Exit criteria:** `_provenance_` and `_contributions_` views render correctly and are covered by an example.

## Phase 4 — Quality, docs, and 0.1 release

Hardening, documentation, CI/CD, and project identity before the 1.0 tag.

| Plan | Status | Work |
|------|--------|------|
| CODE-QUALITY-PLAN | ~~Planned~~ **Done** | Enforce rustfmt + clippy + cargo-deny; one-time codebase cleanup. |
| CODE-COVERAGE-PLAN | ~~Planned~~ **Done** | cargo-llvm-cov + Codecov; discover untested paths. |
| UNIT-TEST-PLAN | ~~Planned~~ **Done** | Unit tests for render pipeline; reduce integration test reliance. |
| PROPTEST-PLAN | ~~Planned~~ **Done** | Property-based fuzzing: random mapping generation, structural + execution phases. |
| CI-RELEASE-PLAN | Planned | GitHub Actions CI/CD, pre-built binaries via cargo-dist, crate publication. |
| MATERIALIZED-VIEW-INDEX-PLAN | ~~Design~~ **Done** | Opt-in `--materialize` flag with unique indexes for production deployments. |
| LEARNING-GUIDE-PLAN | Planned | Progressive 7-chapter learning guide teaching mapping concepts. |
| DOCS-SITE-PLAN | ~~Planned~~ **Done** | mdBook documentation site with search, deployed to GitHub Pages. |
| NAMING-PLAN | Design | Rename project (recommended: "Crossfold"). Update crate, binary, repo, docs. |

**Exit criteria:** CI pipeline green on every push. Pre-built binaries on
GitHub Releases. Documentation site live. Proptest harness runs in CI.
Project name settled and applied across all artifacts.

## Post-0.1

Deferred plans live in `engine-rs/plans/`. See individual plan files for
rationale and status.

