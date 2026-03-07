# Mapping Schema Reference

Schema file: [`osi-mapping-schema.json`](../specs/osi-mapping-schema.json)

A mapping document declares how fields in one source dataset map to fields in one target dataset, in both directions.

## Document Structure

```yaml
version: "1.0"
mappings:
  - name: ...
    # ... Mapping entries
```

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `version` | string | yes | Must be `"1.0"` |
| `mappings` | array of [Mapping](#mapping) | yes | One or more mapping entries |

---

## Mapping

A single source-to-target mapping for one dataset pair.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `name` | string | yes | Unique identifier. Must match `^[a-z][a-z0-9_]*$` |
| `description` | string | no | Human-readable description |
| `id` | string or string[] | no | Source field(s) that uniquely identify a row. Required when resolution targets the same dataset. |
| `source` | [ModelRef](#modelref) | yes | The source data model |
| `target` | [ModelRef](#modelref) | yes | The target data model |
| `filter_forward` | [Expression](#expression) | no | Only source rows matching this filter are mapped forward. See [Routing](#routing). |
| `filter_reverse` | [Expression](#expression) | no | Only target rows matching this filter are mapped back to this source. See [Selective Reverse](#selective-reverse). |
| `embedded` | boolean | no | Marks this mapping as an embedded sub-entity extraction. See [Embedded](#embedded). |
| `field_mappings` | array of [FieldMapping](#fieldmapping) | yes | Field-level mappings |

### Example

```yaml
- name: companies_to_company
  description: ERP companies to canonical company model
  id: id
  filter_reverse:
    dialects:
      - dialect: ANSI_SQL
        expression: "'customer' = ANY(role)"
  source:
    semantic_model: erp_model
    dataset: companies
    model_file: ./model-erp.yaml
  target:
    semantic_model: acme_inc_model
    dataset: company
    model_file: ./model-acme.yaml
  field_mappings:
    - target_field: name
      forward_expression:
        dialects:
          - dialect: ANSI_SQL
            expression: company_name
      reverse_expression:
        dialects:
          - dialect: ANSI_SQL
            expression: name
```

---

## FieldMapping

Maps a single field between source and target. Each FieldMapping must have **either** `forward_expression` (scalar mapping) **or** `nested` (array extraction), but not both.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `target_field` | string | yes | Target field name. For nested mappings, names the source array field. |
| `forward_expression` | [Expression](#expression) | one of¹ | Source → target transformation |
| `reverse_expression` | [Expression](#expression) | no | Target → source transformation. Omit for one-way mappings. |
| `nested` | [Nested](#nested) | one of¹ | Array extraction block. See [Nested](#nested). |
| `priority` | integer (≥ 1) | no | COALESCE ordering — lower number wins. See [Resolution](resolution-schema.md). |
| `timestamp_field` | string | no | Source field for LAST_MODIFIED resolution |
| `required_reverse` | boolean | no | When true, the row is excluded from reverse output if this field resolves to null. See [Required Reverse](#required-reverse). |
| `description` | string | no | Human-readable description |
| `data_loss_warning` | string | no | Warning about potential data loss in reverse |

¹ Exactly one of `forward_expression` or `nested` must be present.

### One-way vs. bi-directional

If `reverse_expression` is present, the field is bi-directional. If omitted, the field is forward-only. Common reasons to omit reverse:

- **Constants** — `forward_expression: "'customer'"` injects a literal; no source field to write back to.
- **COLLECT fields** — an array of collected values can't meaningfully reverse to a single source field.
- **Derived values** — computed expressions that don't map cleanly to a source column.

### Example

```yaml
field_mappings:
  - target_field: name
    timestamp_field: modified_at
    forward_expression:
      dialects:
        - dialect: ANSI_SQL
          expression: company_name
    reverse_expression:
      dialects:
        - dialect: ANSI_SQL
          expression: name

  - target_field: role
    forward_expression:
      dialects:
        - dialect: ANSI_SQL
          expression: "'customer'"
    # no reverse — constant value
```

---

## ModelRef

Reference to a data model. Supports two forms:

### OSI semantic model

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `semantic_model` | string | yes | Name of the OSI semantic model |
| `dataset` | string | yes | Dataset within the model |
| `model_file` | string | no | Path to the OSI model YAML file |

### External schema

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `schema_file` | string | yes | Path to external schema (OpenAPI, JSON Schema, Avro, etc.) |
| `schema_path` | string | no | JSON Pointer into the schema file |
| `schema_format` | string | no | One of: `json_schema`, `openapi`, `avro`, `protobuf`, `custom` |

### Examples

```yaml
# OSI model reference
source:
  semantic_model: erp_model
  dataset: companies
  model_file: ./model-erp.yaml

# External schema reference
source:
  schema_file: ./webshop-openapi.yaml
  schema_path: "#/components/schemas/Order"
  schema_format: openapi
```

---

## Expression

All expressions use multi-dialect syntax. This allows the same mapping to work across different SQL engines.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `dialects` | array of [DialectExpression](#dialectexpression) | yes | One or more dialect-specific expressions |

### DialectExpression

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `dialect` | string | yes | Dialect identifier (e.g. `ANSI_SQL`, `SNOWFLAKE`, `BIGQUERY`) |
| `expression` | string | yes | The expression in that dialect |

### Example

```yaml
forward_expression:
  dialects:
    - dialect: ANSI_SQL
      expression: "UPPER(customer_name)"
    - dialect: SNOWFLAKE
      expression: "UPPER(customer_name)"
```

---

## Patterns

### Routing

Use `filter_forward` on Mapping to send different source rows to different targets. Multiple Mapping entries can share the same source, each with a different filter.

```yaml
# Route companies
- name: customers_to_company
  filter_forward:
    dialects:
      - dialect: ANSI_SQL
        expression: "customer_type = 'company'"
  source: { semantic_model: crm_model, dataset: customers }
  target: { semantic_model: acme_inc_model, dataset: company }

# Route persons
- name: customers_to_person
  filter_forward:
    dialects:
      - dialect: ANSI_SQL
        expression: "customer_type = 'person'"
  source: { semantic_model: crm_model, dataset: customers }
  target: { semantic_model: acme_inc_model, dataset: person }
```

### Selective Reverse

Use `filter_reverse` on Mapping to limit which target rows are written back to a source. Useful when a source only handles a subset of the canonical data.

```yaml
- name: companies_to_company
  filter_reverse:
    dialects:
      - dialect: ANSI_SQL
        expression: "'customer' = ANY(role)"
  # Only rows where role includes 'customer' are sent back to ERP
```

### Required Reverse

Use `required_reverse: true` on a FieldMapping to exclude rows from reverse output when a field resolves to null. This enables insert/delete propagation — if a required field has no value, the record isn't sent to the source.

```yaml
- target_field: account
  priority: 1
  required_reverse: true
  forward_expression:
    dialects:
      - dialect: ANSI_SQL
        expression: customer_account
```

### Embedded

Use `embedded: true` on a Mapping to extract a sub-entity from the same source row as a parent mapping. The embedded entity:

- **Shares identity** with the parent source row (no independent `id`).
- **Has no independent existence** — deleting the parent deletes the embedded entity.
- **Reverse joins back** to the parent source row (not emitted as a separate table).

The parent is the non-embedded Mapping with the same source dataset. Tooling correlates them by shared source identity.

```yaml
# Parent mapping (routing)
- name: customers_to_company
  id: id
  source: { semantic_model: crm_model, dataset: customers }
  target: { semantic_model: acme_inc_model, dataset: company }
  field_mappings: [...]

# Embedded — same source, different target
- name: customers_to_address
  embedded: true
  source: { semantic_model: crm_model, dataset: customers }
  target: { semantic_model: acme_inc_model, dataset: address }
  field_mappings:
    - target_field: street
      forward_expression:
        dialects:
          - dialect: ANSI_SQL
            expression: billing_street
      reverse_expression:
        dialects:
          - dialect: ANSI_SQL
            expression: street
```

### Nested

Use `nested` on a FieldMapping to extract items from a source array field into a flat target dataset. This applies only when the source has array-of-objects fields (OpenAPI, JSON Schema, etc.) — OSI models are flat and don't have array types.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `target` | [ModelRef](#modelref) | yes | Flat target dataset for extracted items |
| `id` | string or string[] | no | Field(s) within each array item that uniquely identify it |
| `filter_forward` | [Expression](#expression) | no | Filter for routing array items to different targets |
| `filter_reverse` | [Expression](#expression) | no | Filter for reconstructing the array in reverse |
| `embedded` | boolean | no | Marks this as embedded sub-entity extraction from array items |
| `field_mappings` | array of [FieldMapping](#fieldmapping) | yes | Mappings from array item properties to target fields |

When `nested` is present:
- `target_field` names the **source array field** (e.g., `lines`).
- `forward_expression` and `reverse_expression` are **omitted** — the nested block replaces them.
- The nested `field_mappings` support all normal FieldMapping features.
- Nested blocks can be **recursive** — a nested field_mapping can itself have a `nested` block for deep nesting.

#### Basic nested extraction

```yaml
- target_field: lines
  nested:
    target:
      semantic_model: acme_inc_model
      dataset: order_line
    id: line_num
    field_mappings:
      - target_field: product_id
        forward_expression:
          dialects:
            - dialect: ANSI_SQL
              expression: product_id
```

#### Nested routing

Multiple FieldMapping entries can share the same `target_field`, each with its own `nested` block and `filter_forward`. This routes different array items to different targets — the nested analog of top-level routing.

```yaml
# Route product lines → order_line
- target_field: lines
  nested:
    target: { semantic_model: acme_inc_model, dataset: order_line }
    id: line_num
    filter_forward:
      dialects:
        - dialect: ANSI_SQL
          expression: "line_type = 'product'"
    field_mappings: [...]

# Route discount lines → order_discount
- target_field: lines
  nested:
    target: { semantic_model: acme_inc_model, dataset: order_discount }
    id: line_num
    filter_forward:
      dialects:
        - dialect: ANSI_SQL
          expression: "line_type = 'discount'"
    field_mappings: [...]
```

#### Nested embedding

Use `embedded: true` inside a `nested` block to extract denormalized data from array items into a separate entity. Same semantics as top-level embedded, scoped to the array item context.

```yaml
# Extract product info denormalized in line items → product dataset
- target_field: lines
  nested:
    embedded: true
    target: { semantic_model: acme_inc_model, dataset: product }
    id: product_id
    filter_forward:
      dialects:
        - dialect: ANSI_SQL
          expression: "line_type = 'product'"
    field_mappings:
      - target_field: product_name
        forward_expression:
          dialects:
            - dialect: ANSI_SQL
              expression: product_name
```

### Reverse Direction

In the reverse direction:

- **Scalar fields**: `reverse_expression` transforms the target value back to the source column.
- **Routing**: `filter_forward` is inverted — tooling knows which target dataset maps back to which source rows.
- **Embedded**: Embedded fields are joined back to the parent source row.
- **Nested**: Flat target dataset rows are reassembled into an array and embedded back into the parent source object. For nested routing, items from multiple target datasets merge back into one array.
