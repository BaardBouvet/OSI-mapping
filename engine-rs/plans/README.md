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
| [JSON-FIELDS-PLAN.md](JSON-FIELDS-PLAN.md) | Design | JSON sub-field extraction/writing for structured source columns. |
| [COMPOSITE-TYPES-PLAN.md](COMPOSITE-TYPES-PLAN.md) | Proposed | Replace JSONB with PostgreSQL composite types for typed nested array output. |
