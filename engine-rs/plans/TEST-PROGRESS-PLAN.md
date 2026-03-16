# Test progress

**Status:** Done

## Generic Test Runner

Added `execute_all_examples` integration test that:
1. Discovers all examples with `tests:` sections
2. Parses + renders each example
3. Skips examples without sync views (no delta to verify)
4. Executes SQL views against PostgreSQL (testcontainers)
5. Verifies expected updates/inserts/deletes against actual delta view output
6. Reports a pass/fail/skip summary at the end (non-panicking)

Run with: `cargo test --test integration execute_all_examples -- --nocapture`

Filter: `OSI_EXAMPLES=route,hello-world cargo test execute_all`

## Test Suite

- **10 unit tests**: parser, DAG, validator (5 passes), forward view column matching
- **11 integration tests**: parse_all, render_all, list_testable, execute_hello_world, execute_references, execute_route, 3× dump intermediates, execute_all_examples
- **35/35 examples**: All passing E2E (execute_all_examples)

## Feature Coverage

All 35 examples exercise the full pipeline (forward → identity → resolution → reverse → delta):

| Feature | Examples |
|---------|----------|
| Basic merge (coalesce) | hello-world, minimal, merge-internal, merge-threeway, flattened |
| Identity linking | hello-world, merge-curated, merge-generated-ids, merge-groups |
| Last-modified resolution | hello-world, value-groups, concurrent-detection |
| Expression fields | custom-resolution, types, value-conversions, value-derived |
| Default values | value-defaults |
| Group resolution | value-groups, value-derived |
| References (FK) | references, composite-keys, vocabulary-custom, vocabulary-standard |
| Composite keys | composite-keys, relationship-embedded, relationship-mapping |
| Reverse expressions | value-conversions, merge-partials |
| Reverse filter/required | inserts-and-deletes, merge-partials, route, route-combined |
| Routing (filter) | route, route-combined, route-embedded, route-multiple |
| Embedded mappings | embedded-simple, embedded-multiple, embedded-objects, embedded-vs-many-to-many, multiple-target-mappings, route-embedded |
| Relationship mappings | relationship-embedded, relationship-mapping |
| Nested arrays (path) | nested-arrays, nested-arrays-deep, nested-arrays-multiple |
| Cluster/origin | merge-generated-ids, merge-curated |
| Noop detection (_base) | concurrent-detection, all examples with single-source round-trips |
| Insert/delete propagation | inserts-and-deletes, relationship-embedded, relationship-mapping, vocabulary-custom, vocabulary-standard |
| Target field types | composite-keys (numeric), value-defaults (numeric, boolean), custom-resolution (numeric), route-multiple (boolean), merge-partials (boolean) |
| Reference preservation | reference-preservation |
