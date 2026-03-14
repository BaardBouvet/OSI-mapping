# OSI Mapping Reference Engine — Implementation Plan

## Overview

A reference engine that compiles an OSI mapping YAML file into a **DAG of PostgreSQL views**,
implementing the full forward → resolution → reverse pipeline described by the spec.
Written in **Rust** for fast rendering. Includes a **Rust reimplementation of the validator**
(replacing the Python `validate.py`) with all 7 validation passes.

Development uses **devcontainers** (for reproducible tooling) and **testcontainers** (for
ephemeral PostgreSQL instances in tests). The 35+ existing examples drive the implementation
as integration tests.

---

## Architecture

```
mapping.yaml
     │
     ▼
┌──────────────┐     ┌──────────────────────────────┐
│  Rust Engine  │────▶│  DAG of PostgreSQL VIEWs/CTEs│
│  (osi-engine) │     └──────────────────────────────┘
└──────────────┘
     │
     ▼
  SQL script (stdout / file)
  osi-engine validate   ← replaces Python validate.py```

### Pipeline Stages (each becomes one or more views)

```
Source tables (external)
  │
  ├─ 1. Forward views    — per-mapping: filter + field expressions + nested array unnest
  │
  ├─ 2. Identity views   — per-target: transitive closure over identity/link_group fields
  │
  ├─ 3. Resolution views — per-target: merge contributions using strategy (coalesce/last_modified/expression/collect)
  │                         group-aware atomic resolution; default/default_expression fallback
  │
  ├─ 4. Reverse views    — per-mapping: resolved target → source shape
  │                         reverse_expression, reverse_filter, reverse_required, include_base
  │                         FK translation via references
  │
  └─ 5. Delta views      — per-mapping: diff reverse vs original source → updates/inserts/deletes
```

### View Naming Convention

```
_fwd_{mapping_name}          — forward projection
_id_{target_name}            — identity / transitive closure
_resolved_{target_name}      — merged golden record
_rev_{mapping_name}          — reverse projection
_delta_{mapping_name}        — final change set (updates/inserts/deletes)
```

---

## Directory Structure

```
engine/
├── PLAN.md                   ← this file
├── Cargo.toml                ← Rust workspace root
├── src/
│   ├── main.rs               ← CLI entry point (render, validate, dot)
│   ├── lib.rs                ← public API
│   ├── parser.rs             ← YAML → internal model (serde)
│   ├── model.rs              ← strongly-typed IR (targets, mappings, fields, tests)
│   ├── validate.rs           ← 7-pass validator (replaces Python validate.py)
│   ├── dag.rs                ← dependency graph builder
│   ├── render/
│   │   ├── mod.rs            ← SQL rendering orchestrator
│   │   ├── forward.rs        ← forward view generation
│   │   ├── identity.rs       ← transitive closure view generation
│   │   ├── resolution.rs     ← resolution view generation
│   │   ├── reverse.rs        ← reverse view generation
│   │   └── delta.rs          ← delta/changeset view generation
│   └── error.rs              ← error types
├── tests/
│   ├── integration.rs        ← testcontainers harness: load example → render → execute → compare
│   └── snapshots/            ← (optional) SQL snapshot tests
├── .devcontainer/
│   ├── devcontainer.json     ← devcontainer config (Rust + Python + Postgres client)
│   └── Dockerfile            ← custom image if needed
└── README.md                 ← usage + development docs
```

### Python Validator Replacement

The existing `validation/validate.py` is replaced by `osi-engine validate`, which
reimplements all 7 validation passes in Rust:

| Pass | Description | Implementation |
|------|-------------|----------------|
| 1 | Schema / structural | serde deserialization + naming regex checks |
| 2 | Unique names | HashMap counting |
| 3 | Target references | HashSet lookups |
| 4 | Strategy consistency | Contribution index analysis |
| 5 | Field coverage | Contributed set vs target field set |
| 6 | Test datasets | Source dataset set matching |
| 7 | SQL syntax | Parenthesis/quote balancing (no external SQL parser dep) |

CLI usage mirrors the Python tool:
```bash
osi-engine validate                          # all examples
osi-engine validate path/to/mapping.yaml     # single file
osi-engine validate path/to/dir/             # all mapping.yaml in dir
osi-engine validate -v                       # show warnings
osi-engine validate -q                       # only show failures
```

---

## Implementation Phases

### Phase 1 — Project Setup & Parser

**Goal:** Devcontainer, Rust project skeleton, YAML deserialization, model types.

1. Create `.devcontainer/devcontainer.json` with:
   - Rust toolchain (stable)
   - Python 3.11+ with pip
   - PostgreSQL client (`psql`, `libpq`)
   - Docker-in-Docker (for testcontainers)
2. Initialize `Cargo.toml` with dependencies:
   - `serde`, `serde_yaml` — YAML parsing
   - `clap` — CLI
   - `testcontainers` — ephemeral Postgres in tests
   - `tokio-postgres` or `sqlx` — SQL execution in tests
   - `anyhow`, `thiserror` — error handling
3. Define `model.rs` — Rust structs mirroring the JSON schema:
   - `MappingDocument`, `Target`, `TargetField`, `TargetFieldDef`
   - `Mapping`, `FieldMapping`, `SourceRef`, `TestCase`
   - Enums: `Strategy`, `Direction`
4. Implement `parser.rs` — deserialize YAML to model with serde
5. Unit tests: parse every example YAML without error

### Phase 2 — DAG Builder & Forward Views

**Goal:** Build the dependency graph; generate forward mapping views.

1. `dag.rs` — Topological sort of view generation order:
   - Forward views have no dependencies (just source tables)
   - Identity views depend on forward views
   - Resolution views depend on identity views
   - Reverse views depend on resolution views
   - Delta views depend on reverse views + source tables
2. `render/forward.rs` — For each mapping, emit:
   ```sql
   CREATE VIEW _fwd_{name} AS
   SELECT {field_expressions}
   FROM {source_dataset}
   WHERE {filter}  -- if present
   ```
   Handle:
   - Simple field copies (`source: x, target: y` → `x AS y`)
   - Expression transforms (`expression: "UPPER(name)"` → `UPPER(name) AS full_name`)
   - Forward-only computed fields (no source → expression only)
   - Nested arrays via `LATERAL jsonb_array_elements(...)` or similar unnest
   - `parent_fields` import from parent scope
   - Direction filtering (skip `reverse_only` fields)

### Phase 3 — Identity & Resolution Views

**Goal:** Transitive closure for record linking; conflict resolution.

1. `render/identity.rs` — Per target entity:
   - Collect identity contributions from all forward views
   - Handle `link_group` (composite tuple matching)
   - Handle multiple independent identity fields (OR-based linking)
   - Generate transitive closure using recursive CTE:
     ```sql
     WITH RECURSIVE id_closure AS (
       SELECT ... UNION ALL SELECT ...
     )
     CREATE VIEW _id_{target} AS SELECT ...
     ```
2. `render/resolution.rs` — Per target entity:
   - Join forward views via identity closure
   - Apply per-field resolution strategy:
     - `identity` / `collect` → `array_agg(DISTINCT ...)` or `string_agg(...)`
     - `coalesce` → `FIRST_VALUE(...) OVER (ORDER BY priority)`
     - `last_modified` → `FIRST_VALUE(...) OVER (ORDER BY timestamp DESC)`
     - `expression` → custom SQL aggregation expression
   - Handle `group` — atomic resolution (all fields in group from same winning source)
   - Apply `default` / `default_expression` with COALESCE fallback
   - Handle `references` — FK values resolve through the referenced target's identity

### Phase 4 — Reverse & Delta Views

**Goal:** Map resolved records back to source shape; compute changesets.

1. `render/reverse.rs` — Per mapping:
   - Select from `_resolved_{target}`
   - Apply `reverse_expression` transforms
   - Apply `reverse_filter` condition
   - Handle `reverse_required` — CASE/WHERE to exclude null rows
   - Handle `include_base` — join original source to add `_base_` prefixed columns
   - Handle `direction` — skip `forward_only` fields
   - Handle FK translation via `references` (join identity view to translate entity IDs
     back to source-native IDs)
   - Re-nest arrays if source used `path` (aggregate back into JSON arrays)
2. `render/delta.rs` — Per mapping:
   - FULL OUTER JOIN reverse view with original source
   - Classify rows:
     - **updates**: matched rows with at least one changed field
     - **inserts**: rows in reverse but not in source (new records from other sources)
     - **deletes**: rows in source but not in reverse (excluded by reverse_required/filter)

### Phase 5 — CLI & SQL Output

**Goal:** Complete CLI that reads YAML and emits SQL.

1. `main.rs` — CLI with clap:
   ```
   osi-engine render <mapping.yaml>           → SQL to stdout
   osi-engine render <mapping.yaml> -o out.sql → SQL to file
   osi-engine dot <mapping.yaml>              → GraphViz DOT of the DAG
   ```
2. Emit well-commented SQL with clear stage separation
3. Wrap entire output in a transaction with `SET search_path`

### Phase 6 — Integration Tests with Testcontainers

**Goal:** Validate correctness using example test cases.

1. Test harness in `tests/integration.rs`:
   - For each example with `tests:` section:
     a. Start ephemeral Postgres via testcontainers
     b. Create source tables from test input data
     c. Render mapping to SQL views
     d. Execute SQL to create views
     e. Query delta views
     f. Compare actual output with expected updates/inserts/deletes
2. Test discovery: glob `../examples/*/mapping.yaml`, filter to those with tests
3. Handle edge cases:
   - Type coercion (YAML strings vs. Postgres types)
   - Row ordering (sort for comparison)
   - Null handling

### Phase 7 — End-to-End Engine Execution Tests

**Goal:** Validate full pipeline correctness via testcontainers.

1. For each example with `tests:` section:
   a. Start ephemeral Postgres via testcontainers
   b. Create source tables from test input data
   c. Render mapping to SQL views
   d. Execute SQL to create views
   e. Query delta views
   f. Compare actual output with expected updates/inserts/deletes
2. Optionally expose as `osi-engine test <mapping.yaml>` CLI command

---

## Key Design Decisions

### Why a DAG of Views (not procedural ETL)?

- **Declarative** — mirrors the spec's philosophy; Postgres optimizes the plan
- **Inspectable** — users can query any intermediate view to debug
- **Composable** — views reference each other; Postgres resolves dependencies
- **Testable** — load data, create views, query results

### Why Transitive Closure via Recursive CTE?

- PostgreSQL natively supports `WITH RECURSIVE`
- Handles arbitrary chains of identity links
- No external graph library needed at runtime

### Why Testcontainers (not a mock)?

- Tests validate actual SQL execution against real Postgres
- Catches SQL dialect issues, type mismatches, and edge cases
- Each test gets an isolated database — no cross-contamination

---

## Phase 8 — Real Primary Keys

See [PRIMARY-KEYS-PLAN.md](PRIMARY-KEYS-PLAN.md).

Summary: add `sources:` section with `primary_key` declarations. Replace synthetic
`_row_id` with real PKs. Single PKs become `pk::text AS _src_id`; composite PKs
become `jsonb_build_object(...)::text AS _src_id`. Backward compatible — omitting
`sources:` falls back to `_row_id`.

### SQL Generation Strategy

- Generate **one SQL script** with all views in dependency order
- Each view is a `CREATE OR REPLACE VIEW` (idempotent)
- Source tables are assumed to exist (engine does not create them)
- In test mode, we create source tables with appropriate types from test input data

---

## Dependencies

### Rust Crates

| Crate | Purpose |
|-------|---------|
| `serde` + `serde_yaml` | YAML deserialization |
| `clap` | CLI argument parsing |
| `anyhow` | Error propagation |
| `thiserror` | Custom error types |
| `testcontainers` | Ephemeral Postgres for tests |
| `testcontainers-modules` | Postgres module for testcontainers |
| `tokio` | Async runtime (for testcontainers + Postgres) |
| `tokio-postgres` | Postgres client for tests |
| `serde_json` | JSON handling for nested array support |
| `regex` | Naming convention validation |

### Python (no longer needed)

The Python validator is fully replaced by the Rust implementation.
The original `validation/validate.py` is preserved for reference but
`osi-engine validate` is the canonical tool.

---

## Test Strategy

### Unit Tests (Rust)

- **Parser tests**: Every example YAML deserializes without error
- **Model tests**: Enum round-trips, shorthand normalization
- **Render tests**: Individual SQL fragments are syntactically correct (snapshot tests)
- **DAG tests**: Dependency ordering is correct

### Integration Tests (Rust + Testcontainers)

- **Example-driven**: Each example with `tests:` section becomes an integration test
- **Pipeline**: parse → render → execute → compare
- **Coverage target**: All 35+ examples parse; all examples with tests pass

### Validation Tests (Rust)

- All 36 examples parse and validate with 0 errors
- Unit tests for each error detection case (duplicate names, invalid refs, etc.)
- Warning parity with Python validator's semantic checks
- CLI output matches the Python validator's format

---

## Milestone Checklist

- [ ] Phase 1: Project setup, devcontainer, parser, model ✅
- [ ] Phase 2: DAG builder, forward view rendering ✅
- [ ] Phase 3: Identity (transitive closure) and resolution views
- [ ] Phase 4: Reverse projection and delta views
- [ ] Phase 5: CLI and SQL output ✅
- [ ] Phase 6: Integration tests against all examples
- [ ] Phase 7: End-to-end engine execution tests
- [x] Validator: All 7 passes reimplemented in Rust ✅

---

## Open Questions / Risks

1. **Nested array representation** — Should source tables store nested arrays as
   `JSONB` columns, or should we pre-flatten with separate tables? JSONB + `jsonb_array_elements`
   is more faithful to the spec but may complicate type handling.
   → **Decision: Use JSONB** for nested data; it matches the spec's hierarchical model.

2. **Transitive closure performance** — Recursive CTEs can be slow for large datasets,
   but this is a reference implementation (correctness over performance). Production
   engines can use Union-Find or materialized views.

3. **SQL dialect** — Target PostgreSQL 15+. Use standard SQL where possible; PG-specific
   features (`jsonb_array_elements`, `string_agg`, `WITH RECURSIVE`) where needed.

4. **Type inference** — Test input data is YAML (strings/numbers). Need a type inference
   strategy for creating source tables. Start with `TEXT` for everything, then refine
   based on field usage (timestamps, integers, etc.).
   → **Decision: Use TEXT** as the default; explicitly cast in expressions. This avoids
   type mismatch errors and matches the loosely-typed nature of integration data.

---

## Phase 9 — Insert Origin & Feedback

See [ORIGIN-PLAN.md](ORIGIN-PLAN.md).

Summary: the engine emits `_cluster_id` (deterministic hash) on delta insert rows
and an optional provenance view. All origin tracking, cluster management, and
generated-ID feedback is ETL runtime state — not the engine's concern.
