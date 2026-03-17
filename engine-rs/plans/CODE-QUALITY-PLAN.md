# Code quality tooling

**Status:** Done

Enforce consistent formatting, catch common mistakes, and tighten the
codebase with `rustfmt`, `clippy`, and related tools.

## Problem

The codebase has grown organically through feature plans. There is no
enforced formatting standard, no lint configuration, and no automated
checks for common Rust pitfalls. Inconsistencies accumulate — mixed
formatting styles, unused imports, redundant clones, non-idiomatic patterns.

## Goals

1. **Consistent formatting** — `cargo fmt` as the single source of truth.
2. **Lint enforcement** — `clippy` catches bugs and non-idiomatic code.
3. **Zero-warning policy** — CI fails on any warning.
4. **One-time cleanup** — apply tools across the entire codebase once,
   then enforce going forward.

## Formatting: rustfmt

### Configuration

Create `engine-rs/rustfmt.toml` with project settings:

```toml
edition = "2021"
max_width = 100
use_field_init_shorthand = true
use_try_shorthand = true
```

Keep defaults for most settings — the community standard is widely
understood and minimizes bikeshedding.

### Initial cleanup

```bash
cd engine-rs
cargo fmt
```

Review the diff. It will be large but mechanical. Commit as a single
formatting-only commit with a clear message (`chore: apply rustfmt`).

## Linting: clippy

### Configuration

Create `engine-rs/clippy.toml` or configure via `Cargo.toml`:

```toml
# In Cargo.toml [lints] section (Rust 1.74+)
[lints.clippy]
# Deny common mistake categories
correctness = { level = "deny" }
suspicious = { level = "deny" }
# Warn on style and complexity
style = { level = "warn" }
complexity = { level = "warn" }
perf = { level = "warn" }
# Specific useful lints
needless_pass_by_value = "warn"
redundant_clone = "warn"
manual_let_else = "warn"
uninlined_format_args = "warn"
```

### Initial cleanup

```bash
cargo clippy -- -D warnings 2>&1 | head -100
```

Fix issues in categories:
- **Redundant clones** — `.clone()` where a borrow suffices
- **Needless allocations** — `.to_string()` where `&str` works
- **Pattern matching** — `if let` vs `match` vs `let-else`
- **String formatting** — `format!("{}", x)` → `format!("{x}")`
- **Unused imports / variables** — remove dead code

Commit fixes grouped by category for reviewable history.

## Additional tools

### `cargo-deny`

Audit dependencies for security advisories and license compatibility:

```bash
cargo install cargo-deny
cargo deny init
cargo deny check
```

Add to CI as a non-blocking advisory check initially, then make it blocking.

### `cargo-audit`

Checks for known vulnerabilities in dependencies:

```bash
cargo install cargo-audit
cargo audit
```

Lighter than `cargo-deny` — just security, no license checking.

### `cargo-machete`

Find unused dependencies in `Cargo.toml`:

```bash
cargo install cargo-machete
cargo machete
```

### `cargo-outdated`

Check for outdated dependencies:

```bash
cargo install cargo-outdated
cargo outdated
```

Run periodically (not in CI) to keep dependencies current.

## CI enforcement

Add to the CI workflow (from CI-RELEASE-PLAN):

```yaml
  quality:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --check
      - run: cargo clippy -- -D warnings
      - run: cargo deny check advisories
```

`cargo fmt --check` exits non-zero if any file is unformatted.
`clippy -- -D warnings` fails on any lint warning.

## Implementation

### Phase 1 — Format and lint

1. Create `rustfmt.toml`
2. Run `cargo fmt`, commit formatting changes
3. Configure clippy lints in `Cargo.toml`
4. Run `cargo clippy`, fix warnings, commit in batches
5. Verify all 42 tests still pass

### Phase 2 — CI gate

1. Add fmt + clippy checks to CI workflow
2. Add `cargo-deny` check for dependency advisories
3. Ensure the quality job blocks PR merges

### Phase 3 — Dependency hygiene

1. Run `cargo-machete` to remove unused dependencies
2. Run `cargo-outdated` and update what's safe to update
3. Add periodic dependency update reminders (Dependabot or Renovate)
