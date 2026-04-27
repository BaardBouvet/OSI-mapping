# Examples catalog

Runnable mapping examples, each in its own directory under [`examples/`](../../examples/).

Use these examples together with the [Annotated example](annotated-example.md)
and [Schema reference](schema-reference.md).

## v2 examples (current engine)

The v2 examples use `version: "2.0"` and are tested end-to-end against both the
PostgreSQL and SPARQL backends on every CI run.

| Example | What it demonstrates |
|---|---|
| [hello-world](../../examples/hello-world/README.md) | Two sources, one target, single-field identity, `coalesce` resolution, priority-based field winners. The starting point. |
| [composite-identity](../../examples/composite-identity/README.md) | AND-tuple identity: two rows match when *both* `first_name` and `last_name` agree. |
| [last-modified](../../examples/last-modified/README.md) | `last_modified` resolution: most recently changed value wins, determined by a source timestamp column. |
| [nested-arrays-shallow](../../examples/nested-arrays-shallow/README.md) | Child mapping with `parent:` / `array:` / `parent_fields:` — expands a JSON array column into individual target rows. Composite identity on the child. |

### In progress

| Example | Status |
|---|---|
| [nested-arrays-v2](../../examples/nested-arrays-v2/README.md) | Parses cleanly but awaits `references:` (slice 4) implementation before it can be executed. |

## v1 examples (legacy)

42 examples use `version: "1.0"`. They document the full feature set developed
in the v1 engine but are not yet migrated to v2 schema. The v2 engine does not
parse v1 mappings.

These examples remain the reference for understanding features planned for v2
migration. They are organised below by schema feature.

### Identity & merge

| Example | Demonstrates |
|---|---|
| [composite-keys](../../examples/composite-keys/README.md) | Multi-field identity (compound match key) |
| [merge-threeway](../../examples/merge-threeway/README.md) | Three-way merge across CRM, ERP, and phone directory |
| [merge-curated](../../examples/merge-curated/README.md) | Manually curated golden record overrides |
| [merge-internal](../../examples/merge-internal/README.md) | Internal merge from a single source with multiple match paths |
| [concurrent-detection](../../examples/concurrent-detection/README.md) | Concurrent-update detection and noop suppression |

### Resolution

| Example | Demonstrates |
|---|---|
| [element-last-modified](../../examples/element-last-modified/README.md) | `last_modified` on nested array elements |
| [element-priority](../../examples/element-priority/README.md) | Priority-based resolution on nested array elements |
| [precision-loss](../../examples/precision-loss/README.md) | `normalize:` for lossy noop comparison |
| [value-defaults](../../examples/value-defaults/README.md) | Default values and default expressions |
| [value-groups](../../examples/value-groups/README.md) | Atomic field group resolution |
| [derive-timestamps](../../examples/derive-timestamps/README.md) | Derived per-field `_ts_*` timestamps |
| [derive-noop](../../examples/derive-noop/README.md) | Written-state noop suppression |

### Nested arrays

| Example | Demonstrates |
|---|---|
| [nested-arrays](../../examples/nested-arrays/README.md) | Nested array from a separate source table |
| [nested-arrays-deep](../../examples/nested-arrays-deep/README.md) | Deeply nested (3-level) arrays via recursive CTEs |
| [nested-array-path](../../examples/nested-array-path/README.md) | Dotted `array_path` for nested JSON extraction |
| [scalar-array](../../examples/scalar-array/README.md) | Scalar (string[]) array fields |
| [crdt-ordering](../../examples/crdt-ordering/README.md) | CRDT-style ordering with `order: true` and prev/next links |
| [crdt-ordering-linked](../../examples/crdt-ordering-linked/README.md) | CRDT ordering across linked entities |
| [embedded-objects](../../examples/embedded-objects/README.md) | Embedded sub-entities as JSONB objects |
| [embedded-vs-many-to-many](../../examples/embedded-vs-many-to-many/README.md) | Choosing between embedded and many-to-many models |

### References & foreign keys

| Example | Demonstrates |
|---|---|
| [references](../../examples/references/README.md) | Cross-entity FK references with explicit `references:` |
| [external-links](../../examples/external-links/README.md) | `links:` and `link_key:` for ETL insert deduplication |
| [relationship-mapping](../../examples/relationship-mapping/README.md) | Multi-entity mapping with bi-directional references |
| [reference-preservation](../../examples/reference-preservation/README.md) | FK preservation across merged entities |

### Soft delete & tombstones

| Example | Demonstrates |
|---|---|
| [soft-delete](../../examples/soft-delete/README.md) | `soft_delete:` with `deleted_flag` strategy |
| [soft-delete-child](../../examples/soft-delete-child/README.md) | Element-level soft delete on nested arrays |
| [soft-delete-resurrect](../../examples/soft-delete-resurrect/README.md) | Resurrect behavior — undelete when tombstone removed |
| [hard-delete](../../examples/hard-delete/README.md) | Hard delete (row removal) vs soft delete |

### Advanced field mapping

| Example | Demonstrates |
|---|---|
| [json-fields](../../examples/json-fields/README.md) | `source_path` for JSONB sub-field extraction |
| [json-opaque](../../examples/json-opaque/README.md) | JSONB fields treated as opaque values |
| [passthrough](../../examples/passthrough/README.md) | `passthrough:` columns carried to delta output unchanged |
| [flattened](../../examples/flattened/README.md) | Flattened source rows expanded to multiple target fields |
| [required-fields](../../examples/required-fields/README.md) | Validation that required fields are non-null |
| [asymmetric-io](../../examples/asymmetric-io/README.md) | Asymmetric read/write shapes between sources |
| [multiple-target-mappings](../../examples/multiple-target-mappings/README.md) | Multiple mappings targeting the same entity |

### Routing & transforms

| Example | Demonstrates |
|---|---|
| [route](../../examples/route/README.md) | Conditional routing of source rows to different targets |
| [route-combined](../../examples/route-combined/README.md) | Combined routing and merging |
| [depth-mismatch](../../examples/depth-mismatch/README.md) | Asymmetric nesting depth across sources |
| [inserts-and-deletes](../../examples/inserts-and-deletes/README.md) | Basic insert and delete delta generation |

### Vocabulary & annotations

| Example | Demonstrates |
|---|---|
| [vocabulary-standard](../../examples/vocabulary-standard/README.md) | Standard vocabulary usage |
| [sesam-annotated](../../examples/sesam-annotated/README.md) | Sesam DTL-annotated mapping with enriched expressions |
| [multi-value](../../examples/multi-value/README.md) | Multi-value cardinality via `collect` strategy |

### Large scale

| Example | Demonstrates |
|---|---|
| [benchmark-large](../../examples/benchmark-large/README.md) | Performance benchmark: many sources, many targets |
