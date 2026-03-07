# OSI Mapping — Concepts

The OSI Mapping specification defines **bi-directional field mappings** between data sources and a canonical [OSI semantic model](https://github.com/BaardBouvet/osi). It answers three questions:

1. **Forward** — how does source data flow into the canonical model?
2. **Reverse** — how does canonical data flow back to each source?
3. **Resolution** — when multiple sources contribute to the same target field, which value wins?

## Core Idea

A typical enterprise has many systems (ERP, CRM, APIs, …) that describe overlapping entities. OSI Mapping lets you declare, per field, how each source maps to a single canonical target — and how to get data back out. This is done in plain YAML files that reference the schemas on both sides.

## Documents

The spec uses three document types, each with its own JSON Schema:

| Document | Schema | Purpose |
|----------|--------|---------|
| **Mapping** | `osi-mapping-schema.json` | Declares field-level forward/reverse mappings between one source and one target |
| **Resolution** | `osi-resolution-schema.json` | Declares per-field conflict resolution for target datasets with multiple sources |
| **OSI Model** | `osi-schema.json` | The canonical semantic model (defined by the OSI spec, not this project) |

Mapping and resolution are **separate concerns**. A mapping file knows nothing about which other mappings exist; a resolution file knows nothing about which sources exist. Tooling combines them at runtime.

## Directionality

Every field mapping has a **forward expression** (source → target). A **reverse expression** (target → source) is optional — omit it when the mapping is inherently one-way (e.g., a constant value injected into the target, or a COLLECT field where an array can't reverse to a single value).

## Sources

A source can be:

- An **OSI semantic model** — referenced by `semantic_model` + `dataset` + `model_file`.
- An **external schema** — referenced by `schema_file` + `schema_path` + `schema_format` (OpenAPI, JSON Schema, Avro, Protobuf, etc.).

OSI models are flat/relational. External schemas may have nested structures (arrays of objects), which is where [nested extraction](mapping-schema.md#nested) comes in.

## Expressions

All expressions (forward, reverse, filters) use multi-dialect syntax:

```yaml
forward_expression:
  dialects:
    - dialect: ANSI_SQL
      expression: customer_name
```

This allows the same mapping to carry expressions for multiple SQL dialects (ANSI_SQL, SNOWFLAKE, BIGQUERY, etc.). Tooling picks the dialect it supports.

## Key Patterns

| Pattern | Mechanism | Example |
|---------|-----------|---------|
| **Merge** | Multiple mappings target the same dataset; resolution picks winners | ERP + CRM → company |
| **Routing** | `filter_forward` on Mapping sends different source rows to different targets | CRM customers split by `customer_type` |
| **Selective reverse** | `filter_reverse` on Mapping limits which target rows flow back | Only customers (not suppliers) write back to ERP |
| **Embedded** | `embedded: true` on Mapping extracts a sub-entity from the same source row | Billing address fields → address dataset |
| **Array extraction** | `source_path` on Mapping flattens array items into a separate target | API order lines[] → order_line rows |
| **Array routing** | `filter_forward` + `source_path` routes array items by type | Product lines vs. discount lines |
| **Array embedding** | `embedded: true` + `source_path` extracts denormalized data from array items | Product info on line items → product dataset |
| **Parent context** | `parent_fields` pulls ancestor fields into scope as aliases | Parent `order_id` available in line item mapping |
| **Atomic groups** | `groups` on Resolution lists fields that resolve together from one source | street + city + postal_code always come from same source |
| **Vocabulary** | Map each source's lookup table into a shared target entity; resolution links on common key; FK resolved via source identity tracing | ERP Norwegian + CRM English country_lookup → canonical country entity |
| **FK resolution** | OSI `Relationship` declares FKs on target model; tooling matches `forward_expression` values against `to_columns` | `order_line.order_ref → order.order_id` |

## File Layout

A typical project:

```
example/
  model-acme.yaml          # Target OSI model
  model-erp.yaml           # Source OSI model (ERP)
  model-crm.yaml           # Source OSI model (CRM)
  webshop-openapi.yaml     # Source external schema (OpenAPI)
  mapping-erp.yaml         # ERP → Acme mappings
  mapping-crm.yaml         # CRM → Acme mappings (routing + embedded)
  mapping-webshop.yaml     # Webshop API → Acme mappings (source_path + parent_fields)
  resolution-acme.yaml     # Resolution rules for Acme target
specs/
  osi-mapping-schema.json
  osi-resolution-schema.json
  osi-schema.json
```

## Schema Reference

| Document | Description |
|----------|-------------|
| [Mapping Schema](mapping-schema.md) | Full reference for mappings, field mappings, expressions, routing, embedding, source paths, and parent fields |
| [Resolution Schema](resolution-schema.md) | Full reference for conflict resolution strategies (COALESCE, LAST_MODIFIED, COLLECT) and resolution groups |
| [FK Resolution](fk-resolution.md) | FK resolution patterns — same-source, cross-source, vocabulary normalization, OpenAPI nested FKs |
| [Derived Fields](derived-fields.md) | Derived fields, model expressions, and resolution group patterns |
