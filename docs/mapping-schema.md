# OSI Mapping Schema Reference

> **Schema file:** [`specs/osi-mapping-schema.json`](../specs/osi-mapping-schema.json)  
> **JSON Schema draft:** 2020-12  
> **Version:** 1.0

The OSI Mapping Schema defines field mappings between data sources and OSI semantic models. A mapping file declares how columns or properties in a source system relate to fields in a target model using SQL expressions grouped by dialect. Each field mapping specifies `target_field` and optionally `source_field`, with optional `expression_forward` and `expression_reverse` for transformations. When expressions are omitted, the field is copied as-is.

---

## Document Structure

A mapping document is a JSON or YAML object with two required top-level properties:

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `version` | `string` | **Yes** | Must be `"1.0"` |
| `mappings` | `Mapping[]` | **Yes** | One or more source-to-target mapping definitions |

```yaml
version: "1.0"
mappings:
  - name: orders_to_canonical
    source: { ... }
    target: { ... }
    field_mappings: [ ... ]
```

No additional properties are allowed at the top level.

---

## Mapping

Each entry in the `mappings` array describes how one source dataset maps to one target dataset.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `name` | `string` | **Yes** | Unique identifier for this mapping. Must match `^[a-z][a-z0-9_]*$`. |
| `description` | `string` | No | Human-readable description. |
| `id` | `string` or `string[]` | No | Source field(s) that uniquely identify a row. Required when a resolution document targets the same dataset. |
| `source` | [ModelRef](#modelref) | **Yes** | Reference to the source data model. |
| `target` | [ModelRef](#modelref) | **Yes** | Reference to the target data model. |
| `source_path` | `string` | No | Dot-delimited path to a nested array in the source object. When set, the mapping operates on each item in that array. Use dot notation for deep nesting (e.g. `lines.sub_items`). |
| `parent_fields` | `object` | No | Pulls ancestor-level fields into scope under explicit aliases. Keys are alias names; values are [ParentFieldRef](#parentfieldref) objects. Only meaningful when `source_path` is set. |
| `filter_forward` | [Expression](#expression) | No | SQL WHERE condition — only source rows matching this condition are mapped forward to the target. |
| `filter_reverse` | [Expression](#expression) | No | SQL WHERE condition — only target rows matching this condition are mapped back to this source. |
| `computed_forward` | `object` | No | **Experimental.** Named computed expressions derived from source fields, available as aliases in forward expressions and `filter_forward`. Values are [Expression](#expression) objects. |
| `computed_reverse` | `object` | No | **Experimental.** Named computed expressions derived from target fields, available as aliases in reverse expressions and `filter_reverse`. Values are [Expression](#expression) objects. |
| `default_timestamp_field` | `string` | No | **Experimental.** Fallback source timestamp field for `LAST_MODIFIED` resolution. Used when a field mapping omits `timestamp_field`. |
| `embedded` | `boolean` | No | When `true`, this mapping extracts a sub-entity from the same source row as a parent mapping. The embedded entity shares the parent's source identity. |
| `field_mappings` | [FieldMapping[]](#fieldmapping) | **Yes** | One or more field-level mappings. |

### Example

```yaml
mappings:
  - name: crm_companies_to_company
    description: CRM companies to canonical company
    id: id

    source:
      schema_file: ./crm-openapi.yaml
      schema_path: "#/components/schemas/Company"
      schema_format: openapi

    target:
      semantic_model: acme_model
      dataset: company
      model_file: ./model-acme.yaml

    default_timestamp_field: updated_at

    field_mappings:
      - target_field: name
        source_field: name
        priority: 2
```

---

## ModelRef

References a data model. Can point to an OSI semantic model or an external schema.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `semantic_model` | `string` | Conditional | Name of an OSI semantic model. Required with `dataset`. |
| `dataset` | `string` | Conditional | Dataset within the semantic model. Required with `semantic_model`. |
| `model_file` | `string` | No | Path to the OSI semantic model YAML file. |
| `schema_file` | `string` | Conditional | Path to an external schema file (JSON Schema, OpenAPI, Avro, Protobuf, etc.). |
| `schema_path` | `string` | No | JSON Pointer (RFC 6901) into the `schema_file` identifying the specific schema to map. |
| `schema_format` | `string` | No | Format of the external schema file. One of: `json_schema`, `openapi`, `avro`, `protobuf`, `custom`. |

**Constraint:** Either `semantic_model` + `dataset` or `schema_file` must be provided.

### OSI model reference

```yaml
source:
  semantic_model: erp_model
  dataset: customers
  model_file: ./model-erp.yaml
```

### External schema reference

```yaml
source:
  schema_file: ./crm-openapi.yaml
  schema_path: "#/components/schemas/Company"
  schema_format: openapi
```

---

## FieldMapping

Maps a single field between source and target models.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `target_field` | `string` | **Yes** | Name of the field in the target model. |
| `source_field` | `string` | No | Name of the field in the source model. Defaults to the same value as `target_field` when omitted. |
| `expression_forward` | [Expression](#expression) | No | Expression transforming source → target. When omitted, `source_field` is copied directly. |
| `expression_reverse` | [Expression](#expression) | No | Expression transforming target → source. When omitted, `target_field` is copied directly. |
| `priority` | `integer` | No | COALESCE ordering — lower number wins. Only meaningful with `COALESCE` resolution. Minimum: `1`. |
| `timestamp_field` | `string` | No | Source field driving `LAST_MODIFIED` resolution. Overrides the mapping-level `default_timestamp_field`. |
| `required` | `boolean` | No | When `true`, the entire row is excluded from reverse output if this field's resolved value is null. Enables insert/delete propagation patterns. |
| `description` | `string` | No | Human-readable description of the mapping logic. |

### Bidirectional copy

When both expressions are omitted, the field is copied as-is in both directions:

```yaml
- target_field: email
  source_field: email
```

### Copy with metadata

Omit expressions to copy, but attach metadata like `priority`:

```yaml
- target_field: name
  source_field: name
  priority: 1
```

### Bidirectional with expressions

```yaml
- target_field: email
  source_field: customer_email
  expression_forward:
    dialects:
      - dialect: ANSI_SQL
        expression: customer_email
  expression_reverse:
    dialects:
      - dialect: ANSI_SQL
        expression: email
```

### One-way (forward only)

```yaml
- target_field: full_name
  expression_forward:
    dialects:
      - dialect: ANSI_SQL
        expression: "first_name || ' ' || last_name"
```

---

## ParentFieldRef

References an ancestor-level field, imported into scope under an alias. Only meaningful inside a mapping that uses `source_path`.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `path` | `string` | No | Dot-delimited path to the ancestor scope. Empty string means the root source object (parent of `source_path`). For deep nesting, specify an intermediate path (e.g. `lines` when `source_path` is `lines.sub_items`). |
| `field` | `string` | **Yes** | Field name within the ancestor scope. |

### Example

```yaml
source_path: lines.sub_items
parent_fields:
  order_id:
    path: ""
    field: id
  line_id:
    path: lines
    field: line_id
```

In this example, `order_id` pulls `id` from the root object, and `line_id` pulls `line_id` from the intermediate `lines` array level.

---

## Expression

All expressions use the OSI multi-dialect pattern, allowing the same mapping to carry SQL variants side by side.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `dialects` | [DialectExpression[]](#dialectexpression) | **Yes** | One or more dialect-specific expressions. |

### DialectExpression

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `dialect` | `string` | **Yes** | SQL or expression language dialect identifier (e.g. `ANSI_SQL`, `SNOWFLAKE`). |
| `expression` | `string` | **Yes** | Dialect-specific SQL expression. |

### Example

```yaml
expression_forward:
  dialects:
    - dialect: ANSI_SQL
      expression: "UPPER(name)"
    - dialect: SNOWFLAKE
      expression: "UPPER(name)"
```

---

## Filters and Computed Fields

### Forward/Reverse Filters

Use `filter_forward` and `filter_reverse` to selectively route rows. This is useful when one source maps to multiple targets, or when only a subset of target rows should reverse-map to a particular source.

```yaml
mappings:
  - name: active_customers
    filter_forward:
      dialects:
        - dialect: ANSI_SQL
          expression: "status = 'ACTIVE'"
    # ...
```

### Computed Fields (Experimental)

> **Experimental** — this feature may change in future versions.

`computed_forward` and `computed_reverse` define named expressions that can be referenced in field mappings. They act as reusable aliases scoped to the mapping.

```yaml
mappings:
  - name: orders_mapping
    computed_forward:
      total_with_tax:
        dialects:
          - dialect: ANSI_SQL
            expression: "amount * (1 + tax_rate)"
    field_mappings:
      - target_field: total
        expression_forward:
          dialects:
            - dialect: ANSI_SQL
              expression: total_with_tax
```

---

## Nested Array Mappings

The `source_path` and `parent_fields` properties enable mapping nested/embedded arrays within a source object.

When `source_path` is set, the mapping iterates over items in the specified array. `parent_fields` lets you pull outer-scope fields into the iteration context.

```yaml
mappings:
  - name: order_lines
    source_path: lines
    parent_fields:
      order_id:
        path: ""
        field: id
    source:
      semantic_model: erp
      dataset: orders
    target:
      semantic_model: canonical
      dataset: order_lines
    field_mappings:
      - target_field: order_id
        expression_forward:
          dialects:
            - dialect: ANSI_SQL
              expression: order_id
      - target_field: product
        source_field: product_code
        expression_forward:
          dialects:
            - dialect: ANSI_SQL
              expression: product_code
```

---

## Embedded Mappings

Set `embedded: true` when extracting a sub-entity from the same source row as a parent mapping. The embedded entity shares the parent's source identity and has no independent existence.

```yaml
mappings:
  - name: company_address
    embedded: true
    source:
      schema_file: ./crm-openapi.yaml
      schema_path: "#/components/schemas/Company"
      schema_format: openapi
    target:
      semantic_model: acme_model
      dataset: address
    field_mappings:
      - target_field: street
        expression_forward:
          dialects:
            - dialect: ANSI_SQL
              expression: billing_street
```

---

## YAML Language Server Header

For in-editor validation, add this header to your mapping YAML files:

```yaml
# yaml-language-server: $schema=../specs/osi-mapping-schema.json
```

Adjust the relative path to match your project structure.
