# CI/CD, packaging, and release

**Status:** Planned

Set up GitHub Actions for continuous integration, binary packaging, and
release automation so users can install the engine without building from
source.

## Problem

Today the only way to use the engine is `cargo build` from source. There
is no CI pipeline — no automated testing on push, no pre-built binaries,
no crate publication. Contributors have no confidence that PRs don't break
existing tests, and users must have a full Rust toolchain to try the project.

## Goals

1. **CI on every push and PR** — compile, lint, test, catch regressions.
2. **Pre-built binaries** — downloadable for Linux, macOS, Windows.
3. **Crate publication** — `cargo install osi-engine` just works.
4. **Automated releases** — tag a version, binaries and crate publish
   automatically.

## CI pipeline

### Workflow: `ci.yml`

Triggered on push to `main` and all PRs.

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --check
      - run: cargo clippy -- -D warnings
      - run: cargo test --lib
      - run: cargo doc --no-deps

  integration:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_PASSWORD: postgres
        ports: ['5432:5432']
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --test integration -- --nocapture
        env:
          DATABASE_URL: postgres://postgres:postgres@localhost:5432/postgres
```

**Note:** The integration tests currently use testcontainers which starts
its own Postgres. Two options:

- **Option A:** Keep testcontainers in CI — requires Docker-in-Docker or a
  Docker socket on the runner. GitHub Actions runners have Docker available.
- **Option B:** Switch to the `services:` Postgres above and add a
  `DATABASE_URL` env var fallback in the test harness. Faster startup, no
  Docker pull on every run.

Recommendation: **Option A** initially (zero test code changes), migrate to
Option B if CI speed becomes an issue.

## Packaging

### Pre-built binaries

Use [`cargo-dist`](https://opensource.axo.dev/cargo-dist/) or
[`cross`](https://github.com/cross-rs/cross) + manual release workflow:

| Target | OS | Notes |
|--------|-----|-------|
| `x86_64-unknown-linux-gnu` | Linux x64 | Primary target |
| `x86_64-unknown-linux-musl` | Linux x64 static | No glibc dependency |
| `aarch64-unknown-linux-gnu` | Linux ARM64 | Graviton / Apple Silicon VMs |
| `x86_64-apple-darwin` | macOS Intel | |
| `aarch64-apple-darwin` | macOS Apple Silicon | |
| `x86_64-pc-windows-msvc` | Windows x64 | |

### Recommended: cargo-dist

`cargo-dist` generates the release workflow automatically:

```bash
cargo install cargo-dist
cargo dist init
```

This adds a `release.yml` workflow that:
1. Triggers on pushed version tags (`v*`)
2. Cross-compiles for all configured targets
3. Creates a GitHub Release with downloadable archives
4. Generates shell/PowerShell installer scripts
5. Optionally publishes to crates.io

### Crate publication

Add metadata to `Cargo.toml`:

```toml
[package]
name = "osi-engine"
version = "0.1.0"
edition = "2021"
description = "Reference engine for OSI mapping spec"
license = "MIT"
repository = "https://github.com/OWNER/osi-mapping"
readme = "README.md"
keywords = ["mapping", "integration", "postgresql", "entity-resolution"]
categories = ["database", "command-line-utilities"]
```

Publish via `cargo publish` (manual or automated in release workflow).

## Release process

### Versioning

Follow [Semantic Versioning](https://semver.org/):
- **0.x.y** — pre-1.0, breaking changes bump minor
- **1.0.0** — schema and CLI interface stable
- **1.x.y** — backward-compatible additions bump minor, fixes bump patch

### Tagging

```bash
# Bump version in Cargo.toml, commit
cargo dist plan          # preview what will be built
git tag v0.2.0
git push origin v0.2.0   # triggers release workflow
```

### Changelog

Maintain a `CHANGELOG.md` in the repo root. Each release section lists:
- Added / Changed / Fixed / Removed
- Links to relevant plan files for context

## Implementation

### Phase 1 — CI basics

1. Create `.github/workflows/ci.yml` (fmt + clippy + unit tests)
2. Add integration test job (with testcontainers / Docker)
3. Verify all 42 examples pass in CI

### Phase 2 — Release automation

1. Run `cargo dist init` to generate release workflow
2. Configure target triples
3. Add `CARGO_REGISTRY_TOKEN` secret for crates.io
4. Tag and release `v0.1.0` as first published version

### Phase 3 — Polish

1. Add `CHANGELOG.md`
2. Add CI badge to README
3. Consider Homebrew formula / AUR package for popular package managers
