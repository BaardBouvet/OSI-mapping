# Plans

Design plans and architectural decision records for the OSI mapping engine.

| Plan | Status | Summary |
|------|--------|---------|
| [PLAN.md](PLAN.md) | Done | Original implementation plan — Rust engine compiling YAML to a DAG of PostgreSQL views. |
| [PRIMARY-KEYS-PLAN.md](PRIMARY-KEYS-PLAN.md) | Done | Replace synthetic `_row_id` with real source primary keys via `sources:` section. |
| [ANALYTICS-VIEW-PLAN.md](ANALYTICS-VIEW-PLAN.md) | Done | Consumer-friendly analytics view exposing resolved golden records. |
| [ORIGIN-PLAN.md](ORIGIN-PLAN.md) | Done | Origin tracking and insert feedback to prevent duplicate inserts. |
| [VIEW-CONSOLIDATION-PLAN.md](VIEW-CONSOLIDATION-PLAN.md) | Partial | Merge reverse+delta, CTE inlining, naming — changes 1-3 reverted for debuggability; change 4 (naming) kept. |
| [DIAMOND-AVOIDANCE-PLAN.md](DIAMOND-AVOIDANCE-PLAN.md) | Done | Analysis of the reverse view's diamond dependency — accepted and documented. |
| [FORWARD-VIEWS-PLAN.md](FORWARD-VIEWS-PLAN.md) | Done | Restored separate forward views for debuggability and rollout. |
| [SOURCE-REMOVAL-OPTIONS.md](SOURCE-REMOVAL-OPTIONS.md) | Design | Analysis of source removal impact on clusters — validation warnings + bridge-link generation. |
| [FK-REFERENCES-PLAN.md](FK-REFERENCES-PLAN.md) | Done | Explicit `references:` on field mappings for FK reverse resolution. Replaces LCP heuristic. |
| [REFERENCE-HEURISTIC-PLAN.md](REFERENCE-HEURISTIC-PLAN.md) | Superseded | LCP heuristic for same-system mapping detection — replaced by [FK-REFERENCES-PLAN](FK-REFERENCES-PLAN.md). |
| [COMPOSITE-KEY-REFS-PLAN.md](COMPOSITE-KEY-REFS-PLAN.md) | Design | PK-reference field limitation when a source PK column is also mapped to a reference field. |
| [NAMING-PLAN.md](NAMING-PLAN.md) | Design | Recommends renaming to `osi-compiler` with binary `osic`. |
| [SOURCE-TYPES-PLAN.md](SOURCE-TYPES-PLAN.md) | Proposed | Optional `sql_type` on field mappings for type-aware forward views. |
| [TEST-PROGRESS-PLAN.md](TEST-PROGRESS-PLAN.md) | In Progress | Generic `execute_all_examples` test runner with pass/fail/skip summary. |
