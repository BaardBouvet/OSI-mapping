# Plans

Design plans and architectural decision records for the OSI mapping engine.

| Plan | Status | Summary |
|------|--------|---------|
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
| [NAMING-PLAN.md](NAMING-PLAN.md) | Design | Recommends renaming to `osi-compiler` with binary `osic`. |
| [SOURCE-TYPES-PLAN.md](SOURCE-TYPES-PLAN.md) | Done | Source `fields:` with `type:` for PK casting; target field `type:` covers forward view. |
| [SOURCE-GROUPING-PLAN.md](SOURCE-GROUPING-PLAN.md) | Design | Visual grouping for related datasets in DOT output. |
| [SOURCE-REMOVAL-OPTIONS.md](SOURCE-REMOVAL-OPTIONS.md) | Design | Cluster split risk when mappings removed; mitigation strategy needed. |
| [JSON-FIELDS-PLAN.md](JSON-FIELDS-PLAN.md) | Done | `source_path` property for JSONB sub-field extraction with deep path support. |
| [COMPOSITE-TYPES-PLAN.md](COMPOSITE-TYPES-PLAN.md) | Proposed | Replace JSONB with PostgreSQL composite types for typed nested array output. |
| [PARENT-MAPPING-PLAN.md](PARENT-MAPPING-PLAN.md) | Planned | Unify `embedded` + `source.path` under `parent:` with `array`/`array_path` for nested arrays. |
| [HIERARCHY-MERGE-PLAN.md](HIERARCHY-MERGE-PLAN.md) | Planned | Example: merge 2-level and 3-level project hierarchies across systems. |
| [DEPTH-MISMATCH-PLAN.md](DEPTH-MISMATCH-PLAN.md) | Planned | Example: merge when one system has a deeper intermediate level than the other. |
| [MISSING-BOTTOM-PLAN.md](MISSING-BOTTOM-PLAN.md) | Planned | Example: merge when one system lacks the deepest level; `sql:` aggregation pattern. |
| [POSITIONAL-ARRAY-PLAN.md](POSITIONAL-ARRAY-PLAN.md) | Planned | Support `_index` for nested arrays without natural identity; uses `WITH ORDINALITY`. |
| [PROPAGATED-DELETE-PLAN.md](PROPAGATED-DELETE-PLAN.md) | Pattern | GDPR-style deletion propagation using regular target fields + `reverse_filter` — no engine changes. |
| [PRECISION-LOSS-PLAN.md](PRECISION-LOSS-PLAN.md) | Planned | `normalize` property on field mappings for lossy noop comparison (truncation, rounding, case folding). |
| [MULTI-VALUE-PLAN.md](MULTI-VALUE-PLAN.md) | Pattern | Cardinality mismatch (single vs. multi-value fields) — mapping patterns, no engine changes. |
| [EXPRESSION-SAFETY-PLAN.md](EXPRESSION-SAFETY-PLAN.md) | Planned | Validate expressions as safe SQL snippets; `lookup:` for cross-target access. |
| [TARGET-ARRAYS-PLAN.md](TARGET-ARRAYS-PLAN.md) | Planned | Array-typed fields on targets (`text[]`) — eliminates child targets for simple value lists. |
