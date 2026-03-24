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
| [`asymmetric-io`](asymmetric-io/README.md) | Asymmetric field direction — `forward_only`, `reverse_only`, and `bidirectional` in one mapping |
| [`composite-keys`](composite-keys/README.md) | Multi-field identity via `link_group` (compound match key) |
| [`concurrent-detection`](concurrent-detection/README.md) | Detecting concurrent edits via `include_base` |
| [`crdt-ordering`](crdt-ordering/README.md) | Deterministic array element ordering via `order: true` |
| [`crdt-ordering-linked`](crdt-ordering-linked/README.md) | CRDT linked-list adjacency pointers via `order_prev` / `order_next` with `last_modified` resolution |
| [`depth-mismatch`](depth-mismatch/README.md) | Asymmetric nesting depth — 2-level vs 3-level cross-source merge |
| [`derive-noop`](derive-noop/README.md) | Target-centric noop detection via `written_state` + `derive_noop` |
| [`derive-timestamps`](derive-timestamps/README.md) | Per-field change detection via `derive_timestamps` |
| [`element-last-modified`](element-last-modified/README.md) | Element-set resolution by most recent timestamp via `elements: last_modified` |
| [`element-priority`](element-priority/README.md) | Element-set resolution via `elements: coalesce` on child targets |
| [`embedded-objects`](embedded-objects/README.md) | Embedded sub-entities via `parent:` mappings |
| [`embedded-vs-many-to-many`](embedded-vs-many-to-many/README.md) | Embedded ↔ junction table structural conversion |
| [`external-links`](external-links/README.md) | External identity linkage via `links` / `LinkRef` + `link_key` |
| [`flattened`](flattened/README.md) | Flat target from nested source structures |
| [`hard-delete`](hard-delete/README.md) | Hard-delete propagation via `derive_tombstones` + `cluster_members` |
| [`hello-world`](hello-world/README.md) | Simplest mapping — two sources, one target, identity + coalesce |
| [`inserts-and-deletes`](inserts-and-deletes/README.md) | Insert suppression via `reverse_required` |
| [`json-fields`](json-fields/README.md) | JSONB sub-field extraction via `source_path` |
| [`json-opaque`](json-opaque/README.md) | Whole JSON values mapped as atomic blobs (`type: jsonb`) |
| [`merge-curated`](merge-curated/README.md) | Human-curated merge via explicit linkage tables |
| [`merge-internal`](merge-internal/README.md) | Single-source deduplication |
| [`merge-threeway`](merge-threeway/README.md) | Three-way merge via transitive identity closure |
| [`multi-value`](multi-value/README.md) | Scalar ↔ list cardinality mismatch |
| [`multiple-target-mappings`](multiple-target-mappings/README.md) | Multiple targets from one source |
| [`nested-array-path`](nested-array-path/README.md) | Nested array-of-arrays via `array_path` dotted path |
| [`nested-arrays`](nested-arrays/README.md) | Array-of-objects via `parent:` + `array:` |
| [`nested-arrays-deep`](nested-arrays-deep/README.md) | Multi-level nesting with `parent_fields` chains |
| [`passthrough`](passthrough/README.md) | Unmapped source columns via `passthrough:` |
| [`precision-loss`](precision-loss/README.md) | Lossy noop comparison via `normalize` |
| [`reference-preservation`](reference-preservation/README.md) | FK preservation after entity merge |
| [`references`](references/README.md) | Cross-entity foreign keys via `references:` |
| [`relationship-mapping`](relationship-mapping/README.md) | Many-to-many relationship mapping with `link_group` |
| [`required-fields`](required-fields/README.md) | Data quality gates via `reverse_filter` |
| [`route`](route/README.md) | Discriminator-based routing via `filter:` |
| [`route-combined`](route-combined/README.md) | Routing + dedicated sources merging |
| [`scalar-array`](scalar-array/README.md) | Bare scalar array element extraction via `scalar: true` |
| [`sesam-annotated`](sesam-annotated/README.md) | Sesam DTL annotated example — nested arrays, `reverse_expression`, `normalize`, `expression` constant, FK references |
| [`soft-delete`](soft-delete/README.md) | Soft-delete detection via `soft_delete:` |
| [`soft-delete-child`](soft-delete-child/README.md) | Element-level soft-delete on nested array child mappings |
| [`soft-delete-resurrect`](soft-delete-resurrect/README.md) | Soft-delete resurrection via `soft_delete.target` + `BOOL_AND` consensus |
| [`value-defaults`](value-defaults/README.md) | Fallback values via `default` and `default_expression` |
| [`value-groups`](value-groups/README.md) | Atomic field group resolution via `group:` |
| [`vocabulary-standard`](vocabulary-standard/README.md) | Vocabulary targets with `references_field` |

Each example directory contains a local `README.md` and a `mapping.yaml` with the full definition including test cases.
