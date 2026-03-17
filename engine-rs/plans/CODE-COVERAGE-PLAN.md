# Code coverage

**Status:** Done

Add code coverage measurement to the test suite and CI pipeline.

## Problem

The engine has 42 integration examples and a growing set of validation
paths, but there is no visibility into which source lines are actually
exercised by tests. Uncovered code hides silently — dead branches, error
paths never triggered, render edge cases never reached.

## Goals

1. **Measure coverage** — line and branch coverage for `engine-rs/src/`.
2. **CI integration** — coverage runs on every push, results visible.
3. **Coverage trend** — track whether coverage improves or regresses.
4. **No coverage targets** — avoid hard thresholds that incentivize
   gaming. Use coverage as a discovery tool, not a gate.

## Tool choice

### Recommended: `cargo-llvm-cov`

[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) uses LLVM's
source-based instrumentation. Most accurate for Rust code.

```bash
cargo install cargo-llvm-cov

# Generate HTML report
cargo llvm-cov --html --test integration

# Generate lcov for upload
cargo llvm-cov --lcov --output-path lcov.info
```

Advantages:
- Source-based (not DWARF-based) — accurate line-level attribution
- Supports branch coverage via `--branch`
- Outputs lcov, Cobertura, JSON — compatible with all reporting services
- Maintained, widely used in the Rust ecosystem

### Alternative: `cargo-tarpaulin`

[`cargo-tarpaulin`](https://github.com/xd009642/tarpaulin) is DWARF-based.
Simpler to install but occasionally inaccurate on complex Rust code
(async, macros). Works fine for simpler projects.

### Reporting

Upload coverage to a service for PR annotations and trend tracking:

| Service | Free for OSS | PR comments | Badge |
|---------|-------------|-------------|-------|
| [Codecov](https://codecov.io) | Yes | Yes | Yes |
| [Coveralls](https://coveralls.io) | Yes | Yes | Yes |
| GitHub Actions summary | N/A | Via step output | Manual |

Recommendation: **Codecov** — free for open source, GitHub integration,
no self-hosting.

## CI integration

Add to the CI workflow (from CI-RELEASE-PLAN):

```yaml
  coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: llvm-tools-preview
      - uses: taiki-e/install-action@cargo-llvm-cov
      - uses: Swatinem/rust-cache@v2
      - run: cargo llvm-cov --test integration --lcov --output-path lcov.info
      - uses: codecov/codecov-action@v4
        with:
          files: lcov.info
          token: ${{ secrets.CODECOV_TOKEN }}
```

## Local usage

```bash
# Quick summary
cargo llvm-cov --test integration

# HTML report (opens in browser)
cargo llvm-cov --html --test integration --open

# Which lines in forward.rs are uncovered?
cargo llvm-cov --html --test integration
open target/llvm-cov/html/index.html
```

## What to look for

Coverage as a discovery tool, not a metric to maximize:

- **Uncovered error paths** — validation branches that no example triggers.
  May indicate missing test cases or dead code.
- **Render branches** — conditional SQL generation for features like
  `type:`, `direction:`, `reverse_filter:`. Ensure each feature has at
  least one exercising example.
- **Parser edge cases** — shorthand vs longhand YAML forms, optional fields.

## Implementation

1. Add `cargo-llvm-cov` to the development toolchain
2. Run locally, review initial coverage report
3. Add coverage job to CI workflow
4. Add Codecov badge to README
5. Review uncovered lines — add targeted tests or examples where gaps matter
