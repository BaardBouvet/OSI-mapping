# Documentation site

**Status:** Planned

Publish all documentation as a static site using mdBook (`book.toml`),
deployable to GitHub Pages or any static host.

## Problem

Documentation currently lives as loose Markdown files in `docs/` and
`engine-rs/plans/`. Readers must browse the repo tree or know exact file
paths. There is no navigation, search, or unified reading experience.
For a project with 40+ examples and 40+ plans, discoverability matters.

## Tool choice: mdBook

[mdBook](https://rust-lang.github.io/mdBook/) is the standard Rust
ecosystem documentation tool. Advantages:

- Single `book.toml` + `SUMMARY.md` to define structure
- Renders Markdown to a searchable static site
- Built-in search (lunr.js), dark mode, print view
- `mdbook serve` for local preview with live reload
- GitHub Pages deployment via a single CI step
- Used by The Rust Book, Cargo Book, rustc dev guide

Alternatives considered:
- **MkDocs + Material** — Python-based; richer theme but adds a Python
  dependency to a Rust project.
- **Docusaurus** — React-based; heavy for a spec + reference project.
- **Plain GitHub wiki** — no PR workflow, no versioning.

## Proposed structure

```
book.toml                    ← mdBook config (repo root)
docs/
  SUMMARY.md                 ← table of contents / sidebar nav
  introduction.md            ← project overview (from README.md)
  motivation.md              ← existing
  learning-guide/
    01-first-mapping.md      ← from LEARNING-GUIDE-PLAN
    02-identity-resolution.md
    ...
  reference/
    schema-reference.md      ← existing (moved or symlinked)
    annotated-example.md     ← existing
  design/
    design-rationale.md      ← existing
    ai-guidelines.md         ← existing
  examples/
    README.md                ← example catalog (link to each)
```

### `book.toml`

```toml
[book]
title = "OSI Mapping"
authors = ["OSI Mapping contributors"]
language = "en"
src = "docs"

[build]
build-dir = "book"

[output.html]
default-theme = "light"
preferred-dark-theme = "ayu"
git-repository-url = "https://github.com/OWNER/osi-mapping"
edit-url-template = "https://github.com/OWNER/osi-mapping/edit/main/{path}"

[output.html.search]
enable = true
```

### `SUMMARY.md`

```markdown
# Summary

- [Introduction](introduction.md)
- [Motivation](motivation.md)

# Learning Guide

- [Your first mapping](learning-guide/01-first-mapping.md)
- [Identity resolution](learning-guide/02-identity-resolution.md)
- [Merge strategies](learning-guide/03-merge-strategies.md)
- [References](learning-guide/04-references.md)
- [Nested arrays](learning-guide/05-nested-arrays.md)
- [Expressions and filters](learning-guide/06-expressions.md)
- [Advanced patterns](learning-guide/07-advanced-patterns.md)

# Reference

- [Schema reference](reference/schema-reference.md)
- [Annotated example](reference/annotated-example.md)
- [Examples catalog](reference/examples.md)

# Design

- [Design rationale](design/design-rationale.md)
- [AI guidelines](design/ai-guidelines.md)
```

## File organization

Two options for handling the existing `docs/` files:

### Option A — Restructure into subdirectories

Move files into `learning-guide/`, `reference/`, `design/` subdirectories.
Clean structure but breaks existing relative links from examples and plans.

### Option B — Flat with SUMMARY.md navigation

Keep all files in `docs/` (no subdirectories). SUMMARY.md provides the
hierarchy. Simplest migration — no broken links.

**Recommendation: Option B** for initial launch. Restructure later if the
flat directory becomes unwieldy.

## Local development

```bash
# Install mdBook
cargo install mdbook

# Preview with live reload
mdbook serve --open

# Build static site
mdbook build
```

The `book/` output directory should be added to `.gitignore`.

## Deployment

### GitHub Pages via GitHub Actions

```yaml
# .github/workflows/docs.yml
name: Deploy docs
on:
  push:
    branches: [main]
    paths: ['docs/**', 'book.toml']

jobs:
  deploy:
    runs-on: ubuntu-latest
    permissions:
      pages: write
      id-token: write
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install mdbook
      - run: mdbook build
      - uses: actions/upload-pages-artifact@v3
        with:
          path: book
      - uses: actions/deploy-pages@v4
```

## Implementation

1. Install mdBook locally, create `book.toml` and `docs/SUMMARY.md`
2. Verify existing docs render correctly
3. Add `book/` to `.gitignore`
4. Set up GitHub Pages deployment workflow
5. After LEARNING-GUIDE-PLAN lands, add those chapters to SUMMARY.md

## Dependencies

- LEARNING-GUIDE-PLAN — the guide chapters are the highest-value new content
  for the site. The site can launch without them (existing docs are still
  useful), but the guide is what makes the site worth visiting.
