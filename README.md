# OSI Mapping

> **Work in progress** — this project is under active development and not yet stable.

OSI Mapping is a schema and set of conventions for declaring bi-directional field mappings between data sources and [OSI semantic models](https://github.com/Open-Model-Initiative). It lets you describe how columns or properties in a source system relate to fields in a target model — and vice versa — using SQL expressions grouped by dialect.

## Key ideas

- **Symmetric model references** — both source and target are described with the same `model_ref` structure, whether they are OSI semantic models or external schemas (OpenAPI, JSON Schema, etc.).
- **Forward & reverse expressions** — each field mapping carries a `forward_expression` (source → target) and an optional `reverse_expression` (target → source), enabling bi-directional lineage and view generation.
- **Dialect blocks** — expressions use the OSI `dialects` pattern so the same mapping can carry SQL variants side by side.
- **Format-agnostic sources** — sources can be OSI models, OpenAPI specs, JSON Schema files, or anything reachable via a file reference and a `schema_format` hint.

## Repository layout

```
specs/
  osi-mapping-schema.json   # JSON Schema (draft-07) for mapping files
  osi-schema.json            # OSI semantic model schema (reference)
examples/
  osi-to-osi-minimal/        # OSI model → OSI model mapping example
  openapi-to-osi-minimal/    # OpenAPI spec → OSI model mapping example
docs/
  adr/                       # Architecture Decision Records
PLAN.md                      # Internal planning notes
```

## Quick start

1. Author or reference an OSI semantic model (the **target**).
2. Describe your source — either as another OSI model or by pointing to an external schema file.
3. Create a mapping YAML that pairs source fields to target fields with SQL expressions.
4. Validate the mapping against `specs/osi-mapping-schema.json`.

Each mapping file supports a YAML language-server header for in-editor validation:

```yaml
# yaml-language-server: $schema=../../specs/osi-mapping-schema.json
```

## Examples

| Example | Source type | Description |
|---------|-----------|-------------|
| [osi-to-osi-minimal](examples/osi-to-osi-minimal/) | OSI model | ERP system (Norwegian columns) mapped to a company semantic model |
| [openapi-to-osi-minimal](examples/openapi-to-osi-minimal/) | OpenAPI 3.0 | REST API company schema mapped to the same semantic model |

## Status

This project is in early design. The mapping schema is functional but subject to change. SQL view generation and non-SQL dialect support are planned but not yet implemented.

See [docs/adr/](docs/adr/) for recorded design decisions.

## License

Apache License 2.0. See [LICENSE](LICENSE).
