# Examples catalog

Runnable mapping examples, each in its own directory under `examples/`.

## Suggested starting points

- **hello-world** — simplest end-to-end example
- **merge-threeway** — merge behavior across multiple systems
- **reference-preservation** — foreign-key handling across merged entities

Use these examples together with the [Annotated example](annotated-example.md)
and [Schema reference](schema-reference.md).

## Full catalog

| Example | Demonstrates |
|---|---|
| composite-keys | Multi-field identity (compound match key) |
| concurrent-detection | Detecting and handling concurrent edits |
| custom-resolution | Custom resolution strategy via expression |
| depth-mismatch | Asymmetric nesting depth — 2-level vs 3-level with intermediate grouping |
| embedded-simple | Single embedded sub-entity |
| embedded-objects | Nested embedded objects |
| embedded-multiple | Multiple embedded entities |
| embedded-vs-many-to-many | Embedded vs. reference-based relationships |
| flattened | Flattened source structure into normalized target |
| hello-world | Simplest mapping — two sources, one target, identity + coalesce |
| hierarchy-merge | Merging hierarchies via cross-depth identity resolution |
| inserts-and-deletes | Handling new and removed records |
| json-fields | Extracting sub-fields from JSONB source columns |
| merge-curated | Curated merge with manual overrides |
| merge-generated-ids | Merge with system-generated identifiers |
| merge-groups | Group-based atomic resolution |
| merge-internal | Internal merge within a single source |
| merge-partials | Partial record merge |
| merge-threeway | Three-way merge between sources |
| multi-value | Scalar-vs-list cardinality mismatch |
| multiple-target-mappings | Multiple targets in one file |
| nested-arrays | Array-of-objects field mapping |
| nested-arrays-deep | Deeply nested array structures |
| nested-arrays-multiple | Multiple nested arrays |
| null-propagation | Propagating intentional NULLs via sentinel pattern |
| propagated-delete | GDPR-style deletion propagation via bool\_or + reverse\_filter |
| reference-preservation | Preserving foreign-key references |
| references | Foreign-key references between targets |
| required-fields | Required-field constraints via reverse\_filter OR pattern |
| relationship-embedded | Embedded relationship mapping |
| relationship-mapping | Standalone relationship mapping |
| route | Routing records by field values |
| route-combined | Combined routing logic |
| route-embedded | Routing within embedded objects |
| route-multiple | Multiple routing rules |
| types | Type conversion and coercion |
| value-conversions | Value mapping / vocabulary conversion |
| value-defaults | Default values and default expressions |
| value-derived | Derived / computed fields |
| value-groups | Field group resolution |
| vocabulary-custom | Custom vocabulary definitions |
| vocabulary-standard | Standard vocabulary usage |

Each example directory contains a `README.md` and a `mapping.yaml` with the
full definition including test cases.
