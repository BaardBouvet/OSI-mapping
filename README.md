# Integration Mapping Schema

A declarative schema for defining how fields from multiple source systems map to a shared target model—and how conflicts between sources are resolved.

One YAML file describes the full picture: target entities, field mappings, resolution strategies, and test cases. All related entities (e.g., company, contact, country) belong in the same file — not one file per entity. This is essential for cross-entity references and holistic conflict resolution.

## Quick Start

Use the docs and examples instead of learning from a dense inline snippet:

- Start here: [`examples/hello-world/README.md`](examples/hello-world/README.md)
- Step-by-step walkthrough: [`docs/annotated-example.md`](docs/annotated-example.md)
- Full schema reference: [`docs/schema-reference.md`](docs/schema-reference.md)
- Why this project exists: [`docs/motivation.md`](docs/motivation.md)
- Design background and tradeoffs: [`docs/design-rationale.md`](docs/design-rationale.md)
- AI authoring guidance: [`docs/ai-guidelines.md`](docs/ai-guidelines.md)



## Structure

```
spec/
  mapping-schema.json    # JSON Schema (Draft 2020-12)
examples/
  hello-world/           # Simplest possible mapping (start here)
  ...                    # 34 more examples covering all features
docs/
  ai-guidelines.md       # Guidelines for AI agents working with mapping files
  design-rationale.md    # Design decisions and rationale
engine-rs/               # Rust reference engine (YAML → PostgreSQL views)
validation/
  validate.py            # Multi-pass validator
```

## Resolution Strategies

Each target field declares a resolution strategy that determines how conflicts between sources are handled:

| Strategy | Purpose | Requirement |
|---|---|---|
| `identity` | Match records across sources (composite key when multiple fields) | At least one per target |
| `coalesce` | Pick best non-null value by priority | `priority` on field mappings |
| `last_modified` | Most recently changed value wins | Mapping-level `last_modified` timestamp field |
| `expression` | SQL expression computes the value | `expression` on field mappings |
| `collect` | Gather all values (no conflict resolution) | — |

## Key Features

- **Composite keys** — Multiple `identity` fields form a compound match key
- **Embedded objects** — Nested sub-entities with their own identity and resolution
- **Nested arrays** — `source.path` + `parent_fields` for array-of-objects
- **References & FK resolution** — Declare foreign keys between target entities. When entities merge during identity linking, local IDs in referencing records are automatically preserved — each source keeps its own FK value pointing to the correct local record. Building this by hand across systems with independent ID namespaces is one of the hardest integration problems.
- **Groups** — Atomic resolution (all-or-nothing) for related fields
- **Link groups** — Multi-field composite identity (e.g., first_name + last_name + dob)
- **Filters** — `filter` / `reverse_filter` to scope which source records qualify
- **Derived fields** — `default`, `default_expression`, direction control
- **Vocabulary** — Value conversion between source/target vocabularies
- **Tests** — Inline test cases with `input` → `expected` per dataset (`updates`, `inserts`, `deletes`)

## Validation

The validator performs 7 passes:

1. **JSON Schema** — Structural validity against `mapping-schema.json`
2. **Unique names** — No duplicate mapping names or duplicate field targets
3. **Target references** — Mapping `target` and field `target` refer to declared entities/fields
4. **Strategy consistency** — Required properties present for each strategy
5. **Field coverage** — All target fields have at least one mapping
6. **Test datasets** — Test `input`/`expected` dataset names match mapping sources
7. **SQL syntax** — Expression fields parse as valid SQL (requires `sqlglot`)

### Run

```bash
# Validate all examples
python3 validation/validate.py

# Validate a specific file
python3 validation/validate.py examples/hello-world/mapping.yaml

# Verbose output
python3 validation/validate.py -v

# Quiet mode (summary only)
python3 validation/validate.py -q
```

### Install dependencies

```bash
pip install jsonschema pyyaml
pip install sqlglot  # optional, for SQL expression validation
```

## Examples

See [`examples/README.md`](examples/README.md) for the full example catalog and what each scenario demonstrates.

## License

See [LICENSE](LICENSE).
