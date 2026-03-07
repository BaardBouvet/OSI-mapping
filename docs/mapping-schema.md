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
| `source_path` | string | no | Dot-delimited path to a source array field. When set, this mapping operates on each array item. See [Source Path](#source-path). |
| `parent_fields` | object&lt;string, [ParentFieldRef](#parentfieldref)&gt; | no | Pulls ancestor fields into scope as aliases. Only meaningful with `source_path`. See [Parent Fields](#parent-fields). |
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

Maps a single field between source and target.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `target_field` | string | yes | Target field name |
| `forward_expression` | [Expression](#expression) | yes | Source → target transformation |
| `reverse_expression` | [Expression](#expression) | no | Target → source transformation. Omit for one-way mappings. |
| `priority` | integer (≥ 1) | no | COALESCE ordering — lower number wins. See [Resolution](resolution-schema.md). |
| `timestamp_field` | string | no | Source field for LAST_MODIFIED resolution |
| `required_reverse` | boolean | no | When true, the row is excluded from reverse output if this field resolves to null. See [Required Reverse](#required-reverse). |
| `description` | string | no | Human-readable description |
| `data_loss_warning` | string | no | Warning about potential data loss in reverse |

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

### Source Path

Use `source_path` on a Mapping to extract items from a source array field into a flat target dataset. This replaces the need for recursive nesting — each extraction is a separate, flat Mapping entry.

This applies when the source has array-of-objects fields (OpenAPI, JSON Schema, etc.) — OSI models are flat and don't have array types.

```yaml
- name: api_orders_to_order_line
  source:
    schema_file: ./webshop-openapi.yaml
    schema_path: "#/components/schemas/Order"
  source_path: lines
  target: { semantic_model: acme_inc_model, dataset: order_line }
  id: line_num
  field_mappings:
    - target_field: product_id
      forward_expression:
        dialects:
          - dialect: ANSI_SQL
            expression: product_id
```

For deep nesting, use dot notation: `source_path: lines.sub_items`.

All top-level Mapping features work identically with `source_path` — routing (`filter_forward`), selective reverse (`filter_reverse`), and embedding (`embedded`) all apply to the array items without any special "nested variant".

#### Routing array items

Multiple Mappings can share the same source and `source_path`, each with a different `filter_forward`. This routes array items to different targets — the same pattern as top-level routing:

```yaml
# Route product lines → order_line
- name: api_orders_to_order_line
  source_path: lines
  filter_forward:
    dialects:
      - dialect: ANSI_SQL
        expression: "line_type = 'product'"
  target: { semantic_model: acme_inc_model, dataset: order_line }
  field_mappings: [...]

# Route discount lines → order_discount
- name: api_orders_to_order_discount
  source_path: lines
  filter_forward:
    dialects:
      - dialect: ANSI_SQL
        expression: "line_type = 'discount'"
  target: { semantic_model: acme_inc_model, dataset: order_discount }
  field_mappings: [...]
```

#### Embedding from array items

Use `embedded: true` with `source_path` to extract denormalized data from array items. Same semantics as top-level embedded:

```yaml
- name: api_orders_to_product
  embedded: true
  source_path: lines
  filter_forward:
    dialects:
      - dialect: ANSI_SQL
        expression: "line_type = 'product'"
  target: { semantic_model: acme_inc_model, dataset: product }
  field_mappings:
    - target_field: product_name
      forward_expression:
        dialects:
          - dialect: ANSI_SQL
            expression: product_name
```

### Parent Fields

When using `source_path`, expressions operate on array item fields by default. Use `parent_fields` to pull ancestor-level fields into scope under explicit aliases. This keeps expressions as pure SQL — no magic prefixes.

```yaml
- name: api_orders_to_order_line
  source_path: lines
  parent_fields:
    parent_order_id:
      path: ""             # "" = root source object
      field: order_id
  field_mappings:
    - target_field: order_ref
      forward_expression:
        dialects:
          - dialect: ANSI_SQL
            expression: parent_order_id    # alias — valid SQL identifier
```

For deep nesting (e.g., `source_path: lines.sub_items`), set `path` to an intermediate ancestor:

```yaml
parent_fields:
  root_order_id:
    path: ""               # root Order object
    field: order_id
  parent_line_num:
    path: lines             # parent array item
    field: line_num
```

---

## ParentFieldRef

Reference to an ancestor-level field, imported into scope under an alias.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `path` | string | no | Dot-delimited path to the ancestor scope. `""` = root source object. |
| `field` | string | yes | Field name within the ancestor scope |

### Reverse Direction

In the reverse direction:

- **Scalar fields**: `reverse_expression` transforms the target value back to the source column.
- **Routing**: `filter_forward` is inverted — tooling knows which target dataset maps back to which source rows.
- **Embedded**: Embedded fields are joined back to the parent source row.
- **Source path**: Flat target dataset rows are reassembled into an array and placed back into the parent source object. For routed array items, rows from multiple target datasets merge back into one array. Parent field aliases are resolved back to their ancestor locations.
