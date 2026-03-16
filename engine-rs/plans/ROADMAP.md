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
| PROPAGATED-DELETE-PLAN | Pattern | Example: GDPR deletion cascading via `bool_or` + `reverse_filter`. |
| MULTI-VALUE-PLAN | Pattern | Example: single-vs-multi-value cardinality mismatch. |
| HIERARCHY-MERGE-PLAN | Planned | Example: merging 2-level and 3-level hierarchies. |
| DEPTH-MISMATCH-PLAN | Planned | Example: asymmetric nesting depth across systems. |
| MISSING-BOTTOM-PLAN | Planned | Example: aggregation when one system lacks the leaf level. |

**Exit criteria:** Five new examples passing E2E tests.

## Phase 1 — Schema and safety

The two changes most likely to break existing mappings. Land them together so
users migrate once.

| Plan | Status | Work |
|------|--------|------|
| PARENT-MAPPING-PLAN | Planned | Replace `embedded` + `source.path` with `parent:` and `array:`. Major schema change — rewrite all affected examples. |
| EXPRESSION-SAFETY-PLAN | Planned | Validate expressions as safe SQL snippets. Add `lookup:` for cross-target access. Phase 1 only (static validation + AST check). |

**Dependencies:** PARENT-MAPPING-PLAN touches nested-array examples.
EXPRESSION-SAFETY-PLAN informs how future expression-heavy plans are written.

**Exit criteria:** All 35+ examples pass with the new schema. Expression
validator rejects known-bad inputs. No mapping uses internal view names in
expressions.

## Phase 2 — Core improvements

Engine features that improve correctness and expand what mappings can express,
without changing the schema surface locked in Phase 1.

| Plan | Status | Work |
|------|--------|------|
| PRECISION-LOSS-PLAN | Planned | `normalize:` property on field mappings for lossy noop comparison (Phase 1 only: truncation, rounding, case folding). |
| POSITIONAL-ARRAY-PLAN | Planned | `_index` identity for nested arrays without natural keys; `WITH ORDINALITY` in forward views. |
| PASSTHROUGH-PLAN | Planned | `passthrough:` list on mappings to carry unmapped columns to delta output. |

**Exit criteria:** New examples for each feature. Noop suppression correct for
normalized fields. Positional arrays round-trip through reverse views.

## Phase 3 — Richer types and output

Larger features that expand the type system and analytics layer.

| Plan | Status | Work |
|------|--------|------|
| TARGET-ARRAYS-PLAN | Planned | Array-typed target fields (`text[]`, `integer[]`). Eliminates child targets for simple value lists. Full pipeline impact. |
| COMPOSITE-TYPES-PLAN | Proposed | Replace JSONB nested-array output with PostgreSQL composite types in delta/analytics views. |
| ANALYTICS-PROVENANCE-PLAN | Planned | `_provenance_` and `_contributions_` views for source-tracing and stewardship. |

**Exit criteria:** Array fields work in forward, identity, resolution, reverse,
and delta views. Composite-type output optional and backward-compatible.

## Phase 4 — Quality and project

Hardening, testing, and project identity before the 1.0 tag.

| Plan | Status | Work |
|------|--------|------|
| PROPTEST-PLAN | Planned | Property-based fuzzing: random mapping generation, structural + execution phases. |
| NAMING-PLAN | Design | Rename project (recommended: "Crossfold"). Update crate, binary, repo, docs. |
| SOURCE-GROUPING-PLAN | Design | `system:` property on sources for visual DOT grouping. |

**Exit criteria:** Proptest harness runs in CI. Project name settled and
applied across all artifacts.

## Post-1.0

Plans that have workarounds today, are explicitly deferred, or require more
design. They may ship as 1.x minor releases.

| Plan | Status | Reason deferred |
|------|--------|-----------------|
| COMPUTED-FIELDS-PLAN | Design | Depends on EXPRESSION-SAFETY; only analytics layer. Ship as 1.x. |
| TYPE-HIERARCHY-PLAN | Design | Existing `CASE` expressions handle it today. |
| NULL-WINS-PLAN | Maybe | Sentinel pattern works. Proper implementation deferred until PRECISION-LOSS lands. |
| SOURCE-REMOVAL-OPTIONS | Design | Validation-only; bridge-link tooling is additive. |
| TARGET-PATH-PLAN | Design | Explicitly recommends NOT implementing. Output formatting is a consumer concern. |

## Dependency graph

```
Phase 0 (examples/patterns)
    │
    ▼
Phase 1
    ├── PARENT-MAPPING-PLAN ──▶ unblocks: nested-array examples in Phase 0
    └── EXPRESSION-SAFETY-PLAN ──▶ unblocks: COMPUTED-FIELDS (post-1.0)
            │
            ▼
Phase 2
    ├── PRECISION-LOSS-PLAN ──▶ unblocks: NULL-WINS (post-1.0)
    ├── POSITIONAL-ARRAY-PLAN
    └── PASSTHROUGH-PLAN
            │
            ▼
Phase 3
    ├── TARGET-ARRAYS-PLAN ──▶ simplifies MULTI-VALUE pattern
    ├── COMPOSITE-TYPES-PLAN
    └── ANALYTICS-PROVENANCE-PLAN
            │
            ▼
Phase 4
    ├── PROPTEST-PLAN
    ├── NAMING-PLAN
    └── SOURCE-GROUPING-PLAN
            │
            ▼
        1.0 release
```

## Summary

| Phase | Plans | Engine changes | Theme |
|-------|-------|---------------|-------|
| 0 | 5 | 0 | Prove patterns with examples |
| 1 | 2 | 2 | Lock the schema, secure expressions |
| 2 | 3 | 3 | Precision, positional identity, passthrough |
| 3 | 3 | 3 | Rich types and provenance |
| 4 | 3 | 1 | Quality, naming, polish |
| Post | 5 | — | Deferred or not implementing |
| **Total** | **21** | **9** | |
