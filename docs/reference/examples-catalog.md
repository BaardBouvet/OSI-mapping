# Examples catalog

Runnable mapping examples, each in its own directory under `examples/`.

## Suggested starting points

- [**hello-world**](../../examples/hello-world/README.md) — simplest end-to-end example
- [**merge-threeway**](../../examples/merge-threeway/README.md) — merge behavior across multiple systems
- [**reference-preservation**](../../examples/reference-preservation/README.md) — foreign-key handling across merged entities

Use these examples together with the [Annotated example](annotated-example.md)
and [Schema reference](schema-reference.md).

Examples are split into **primitives** (single schema feature) and
**patterns** (multiple features combined to solve a real-world problem).

## Primitives

Each primitive example demonstrates one schema feature in isolation.

### Identity & merge keys

| Example | Demonstrates |
|---|---|
| [hello-world](../../examples/hello-world/README.md) | Simplest mapping — two sources, one target, identity + coalesce |
| [composite-keys](../../examples/composite-keys/README.md) | Multi-field identity (compound match key) |
| [merge-groups](../../examples/merge-groups/README.md) | Group-based atomic resolution |

### Resolution strategies

| Example | Demonstrates |
|---|---|
| [custom-resolution](../../examples/custom-resolution/README.md) | Custom resolution strategy via expression |
| [value-groups](../../examples/value-groups/README.md) | Field group resolution |
| [types](../../examples/types/README.md) | Type tracking via composite expression strategy |

### Value transforms

| Example | Demonstrates |
|---|---|
| [value-conversions](../../examples/value-conversions/README.md) | Bidirectional value transformations via expression / reverse\_expression |
| [value-defaults](../../examples/value-defaults/README.md) | Default values and default expressions |
| [value-derived](../../examples/value-derived/README.md) | Derived / computed fields |
| [json-fields](../../examples/json-fields/README.md) | Extracting sub-fields from JSONB source columns via `source_path` |
| [precision-loss](../../examples/precision-loss/README.md) | Handling precision loss with `normalize` on field mappings |

### Vocabulary

| Example | Demonstrates |
|---|---|
| [vocabulary-custom](../../examples/vocabulary-custom/README.md) | Custom vocabulary definitions |
| [vocabulary-standard](../../examples/vocabulary-standard/README.md) | Standard vocabulary usage |

### Embedded entities

| Example | Demonstrates |
|---|---|
| [embedded-simple](../../examples/embedded-simple/README.md) | Single embedded sub-entity |
| [embedded-objects](../../examples/embedded-objects/README.md) | Nested embedded objects |
| [embedded-multiple](../../examples/embedded-multiple/README.md) | Multiple embedded entities |

### Nested arrays

| Example | Demonstrates |
|---|---|
| [nested-arrays](../../examples/nested-arrays/README.md) | Array-of-objects field mapping |
| [nested-arrays-deep](../../examples/nested-arrays-deep/README.md) | Deeply nested array structures |
| [nested-arrays-multiple](../../examples/nested-arrays-multiple/README.md) | Multiple nested arrays |

### References

| Example | Demonstrates |
|---|---|
| [references](../../examples/references/README.md) | Foreign-key references between targets |

### Routing & filtering

| Example | Demonstrates |
|---|---|
| [route](../../examples/route/README.md) | Routing records by field values |
| [route-embedded](../../examples/route-embedded/README.md) | Routing within embedded objects |
| [inserts-and-deletes](../../examples/inserts-and-deletes/README.md) | Conditional row inclusion via `reverse_required` |

### Ordering

| Example | Demonstrates |
|---|---|
| [crdt-ordering](../../examples/crdt-ordering/README.md) | Deterministic array element ordering via `order: true` |
| [crdt-ordering-native](../../examples/crdt-ordering-native/README.md) | Mixed ordering: native `sort_key` + generated ordering |

### State & noop detection

| Example | Demonstrates |
|---|---|
| [concurrent-detection](../../examples/concurrent-detection/README.md) | Detecting and handling concurrent edits via `include_base` |
| [derive-noop](../../examples/derive-noop/README.md) | Target-centric noop detection via ETL written state |
| [derive-timestamps](../../examples/derive-timestamps/README.md) | Per-field timestamp inference from written state |
| [passthrough](../../examples/passthrough/README.md) | Carrying unmapped source columns through to delta output |

## Patterns

Each pattern example combines multiple features to solve a real-world
integration problem.

### Merge strategies

| Example | Demonstrates |
|---|---|
| [merge-internal](../../examples/merge-internal/README.md) | Internal merge within a single source |
| [merge-threeway](../../examples/merge-threeway/README.md) | Three-way merge between sources |
| [merge-curated](../../examples/merge-curated/README.md) | Curated merge with manual overrides via linking table |
| [merge-generated-ids](../../examples/merge-generated-ids/README.md) | Cross-system linkage via ETL-maintained tables with generated IDs |
| [merge-partials](../../examples/merge-partials/README.md) | Partial record merge with forward\_only mappings |

### Structural bridging

| Example | Demonstrates |
|---|---|
| [depth-mismatch](../../examples/depth-mismatch/README.md) | Asymmetric nesting depth — 2-level vs 3-level with intermediate grouping |
| [hierarchy-merge](../../examples/hierarchy-merge/README.md) | Merging hierarchies of different depths via cross-depth identity |
| [flattened](../../examples/flattened/README.md) | Denormalizing multiple sources into a single flattened target |
| [multi-value](../../examples/multi-value/README.md) | Scalar-vs-list cardinality mismatch |
| [embedded-vs-many-to-many](../../examples/embedded-vs-many-to-many/README.md) | Bridging embedded relationships with normalized junction tables |

### Relationships

| Example | Demonstrates |
|---|---|
| [relationship-mapping](../../examples/relationship-mapping/README.md) | Many-to-many relationships via junction table target |
| [relationship-embedded](../../examples/relationship-embedded/README.md) | Converting embedded relationships to many-to-many |
| [reference-preservation](../../examples/reference-preservation/README.md) | Preserving original reference IDs when records merge |
| [multiple-target-mappings](../../examples/multiple-target-mappings/README.md) | Single source mapping to multiple embedded targets |

### Combined routing

| Example | Demonstrates |
|---|---|
| [route-combined](../../examples/route-combined/README.md) | Merging routing source with dedicated sources |
| [route-multiple](../../examples/route-multiple/README.md) | Multiple routing rules from single source to same target |

### Data quality & deletes

| Example | Demonstrates |
|---|---|
| [null-propagation](../../examples/null-propagation/README.md) | Propagating intentional NULLs via sentinel pattern |
| [propagated-delete](../../examples/propagated-delete/README.md) | GDPR-style deletion propagation via bool\_or + reverse\_filter |
| [required-fields](../../examples/required-fields/README.md) | Minimum-required fields via reverse\_filter OR pattern |
| [derive-tombstones](../../examples/derive-tombstones/README.md) | Element-level deletion-wins for nested arrays via written state |

### End-to-end showcases

| Example | Demonstrates |
|---|---|
| [sesam-annotated](../../examples/sesam-annotated/README.md) | Full DTL annotated example: enriched expressions, nested array sort, reverse\_filter, reverse\_expression, normalize, references, reverse\_only direction |

Each example directory contains a `README.md` and a `mapping.yaml` with the
full definition including test cases.
