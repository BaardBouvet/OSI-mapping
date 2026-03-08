# Integration Mapping Schema

A declarative schema for defining how fields from multiple source systems map to a shared target model—and how conflicts between sources are resolved.

One YAML file describes the full picture: target entities, field mappings, resolution strategies, and test cases.

## Quick Example

```yaml
version: "1.0"
description: Two systems, one shared contact, synced by email.

targets:
  contact:
    fields:
      email: identity
      name: coalesce

mappings:
  - name: crm
    source: { dataset: crm }
    target: contact
    fields:
      - source: email
        target: email
      - source: name
        target: name
        priority: 1

  - name: erp
    source: { dataset: erp }
    target: contact
    fields:
      - source: email
        target: email
      - source: name
        target: name
        priority: 2

tests:
  - description: "CRM name wins (priority 1) and propagates to ERP"
    input:
      crm:
        - { id: "1", email: "alice@example.com", name: "Alice" }
      erp:
        - { id: "100", email: "alice@example.com", name: "A. Smith" }
    expected:
      crm:
        updates:
          - { id: "1", email: "alice@example.com", name: "Alice" }
      erp:
        updates:
          - { id: "100", email: "alice@example.com", name: "Alice" }
```

Two sources share a `contact` entity. Records are matched by `email` (identity). When names conflict, `coalesce` picks the value from the source with the highest priority (lowest number wins). The test verifies that CRM's name propagates to ERP.

## Structure

```
spec/
  mapping-schema.json    # JSON Schema (Draft 2020-12)
examples/
  hello-world/           # Simplest possible mapping (start here)
  minimal/               # Minimal multi-source mapping
  ...                    # 35 examples covering all features
docs/
  ai-guidelines.md       # Guidelines for AI agents working with mapping files
  design-rationale.md    # Design decisions and rationale
validate.py              # Multi-pass validator
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
python3 validate.py

# Validate a specific file
python3 validate.py examples/hello-world/mapping.yaml

# Verbose output
python3 validate.py -v

# Quiet mode (summary only)
python3 validate.py -q
```

### Install dependencies

```bash
pip install jsonschema pyyaml
pip install sqlglot  # optional, for SQL expression validation
```

## Examples

| Example | Demonstrates |
|---|---|
| `hello-world` | Simplest mapping — two sources, one target, coalesce |
| `minimal` | Minimal complete mapping with identity + resolution |
| `composite-keys` | Multi-field identity (compound match key) |
| `concurrent-detection` | Detecting and handling concurrent edits |
| `custom-resolution` | Custom resolution strategy via expression |
| `embedded-simple` | Single embedded sub-entity |
| `embedded-objects` | Nested embedded objects |
| `embedded-multiple` | Multiple embedded entities |
| `embedded-vs-many-to-many` | Embedded vs. reference-based relationships |
| `flattened` | Flattened source structure into normalized target |
| `inserts-and-deletes` | Handling new and removed records |
| `merge-curated` | Curated merge with manual overrides |
| `merge-generated-ids` | Merge with system-generated identifiers |
| `merge-groups` | Group-based atomic resolution |
| `merge-internal` | Internal merge within a single source |
| `merge-partials` | Partial record merge |
| `merge-threeway` | Three-way merge between sources |
| `multiple-target-mappings` | Multiple targets in one file |
| `nested-arrays` | Array-of-objects field mapping |
| `nested-arrays-deep` | Deeply nested array structures |
| `nested-arrays-multiple` | Multiple nested arrays |
| `reference-preservation` | Preserving foreign-key references |
| `references` | Foreign-key references between targets |
| `relationship-embedded` | Embedded relationship mapping |
| `relationship-mapping` | Standalone relationship mapping |
| `route` | Routing records by field values |
| `route-combined` | Combined routing logic |
| `route-embedded` | Routing within embedded objects |
| `route-multiple` | Multiple routing rules |
| `types` | Type conversion and coercion |
| `value-conversions` | Value mapping / vocabulary conversion |
| `value-defaults` | Default values and default expressions |
| `value-derived` | Derived / computed fields |
| `value-groups` | Field group resolution |
| `vocabulary-custom` | Custom vocabulary definitions |
| `vocabulary-standard` | Standard vocabulary usage |

Each example directory contains a `README.md` explaining the scenario and a `mapping.yaml` with the full definition including test cases.

## License

See [LICENSE](LICENSE).
