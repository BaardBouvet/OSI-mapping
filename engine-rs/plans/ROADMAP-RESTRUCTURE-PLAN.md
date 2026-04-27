# Roadmap restructure & docs split

**Status:** Done

Restructure [`engine-rs/ROADMAP.md`](../ROADMAP.md) around named releases
(starting with **0.1**) and split engine-only material out of `docs/`
into [`engine-rs/plans/`](.). After this lands the outer `docs/` tree
contains only specs, examples documentation, and contributor-facing
design rationale; everything implementation-flavoured lives next to the
engine that ships it.

## Goals

1. Make "what's in this release" answerable in one screen.
2. Stop the roadmap from being a flat changelog of every plan ever opened.
3. Keep `docs/` engine-agnostic (per [CONTRIBUTING.md](../../CONTRIBUTING.md)).
4. Land zero behaviour changes — pure relocation + restructure.

## Non-goals

- Renaming individual plans.
- Marking any plan complete that isn't already.
- Touching `docs/reference/` or `docs/SUMMARY.md` content other than
  removing dead links.
- Rewriting the SPARQL or PG roadmaps' technical content.

## Part 1 — Release-shaped roadmap

Replace the contents of [`engine-rs/ROADMAP.md`](../ROADMAP.md) with a
release-oriented document. Sketch:

```markdown
# Roadmap

The engine ships in numbered releases. Each release links to the plans
it contains; status of individual plans lives in [plans/README.md](plans/README.md).

## 0.1 — Current

Mapping → PostgreSQL views, plus first-cut SPARQL/CONSTRUCT artifacts
for triplestore deployment.

**PG backend (stable):**
- Forward / identity / resolution / analytics / reverse / delta views.
- 43 conformance examples.
- See [Phase 0–3 in this file's history](#) or
  [plans/README.md](plans/README.md) for the full plan list.

**SPARQL backend (preview):**
- CONSTRUCT-only artifact pipeline — see
  [SPARQL-CONSTRUCT-ARTIFACTS-PLAN.md](plans/SPARQL-CONSTRUCT-ARTIFACTS-PLAN.md).
- 4 conformance scenarios green.
- Configurable base IRI via `--base-iri` / `render_sparql_with_base`.

**CLI:** `parse`, `render` (with `-b pg|sparql`, `--out-dir`, `--base-iri`),
`validate`, `dot`.

## 0.2 — Planned

Theme: **release engineering & SPARQL hardening.**

- [CI-RELEASE-PLAN.md](plans/CI-RELEASE-PLAN.md)
- [LEARNING-GUIDE-PLAN.md](plans/LEARNING-GUIDE-PLAN.md)
- [CONSUMER-NAMING-PLAN.md](plans/CONSUMER-NAMING-PLAN.md)
- [DELTA-RESERVED-COLUMNS-PLAN.md](plans/DELTA-RESERVED-COLUMNS-PLAN.md)
- [CLI-TEST-COMMAND-PLAN.md](plans/CLI-TEST-COMMAND-PLAN.md)
- SPARQL: shortcomings #2/#6/#8/#14 from
  [SPARQL-CONSTRUCT-ARTIFACTS-PLAN.md](plans/SPARQL-CONSTRUCT-ARTIFACTS-PLAN.md)
  (validate `--base-iri`, RFC-3986 PK encoding, `PREFIX` block in artifacts,
  cross-base round-trip test).

## 0.3 — Considered

- [NAMING-PLAN.md](plans/NAMING-PLAN.md) — project rename.
- SPARQL artifact self-containment (drop `Doc` from `SparqlPlan`).
- Delta CONSTRUCTs — replace Rust `compute_deltas` with `delta_<M>.sparql`.

## Post-1.0 / unscheduled

Tracked in [plans/README.md](plans/README.md) under `Planned`,
`Design`, `Proposed`, `Maybe`. Releases 0.4+ pull from this pool when
themes solidify.

## Principles

(unchanged from previous roadmap — schema stability first, security
before features, examples prove designs, patterns don't block, defer
what has workarounds)

## Release definition

A release is a git tag. To cut release `X.Y`:
1. All plans listed under that release have status `Done`.
2. CI green on `main`.
3. `cargo test` green for both backends.
4. Bump `engine-rs/Cargo.toml` version, tag, push.
```

The exhaustive Phase-0/1/2/3 tables currently in `ROADMAP.md` get
**deleted** — that history lives in `plans/README.md` (which already
lists every plan with its status) and in git history of `ROADMAP.md`
itself.

## Part 2 — Move engine material out of `docs/design/`

`docs/design/` currently mixes contributor-facing design rationale with
engine-specific implementation plans and analyses. Per
[CONTRIBUTING.md](../../CONTRIBUTING.md):

> Specification files (`docs/`, `spec/`, `examples/`) … must be
> **engine-agnostic**. Never reference `engine-rs/` or any specific
> implementation.

### Stays in `docs/design/`

| File | Why |
| --- | --- |
| `ai-guidelines.md` | Contributor guidance, referenced from `SUMMARY.md`. |
| `design-rationale.md` | Engine-agnostic mapping-language rationale, referenced from `SUMMARY.md`. |

### Moves to `engine-rs/plans/`

| Source                                          | Destination                                                       |
| ---                                             | ---                                                               |
| `docs/design/triplestore-backend.md`            | `engine-rs/plans/TRIPLESTORE-BACKEND-DESIGN.md`                   |
| `docs/design/v2-spec-draft.md`                  | `engine-rs/plans/V2-SPEC-DRAFT.md`                                |
| `docs/design/v2-migration-rfc.md`               | `engine-rs/plans/V2-MIGRATION-RFC.md`                             |
| `docs/design/v2-prototype-examples.md`          | `engine-rs/plans/V2-PROTOTYPE-EXAMPLES.md`                        |
| `docs/design/value-map-rfc.md`                  | `engine-rs/plans/VALUE-MAP-RFC.md`                                |
| `docs/design/product-market-fit-analysis.md`    | `engine-rs/plans/PRODUCT-MARKET-FIT-ANALYSIS.md`                  |

Each moved file:
- Gets a `**Status:** …` header conforming to
  [CONTRIBUTING.md](../../CONTRIBUTING.md) plan format if it doesn't
  already.
- Internal cross-references rewritten to the new locations.

### Cross-reference updates

- `engine-rs/plans/SPARQL-IMPLEMENTATION-PLAN.md` line 9 — update link
  `../../docs/design/triplestore-backend.md` →
  `./TRIPLESTORE-BACKEND-DESIGN.md`.
- Any other `docs/design/{moved-file}` references found via
  repo-wide grep.

### Index updates

- `engine-rs/plans/README.md` — add a row for each moved file.
- `docs/SUMMARY.md` — already only lists the two stayers; verify no
  dangling links.
- `book/` is generated; will be rebuilt on next docs build, no manual edit.

## Acceptance criteria

- `engine-rs/ROADMAP.md` fits on roughly one screen (~120 lines).
- 0.1 release section accurately reflects what's in `main` today.
- 0.2 section lists at most ~6 plans, each linked.
- `docs/design/` contains only `ai-guidelines.md` and
  `design-rationale.md`.
- `engine-rs/plans/README.md` lists the six moved files.
- `cargo test` still green (no code change, but sanity).
- `mdbook build` still produces `docs/SUMMARY.md`'s entries cleanly.
- Repo-wide `grep -r "docs/design/"` returns only references to the two
  remaining files.

## Risks

- **mdBook regression.** None expected — the moved files were never in
  `SUMMARY.md`. Verify by running the mdBook build.
- **Stale external links.** The repo's README links to `engine-rs/ROADMAP.md`
  but not into `docs/design/`; external readers linking directly to the
  moved RFCs will hit 404. Acceptable — pre-1.0 (per
  [CONTRIBUTING.md](../../CONTRIBUTING.md)).
- **Lost roadmap detail.** The Phase 0–3 tables disappear from the new
  roadmap. They remain in git history and in `plans/README.md`.
  Mitigation: ensure `plans/README.md` is up to date before deletion
  (it already is).

## Out of scope

- Renaming any plan file.
- Editing the content of moved plans (status field added if missing,
  links fixed; otherwise byte-identical).
- Touching `engine-rs/TODOS.md`.
- Reorganising `engine-rs/docs/` (the per-engine internal docs).
