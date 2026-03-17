# Introduction

OSI Mapping is a declarative schema for defining how fields from multiple
source systems map to a shared target model — and how conflicts between
sources are resolved.

One YAML file describes the full picture: target entities, field mappings,
resolution strategies, and test cases. All related entities (e.g., company,
contact, country) belong in the same file — not one file per entity. This
is essential for cross-entity references and holistic conflict resolution.

## Quick start

- Start here: [hello-world example](https://github.com/OWNER/osi-mapping/tree/main/examples/hello-world)
- Step-by-step walkthrough: [Annotated example](reference/annotated-example.md)
- Full schema reference: [Schema reference](reference/schema-reference.md)
- Why this project exists: [Motivation](motivation.md)
- Design background and tradeoffs: [Design rationale](design/design-rationale.md)

## Resolution strategies

Each target field declares a resolution strategy that determines how
conflicts between sources are handled:

| Strategy | Purpose | Requirement |
|---|---|---|
| `identity` | Match records across sources (composite key when multiple fields) | At least one per target |
| `coalesce` | Pick best non-null value by priority | `priority` on field mappings |
| `last_modified` | Most recently changed value wins | Mapping-level `last_modified` timestamp field |
| `expression` | SQL expression computes the value | `expression` on field mappings |
| `collect` | Gather all values (no conflict resolution) | — |
| `bool_or` | True if any source is true | — |

## Key features

- **Composite keys** — Multiple `identity` fields form a compound match key
- **Embedded objects** — Nested sub-entities with their own identity and resolution
- **Nested arrays** — `source.path` + `parent_fields` for array-of-objects
- **References & FK resolution** — Declare foreign keys between target entities
- **Groups** — Atomic resolution (all-or-nothing) for related fields
- **Link groups** — Multi-field composite identity (e.g., first\_name + last\_name + dob)
- **Filters** — `filter` / `reverse_filter` to scope which source records qualify
- **Derived fields** — `default`, `default_expression`, direction control
- **Vocabulary** — Value conversion between source/target vocabularies
- **Tests** — Inline test cases with `input` → `expected` per dataset

## Engine

The reference engine (`engine-rs/`) is a Rust CLI that compiles mapping YAML
into a DAG of PostgreSQL views:

```bash
cd engine-rs
cargo run -- render ../examples/hello-world/mapping.yaml
cargo run -- validate ../examples/
```

See the [engine-rs README](https://github.com/OWNER/osi-mapping/tree/main/engine-rs)
for full documentation.
