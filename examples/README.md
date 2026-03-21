# Examples

This directory contains runnable mapping examples, each in its own subdirectory.

## What You Will Find

- One scenario per folder (for example: `hello-world`, `composite-keys`, `references`, `nested-arrays`)
- A `mapping.yaml` file with a complete mapping definition
- A local `README.md` explaining the scenario, why it exists, and what feature it demonstrates

**Note:** Most real-world mappings contain multiple target entities in a single file. Simple examples like `hello-world` use one target for clarity, but when your integration involves related entities (e.g., company + contact + country), they all go in one file. See `references/` and `relationship-mapping/` for multi-entity examples.

## Suggested Starting Points

- `hello-world/` for the simplest end-to-end example
- `merge-threeway/` for merge behavior across multiple systems
- `reference-preservation/` for foreign-key handling across merged entities

Use these examples together with `../docs/reference/annotated-example.md` and `../docs/reference/schema-reference.md`.

## Full Example Catalog

| Example | Demonstrates |
|---|---|
| [`hello-world`](hello-world/README.md) | Simplest mapping — two sources, one target, identity + coalesce |
| [`composite-keys`](composite-keys/README.md) | Multi-field identity (compound match key) |
| [`concurrent-detection`](concurrent-detection/README.md) | Detecting and handling concurrent edits |
| [`crdt-ordering`](crdt-ordering/README.md) | Deterministic array element ordering via `order: true` |
| [`crdt-ordering-native`](crdt-ordering-native/README.md) | Mixed ordering inputs: native `sort_key` + generated `order: true` |
| [`custom-resolution`](custom-resolution/README.md) | Custom resolution strategy via expression |
| [`depth-mismatch`](depth-mismatch/README.md) | Asymmetric nesting depth — 2-level vs 3-level with intermediate grouping |
| [`embedded-simple`](embedded-simple/README.md) | Single embedded sub-entity |
| [`embedded-objects`](embedded-objects/README.md) | Nested embedded objects |
| [`embedded-multiple`](embedded-multiple/README.md) | Multiple embedded entities |
| [`embedded-vs-many-to-many`](embedded-vs-many-to-many/README.md) | Embedded vs. reference-based relationships |
| [`flattened`](flattened/README.md) | Flattened source structure into normalized target |
| [`hard-delete`](hard-delete/README.md) | Hard-delete propagation via `derive_tombstones` + `cluster_members` + `bool_or` resolution |
| [`hierarchy-merge`](hierarchy-merge/README.md) | Merging 2-level and 3-level hierarchies via cross-depth identity resolution |
| [`inserts-and-deletes`](inserts-and-deletes/README.md) | Handling new and removed records |
| [`json-fields`](json-fields/README.md) | Extracting sub-fields from JSONB source columns via `source_path` |
| [`merge-curated`](merge-curated/README.md) | Curated merge with manual overrides |
| [`merge-generated-ids`](merge-generated-ids/README.md) | Merge with system-generated identifiers |
| [`merge-groups`](merge-groups/README.md) | Group-based atomic resolution |
| [`merge-internal`](merge-internal/README.md) | Internal merge within a single source |
| [`merge-partials`](merge-partials/README.md) | Partial record merge |
| [`merge-threeway`](merge-threeway/README.md) | Three-way merge between sources |
| [`multi-value`](multi-value/README.md) | Scalar-vs-list cardinality mismatch with primary_phone + phone list |
| [`multiple-target-mappings`](multiple-target-mappings/README.md) | Multiple targets in one file |
| [`nested-arrays`](nested-arrays/README.md) | Array-of-objects field mapping |
| [`nested-arrays-deep`](nested-arrays-deep/README.md) | Deeply nested array structures |
| [`nested-arrays-multiple`](nested-arrays-multiple/README.md) | Multiple nested arrays |
| [`null-propagation`](null-propagation/README.md) | Propagating intentional NULLs via sentinel pattern |
| [`precision-loss`](precision-loss/README.md) | Handling precision loss with `normalize` on field mappings |
| [`propagated-delete`](propagated-delete/README.md) | GDPR-style deletion propagation via bool_or + reverse_filter |
| [`reference-preservation`](reference-preservation/README.md) | Preserving foreign-key references |
| [`references`](references/README.md) | Foreign-key references between targets |
| [`required-fields`](required-fields/README.md) | Required-field constraints via reverse_filter OR pattern |
| [`relationship-embedded`](relationship-embedded/README.md) | Embedded relationship mapping |
| [`relationship-mapping`](relationship-mapping/README.md) | Standalone relationship mapping |
| [`route`](route/README.md) | Routing records by field values |
| [`route-combined`](route-combined/README.md) | Combined routing logic |
| [`route-embedded`](route-embedded/README.md) | Routing within embedded objects |
| [`route-multiple`](route-multiple/README.md) | Multiple routing rules |
| [`scalar-array-deletion`](scalar-array-deletion/README.md) | Scalar array element deletion via `scalar: true` and `derive_tombstones` |
| [`soft-delete`](soft-delete/README.md) | Soft-delete detection via `soft_delete` |
| [`types`](types/README.md) | Type conversion and coercion |
| [`value-conversions`](value-conversions/README.md) | Value mapping / vocabulary conversion |
| [`value-defaults`](value-defaults/README.md) | Default values and default expressions |
| [`value-derived`](value-derived/README.md) | Derived / computed fields |
| [`value-groups`](value-groups/README.md) | Field group resolution |
| [`vocabulary-custom`](vocabulary-custom/README.md) | Custom vocabulary definitions |
| [`vocabulary-standard`](vocabulary-standard/README.md) | Standard vocabulary usage |
| [`derive-noop`](derive-noop/README.md) | Target-centric noop detection via ETL written state |
| [`passthrough`](passthrough/README.md) | Carrying unmapped source columns through to delta output via `passthrough` |
| [`element-hard-delete`](element-hard-delete/README.md) | Deletion-wins: one source's element removal wins over other sources via `written_state` |
| [`element-soft-delete`](element-soft-delete/README.md) | Element-level soft-delete: promote scalar value lists to objects with `removed_at` lifecycle metadata |

Each example directory contains a local `README.md` and a `mapping.yaml` with the full definition including test cases.

## Schema Properties Without Example Coverage

These mapping schema properties are not yet demonstrated by any example:

| Property | Description |
|---|---|
| `array_path` | Dotted path to a JSONB array nested inside a JSON object (vs `array` for top-level arrays) |
| `links` / `LinkRef` | External identity edges from a linking table |
| `link_key` | Column in a linking table providing pre-computed cluster identity |
| `cluster_members` | ETL feedback table for insert tracking |
| `cluster_field` | Source column holding a pre-populated cluster ID |
