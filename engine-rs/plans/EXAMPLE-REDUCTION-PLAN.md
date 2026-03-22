# Example reduction

**Status:** Planned (Phases 1–2 done)

The `examples/` directory has grown to 52 examples. Many were written to demonstrate SQL-level mechanics (expression strategies, sentinel patterns, aggregation functions) rather than mapping primitives. Now that the schema is stable with a small set of core properties, we should consolidate examples around **what the mapping declares** — not implementation details like `string_agg`, `COALESCE`, or `bool_or` SQL expressions.

Goal: reduce from 52 to roughly 25 examples, each demonstrating one or two mapping primitives with clear input → output scenarios.

## Guiding principles

1. **One primitive, one example.** Each example should be the canonical demonstration of a single schema property or pattern. Combinations are allowed only when the combination itself is the point.
2. **No SQL in the spotlight.** Examples demonstrate mapping YAML and expected data outcomes. Remove examples whose primary purpose is showing a clever `expression` or `reverse_expression`.
3. **Flatten progressions.** Where we have "simple → medium → advanced" chains (embedded-simple → embedded-objects → embedded-multiple), keep only the one that best covers the primitive.
4. **Merge overlapping patterns.** Where two examples differ only in entity names or minor config variation, combine into one.

## Phase 1 — Remove expression-heavy examples

These examples exist primarily to demonstrate SQL expression strategies rather than mapping primitives. The `expression` strategy is a power-user escape hatch, not a core teaching tool.

| Remove | Reason |
|--------|--------|
| `custom-resolution` | Showcases `string_agg`, `max`, `avg` — SQL aggregation, not mapping |
| `merge-partials` | Entire point is `bool_or(is_customer)` expression |
| `null-propagation` | Sentinel pattern with `COALESCE`/`NULLIF` — workaround, not primitive |
| `types` | `string_agg(distinct type, ',')` — SQL aggregation on type classification |
| `propagated-delete` | `bool_or` expression + `reverse_filter` for GDPR deletion — combine deletion signal into `hard-delete` or `soft-delete` as a variant |

**Net change:** −5 examples.

## Phase 2 — Consolidate overlapping groups

### Embedded (4 → 2)

| Keep | Absorbs | Rationale |
|------|---------|-----------|
| `embedded-objects` | `embedded-simple` | Objects already shows the simple case plus FK complexity |
| `embedded-vs-many-to-many` | (standalone) | Structurally distinct: 3 targets, demonstrates relationship conversion |

Remove: `embedded-simple`, `embedded-multiple`.

### Nested arrays (3 → 2)

| Keep | Absorbs | Rationale |
|------|---------|-----------|
| `nested-arrays` | (standalone) | Covers single-level array extraction |
| `nested-arrays-deep` | `nested-arrays-multiple` | Deep already demonstrates `parent_fields` through intermediate levels; multiple branches can be shown as a variant |

Remove: `nested-arrays-multiple`.

### Hierarchy / depth (2 → 1)

| Keep | Absorbs | Rationale |
|------|---------|-----------|
| `depth-mismatch` | `hierarchy-merge` | Both show 2-level vs 3-level merge; depth-mismatch is the cleaner pattern name |

Remove: `hierarchy-merge`.

### Route (4 → 2)

| Keep | Absorbs | Rationale |
|------|---------|-----------|
| `route` | (standalone) | Discriminator routing — canonical |
| `route-combined` | `route-multiple`, `route-embedded` | Combined already shows routed + dedicated sources merging; other variants are minor twists |

Remove: `route-multiple`, `route-embedded`.

### Relationship (2 → 1)

| Keep | Absorbs | Rationale |
|------|---------|-----------|
| `relationship-mapping` | `relationship-embedded` | Both show many-to-many ↔ embedded conversion with `link_group`; keep one |

Remove: `relationship-embedded`.

### CRDT ordering (2 → 1)

| Keep | Absorbs | Rationale |
|------|---------|-----------|
| `crdt-ordering` | `crdt-ordering-native` | Core primitive is `order: true`; native variant can be shown as additional fields in the same example |

Remove: `crdt-ordering-native`.

### Merge (5 → 3)

| Keep | Rationale |
|------|-----------|
| `merge-curated` | Unique: explicit linkage table |
| `merge-threeway` | Unique: transitive closure via shared identity across 3 systems |
| `merge-internal` | Unique: single-source deduplication |

| Remove | Absorbs into | Rationale |
|--------|-------------|-----------|
| `merge-generated-ids` | `merge-curated` | Both use explicit linkage tables; generated-ids adds a third system which curated can demonstrate |
| `merge-groups` | `value-groups` | `link_group` composite identity can be shown alongside `group:` |

### Deletion (5 → 2)

| Keep | Rationale |
|------|-----------|
| `hard-delete` | Entity-level deletion via `derive_tombstones` + `cluster_members` — core pattern |
| `soft-delete` | Entity-level soft-delete via `soft_delete:` property — core pattern |

| Remove | Absorbs into | Rationale |
|--------|-------------|-----------|
| `element-hard-delete` | `hard-delete` (add element-level variant) | Same primitive (`derive_tombstones`) at different scope |
| `element-soft-delete` | `soft-delete` (add element-level variant) | Same primitive (`soft_delete`) at different scope |
| `scalar-array-deletion` | `hard-delete` (add scalar variant) | Same `derive_tombstones` mechanism on `scalar: true` arrays |

### Element resolution (2 → 1)

| Keep | Absorbs | Rationale |
|------|---------|-----------|
| `element-priority` | `element-last-modified` | Both demonstrate `elements:` strategy on targets; priority and last_modified are just strategy names — one example showing both is sufficient |

Remove: `element-last-modified`.

### Vocabulary (2 → 1)

| Keep | Absorbs | Rationale |
|------|---------|-----------|
| `vocabulary-standard` | `vocabulary-custom` | Both show vocabulary as a target with references; combine into one |

Remove: `vocabulary-custom`.

### Value (4 → 2)

| Keep | Rationale |
|------|-----------|
| `value-defaults` | Core: `default` and `default_expression` properties |
| `value-groups` | Core: `group:` atomic resolution |

| Remove | Absorbs into | Rationale |
|--------|-------------|-----------|
| `value-conversions` | `value-defaults` or inline in relevant examples | `expression` / `reverse_expression` on fields — transformation, not a standalone primitive |
| `value-derived` | `value-groups` | Uses `group:` + `default_expression` — same primitives as value-groups |

**Phase 2 net change:** −19 examples.

## Phase 3 — Review remaining examples

After phases 1–2, the surviving set (~28 examples):

| # | Example | Demonstrates |
|---|---------|-------------|
| 1 | `hello-world` | Simplest mapping: identity + coalesce |
| 2 | `composite-keys` | Multi-field primary keys + `link_group` |
| 3 | `concurrent-detection` | `include_base` for optimistic locking |
| 4 | `crdt-ordering` | `order: true` + `order_prev`/`order_next` |
| 5 | `depth-mismatch` | Asymmetric nesting across sources |
| 6 | `derive-noop` | `written_state` + `derive_noop` |
| 7 | `derive-timestamps` | `derive_timestamps` per-field change detection |
| 8 | `embedded-objects` | `parent:` for embedded sub-entities |
| 9 | `embedded-vs-many-to-many` | Embedded ↔ junction table conversion |
| 10 | `flattened` | Flat target from nested sources |
| 11 | `hard-delete` | `derive_tombstones` + `cluster_members` (entity + element + scalar) |
| 12 | `inserts-and-deletes` | `reverse_required` for insert suppression |
| 13 | `json-fields` | `source_path` for JSONB extraction |
| 14 | `json-opaque` | `type: jsonb` atomic blob |
| 15 | `merge-curated` | Explicit linkage tables |
| 16 | `merge-internal` | Single-source dedup |
| 17 | `merge-threeway` | Transitive closure across 3 systems |
| 18 | `multi-value` | Scalar ↔ list cardinality mismatch |
| 19 | `multiple-target-mappings` | Multiple targets from one source |
| 20 | `nested-arrays` | Single-level array extraction |
| 21 | `nested-arrays-deep` | Multi-level `parent_fields` chains |
| 22 | `passthrough` | `passthrough:` unmapped columns |
| 23 | `precision-loss` | `normalize:` on field mappings |
| 24 | `reference-preservation` | FK preservation post-merge |
| 25 | `references` | Cross-entity `references:` |
| 26 | `required-fields` | `reverse_filter` for data quality gates |
| 27 | `relationship-mapping` | Many-to-many relationship mapping |
| 28 | `route` | Discriminator-based routing |
| 29 | `route-combined` | Routing + dedicated sources |
| 30 | `soft-delete` | `soft_delete:` with strategies (entity + element) |
| 31 | `element-priority` | `elements:` target strategy (priority + last_modified) |
| 32 | `value-defaults` | `default` + `default_expression` |
| 33 | `value-groups` | `group:` atomic resolution |
| 34 | `vocabulary-standard` | Vocabulary targets with `references_field` |

Review this list for any further redundancy. Consider whether `concurrent-detection`, `inserts-and-deletes`, and `required-fields` are distinct enough or overlap on filtering/ETL concerns.

## Phase 4 — Update infrastructure

1. Update each surviving example's `mapping.yaml` to remove SQL-focused commentary.
2. Update each surviving example's `README.md` per CONTRIBUTING.md format.
3. Rewrite `examples/README.md` catalog table.
4. Update test suite references (check `engine-rs/tests/` for hardcoded example paths).
5. Run full test suite to verify nothing breaks.
6. Update `docs/reference/annotated-example.md` if it references removed examples.
7. Update `docs/reference/examples-catalog.md` if it exists.

## Summary

| Metric | Before | After |
|--------|--------|-------|
| Total examples | 52 | ~28–34 |
| Expression-heavy removed | — | 5 |
| Consolidated groups | — | ~13–19 |
| Core primitives covered | all | all |
