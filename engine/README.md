# OSI Mapping Reference Engine

A reference implementation that compiles OSI mapping YAML files into a **DAG of PostgreSQL views**, implementing the full forward → resolution → reverse pipeline.

## Quick Start

```bash
# Validate mapping files (replaces Python validate.py)
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

The engine generates five layers of views for each mapping:

| Stage | View Pattern | Purpose |
|-------|-------------|---------|
| Forward | `_fwd_{mapping}` | Project source fields → target fields with expressions/filters |
| Identity | `_id_{target}` | Transitive closure for record linking |
| Resolution | `_resolved_{target}` | Merge contributions using conflict resolution strategies |
| Reverse | `_rev_{mapping}` | Project resolved target back to source shape |
| Delta | `_delta_{mapping}` | Compute updates/inserts/deletes vs original source |

## Development

### Prerequisites

- Rust stable (1.75+)
- Docker (for integration tests with testcontainers)

Or use the devcontainer (recommended):

```bash
# Open in VS Code with Dev Containers extension
code engine/
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

### Project Structure

```
src/
├── main.rs          CLI entry point (render, validate, dot)
├── lib.rs           Public API
├── model.rs         Strongly-typed mapping model (serde)
├── parser.rs        YAML → model deserialization
├── validate.rs      7-pass validator (replaces Python validate.py)
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

- [PLAN.md](PLAN.md) — Detailed implementation plan and design decisions
- [Mapping schema](../spec/mapping-schema.json) — The spec this engine implements
- [Examples](../examples/) — Test cases driving the implementation
