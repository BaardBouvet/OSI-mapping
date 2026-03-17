# OSI Mapping Reference Engine

[![CI](https://github.com/osi-project/osi-mapping/actions/workflows/ci.yml/badge.svg)](https://github.com/osi-project/osi-mapping/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/osi-project/osi-mapping/graph/badge.svg)](https://codecov.io/gh/osi-project/osi-mapping)

A reference implementation that compiles OSI mapping YAML files into a **DAG of PostgreSQL views**, implementing the full forward → resolution → reverse pipeline.

## Quick Start

```bash
# Validate mapping files
cargo run -- validate ../examples/
cargo run -- validate ../examples/hello-world/mapping.yaml -v

# Render a mapping to SQL
cargo run -- render ../examples/hello-world/mapping.yaml

# Output to file
cargo run -- render ../examples/hello-world/mapping.yaml -o views.sql

# Visualize the view DAG
cargo run -- dot ../examples/hello-world/mapping.yaml | dot -Tpng -o dag.png
```

## Pipeline Stages

The engine generates views in two branches from resolution:

| Stage | View Pattern | Purpose |
|-------|-------------|---------|
| Forward | `_fwd_{mapping}` | Project source fields → target fields with expressions/filters |
| Identity | `_id_{target}` | Transitive closure for record linking |
| Resolution | `_resolved_{target}` | Merge contributions using conflict resolution strategies |
| Analytics | `{target}` | Clean golden record for BI consumers (always) |
| Reverse | `_rev_{mapping}` | Project resolved target back to source shape (opt-in) |
| Delta | `_delta_{mapping}` | Compute updates/inserts/deletes vs original source (opt-in) |

Analytics views are always generated. Reverse and delta views are opt-in per mapping via `sync: true`.

## Development

### Prerequisites

- Rust stable (1.75+)
- Docker (for integration tests with testcontainers)

Or use the devcontainer (recommended):

```bash
# Open in VS Code with Dev Containers extension
code engine-rs/
# Then: Ctrl+Shift+P → "Reopen in Container"
```

### Running Tests

```bash
# Unit tests (no Docker needed)
cargo test --lib

# All tests including integration (requires Docker)
cargo test

# Parse-only smoke test
cargo test parse_all_examples
```

### Code Coverage

```bash
# Install cargo-llvm-cov (one time)
cargo install cargo-llvm-cov

# Quick summary (lib tests only, no Docker)
cargo llvm-cov --lib

# Full coverage including integration tests
cargo llvm-cov --test integration --lib

# HTML report (opens in browser)
cargo llvm-cov --test integration --lib --html --open
```

### Project Structure

```
src/
├── main.rs          CLI entry point (render, validate, dot)
├── lib.rs           Public API
├── model.rs         Strongly-typed mapping model (serde)
├── parser.rs        YAML → model deserialization
├── validate.rs      11-pass semantic validator
├── validate_expr.rs Expression safety & column reference validation
├── dag.rs           View dependency graph
├── error.rs         Error types
└── render/
    ├── mod.rs       SQL orchestrator
    ├── forward.rs   Forward view generation
    ├── identity.rs  Transitive closure
    ├── resolution.rs Conflict resolution
    ├── reverse.rs   Reverse projection
    └── delta.rs     Changeset computation
```

## See Also

- [docs/](docs/) — Design decisions, view pipeline documentation
- [PLAN.md](PLAN.md) — Detailed implementation plan and design decisions
- [Mapping schema](../spec/mapping-schema.json) — The spec this engine implements
- [Examples](../examples/) — Test cases driving the implementation
