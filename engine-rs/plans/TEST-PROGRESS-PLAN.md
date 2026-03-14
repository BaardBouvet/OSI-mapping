# Test Progress Plan

**Status:** In Progress

## Generic Test Runner

Added `execute_all_examples` integration test that:
1. Discovers all examples with `tests:` sections
2. Parses + renders each example
3. Skips examples without sync views (no delta to verify)
4. Executes SQL views against PostgreSQL (testcontainers)
5. Verifies expected counts for updates/noops/inserts/deletes
6. Reports a pass/fail/skip summary at the end (non-panicking)

Run with: `cargo test --test integration execute_all_examples -- --nocapture`

## Current Test Status

### Passing (verified E2E)
- hello-world — 6 tests ✓
- references — 3 tests ✓

### Need Verification (have sync views, never executed)
These examples auto-derive sync from bidirectional fields.
Listed in priority order for getting them passing:

| # | Example | Complexity | Likely Issues |
|---|---------|-----------|---------------|
| 1 | minimal | Low | Two tests, basic merge |
| 2 | merge-internal | Low | Same-source merge |
| 3 | inserts-and-deletes | Low | reverse_required filter |
| 4 | merge-curated | Low | Linking table merge |
| 5 | composite-keys | Medium | Composite PK, cross-target refs |
| 6 | merge-threeway | Low | Three-source coalesce |
| 7 | merge-groups | Medium | link_group composite merge keys |
| 8 | merge-generated-ids | Medium | Linkage tables, 3-system |
| 9 | merge-partials | Medium | forward_only flag, reverse_filter |
| 10 | types | Medium | Expression fields, reverse_filter |
| 11 | value-conversions | Medium | Expression + reverse_expression |
| 12 | value-derived | Medium | Group + default_expression |
| 13 | value-defaults | Low | Default values + constants |
| 14 | value-groups | Medium | Group resolution, self-merge |
| 15 | flattened | Medium | Multi-source to one target |
| 16 | vocabulary-custom | Hard | Reference resolution + vocab |
| 17 | vocabulary-standard | Hard | Reference resolution + vocab |
| 18 | concurrent-detection | Medium | _base column verification |
| 19 | embedded-simple | Hard | Embedded mappings |
| 20 | embedded-multiple | Hard | Multiple embedded |
| 21 | embedded-objects | Hard | Embedded + references |
| 22 | embedded-vs-many-to-many | Hard | Many-to-many + embedded |
| 23 | multiple-target-mappings | Medium | Multiple embedded targets |
| 24 | reference-preservation | Medium | PK reference preservation |
| 25 | relationship-embedded | Hard | Embedded associations |
| 26 | relationship-mapping | Hard | Many-to-many relationships |
| 27 | route | Medium | Filter-based routing |
| 28 | route-combined | Hard | Route + dedicated sources |
| 29 | route-embedded | Medium | Route + embedded |
| 30 | route-multiple | Medium | Multiple route targets |

### No Sync Views (forward-only, no delta verification)
These examples have only forward_only or expression-only fields — no reverse:
- nested-arrays, nested-arrays-deep, nested-arrays-multiple (path-based sources)
- custom-resolution (not yet implemented)

## Known Blockers

1. **Composite key reference resolution** — `COMPOSITE-KEY-REFS-PLAN.md`
2. **Embedded mapping reverse** — embedded mappings produce a virtual source;
   reverse needs to reassemble the parent row.
3. **Route-based mappings** — `filter:` restricts which rows enter forward;
   reverse needs the complementary filter for `reverse_expression`.
4. **Path-based sources** (nested arrays) — not SQL-compatible; needs
   JSON unnesting or a different execution model.
