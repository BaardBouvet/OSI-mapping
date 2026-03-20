# CLI test command

**Status:** Proposed

Add an `osi-engine test` subcommand that executes embedded test cases from mapping files against a real PostgreSQL database, giving users a way to verify their mappings without the Rust toolchain.

## Motivation

Every example ships embedded `tests:` with input data and expected output (updates, inserts, deletes, noops, analytics). Today the only way to execute them is `cargo test --test integration execute_all_examples` — which requires the Rust toolchain, Docker, and the full dev-dependency stack. Users authoring mapping files have no way to run the tests they write.

The engine already validates test *structure* (`validate` pass 6 checks that datasets exist), but never executes the SQL or verifies actual output.

## Current infrastructure

The integration test in `tests/integration.rs` already implements the full pipeline:

1. Parse mapping → build DAG → render SQL
2. Spin up PostgreSQL via testcontainers (Docker)
3. Create source tables, cluster_members, written_state tables from test input
4. Execute rendered SQL (view creation)
5. Populate written_state from identity views
6. Query delta views → compare with `expected.updates`, `expected.inserts`, `expected.deletes`
7. Optionally verify `expected.noops` and `analytics`
8. Report pass/fail per test case

This logic lives entirely in `tests/integration.rs` (~1400 lines) and uses dev-dependencies: `tokio`, `tokio-postgres`, `testcontainers`, `testcontainers-modules`.

## Design

### CLI interface

```
osi-engine test <MAPPING> [OPTIONS]

Arguments:
  <MAPPING>   Path to mapping.yaml (or directory to discover all)

Options:
  --pg <URL>        PostgreSQL connection string
                    (default: spin up testcontainer)
  --schema <NAME>   Schema to use for isolation (default: random temp schema)
  --keep            Don't drop schema after test (for debugging)
  --filter <NAME>   Run only tests matching description substring
  -v, --verbose     Show SQL being executed and row-level diffs
  -q, --quiet       Only show pass/fail summary
```

**Directory mode**: `osi-engine test examples/` discovers all `mapping.yaml` files with `tests:` sections (same as `execute_all_examples`).

### Database strategy

Two modes:

1. **User-provided Postgres** (`--pg postgres://...`): Connect directly. Preferred for CI and users who already have a database.

2. **Testcontainer** (default, requires Docker): Spin up an ephemeral Postgres container. Same approach as integration tests today. Zero configuration.

Each test run creates a temporary schema (`_osi_test_{random}`) for isolation, runs everything inside it, and drops it on exit (unless `--keep`). This avoids polluting the user's database and allows parallel runs.

### Dependency changes

Move from dev-dependencies to regular dependencies (behind a feature flag):

```toml
[features]
default = ["test-runner"]
test-runner = ["tokio", "tokio-postgres", "testcontainers", "testcontainers-modules"]

[dependencies]
tokio = { version = "1", features = ["full"], optional = true }
tokio-postgres = { version = "0.7", features = ["with-serde_json-1"], optional = true }
testcontainers = { version = "0.23", optional = true }
testcontainers-modules = { version = "0.11", features = ["postgres"], optional = true }
```

The `test-runner` feature is on by default so the binary includes `test`. Users who only need `render`/`validate`/`dot` can disable it for a slimmer binary.

### Code organization

1. **New module `src/test_runner.rs`** — Extract the reusable test execution logic from `tests/integration.rs`:
   - `setup_pg()` → connection from URL or testcontainer
   - `load_test_data()` → create source tables + insert test rows
   - `ensure_cluster_members_tables()`, `ensure_written_state_tables()`, `ensure_source_columns()`
   - `populate_written_state_tables()`
   - `verify_test_expected()` → compare delta views against expected
   - `verify_analytics()` → compare analytics views against expected
   - `TestResult` struct with pass/fail/skip per test case

2. **Update `src/main.rs`** — Add `Test` variant to the clap `Commands` enum.

3. **Simplify `tests/integration.rs`** — Replace duplicated logic with calls to `osi_engine::test_runner`. The integration test becomes a thin wrapper.

### Output format

Default human-readable output matching current integration test style:

```
============================================================
  Example: hello-world
============================================================
  --- Test 1: basic coalesce ---
  crm_contacts: 2 updates match ✓
  erp_contacts: 1 update match ✓
  ✓ test 1

===== SUMMARY =====
Passed:  1/1
  ✓ hello-world
```

Exit code: 0 if all pass, 1 if any fail.

Future: `--format json` or `--format tap` for machine consumption.

### Test isolation flow

```
1. Connect to Postgres (URL or testcontainer)
2. CREATE SCHEMA _osi_test_{random}
3. SET search_path TO _osi_test_{random}
4. For each mapping file:
   a. Parse + validate + render SQL
   b. For each test case:
      i.   DROP all views (reverse topological order)
      ii.  Create source tables from test.input
      iii. Execute rendered SQL (create views)
      iv.  Populate written_state
      v.   Query delta views → compare with test.expected
      vi.  Query analytics views → compare with test.analytics
      vii. Record pass/fail
5. DROP SCHEMA _osi_test_{random} CASCADE (unless --keep)
6. Print summary, exit with appropriate code
```

## Implementation steps

1. Add `test-runner` feature flag and move dependencies in `Cargo.toml`
2. Create `src/test_runner.rs` — extract logic from `tests/integration.rs` into a public module
3. Add `Test` command to `src/main.rs` with clap args
4. Wire up `main.rs` → `test_runner::run()`
5. Refactor `tests/integration.rs` to use `osi_engine::test_runner` (remove duplication)
6. Test: `cargo run -- test ../examples/hello-world/mapping.yaml`
7. Test: `cargo run -- test ../examples/` (directory discovery)

## Scope boundaries

- No new test syntax — uses existing `tests:` schema as-is.
- No embedded database (SQLite etc.) — PostgreSQL is the target and the tests embed PG-specific SQL.
- No watch mode in v1 — run once, report, exit.
- No parallel test execution in v1 — sequential is simpler and avoids schema collision concerns.
