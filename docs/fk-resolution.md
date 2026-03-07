# Foreign Keys and Relationships

FK resolution uses **no new mapping primitives**. It relies entirely on OSI `Relationship` declarations in the target semantic model combined with existing mapping properties (`id`, `forward_expression`).

## How it works

1. Declare `Relationship` entries on the target model — these specify `from_columns` (FK field) and `to_columns` (referenced field)
2. The `forward_expression` in the mapping produces the FK value
3. Tooling matches the value against the `to_columns` field in the referenced dataset

## Same-source FK (direct match)

Order lines reference their parent order. Both come from the same source:

```yaml
# Relationship on the target model
- name: order_line_to_order
  from: order_line
  to: order
  from_columns: [order_ref]
  to_columns: [order_id]

# Mapping — parent_fields pulls the parent order_id into scope
- target_field: order_ref
  forward_expression:
    dialects:
      - dialect: ANSI_SQL
        expression: parent_order_id
```

Direct match — the FK value is in the same value space as the target PK.

## Cross-source FK (source identity tracing)

When the FK value comes from one source and the referenced entity from another, tooling resolves the FK by tracing through the source identity of the referenced entity's mapping.

```
CRM contact.company_id = 100 (CRM-internal id)
  → forward_expression maps to target person.company_ref = 100
  → Relationship: person.company_ref → company.company_id
  → tooling traces: CRM company mapping has id: db_id
    → source row with db_id=100 was merged into target company entity X
    → resolve: company_ref = X's company_id
```

## Vocabulary normalization via entity mapping

When different sources use different representations for the same value (e.g., Norwegian vs English country names), each source's lookup table is mapped as a proper entity into a shared target dataset — just like companies from different sources merge into one `company` entity.

**Target model** declares a `country` dataset (iso_code only) and a `Relationship`:

```yaml
- name: country
  primary_key: [iso_code]
  fields: [iso_code]

relationships:
  - name: address_to_country
    from: address
    to: country
    from_columns: [country]
    to_columns: [iso_code]
```

**Both sources map their country_lookup into the target country entity:**

```yaml
# ERP mapping (id: name — Norwegian name is the source identity)
- name: country_lookup_to_country
  id: name
  source: { dataset: country_lookup }    # Norwegian names
  target: { dataset: country }
  field_mappings:
    - target_field: iso_code
      forward_expression: iso_code

# CRM mapping (id: name — English name is the source identity)
- name: country_lookup_to_country
  id: name
  source: { dataset: country_lookup }    # English names
  target: { dataset: country }
  field_mappings:
    - target_field: iso_code
      forward_expression: iso_code
```

**Resolution links the two sources by iso_code:**

```yaml
- name: country_resolution
  target: { dataset: country }
  fields:
    iso_code: { strategy: { type: COLLECT, link: true } }
```

**Address mappings just pass through the raw value:**

```yaml
# ERP address mapping
- target_field: country
  forward_expression: address_country     # "Norge"

# CRM address mapping
- target_field: country
  forward_expression: billing_country     # "Norway"
```

**Tooling resolves the FK via source identity tracing:**

1. ERP address produces `country = "Norge"`
2. Tooling sees `Relationship: address.country → country.iso_code`
3. Looks for country mappings from same source (ERP): `country_lookup_to_country` has `id: name`
4. Matches `"Norge"` against source identity → finds the country entity with `iso_code = "NO"`
5. Resolves `address.country = "NO"`

This is the same pattern as person→company: the FK field stores the raw source value during mapping, and tooling traces through the source identity of the referenced entity's mapping to resolve to the target PK.

## OpenAPI source with nested array FKs

When the source is an OpenAPI schema with nested arrays, FK values often span levels of nesting. The webshop API has an `Order` object with a `lines[]` array. Each line item becomes an `order_line` row that references both the parent `order` and a `product`.

**Target model relationships:**

```yaml
relationships:
  - name: order_line_to_order
    from: order_line
    to: order
    from_columns: [order_ref]
    to_columns: [order_id]

  - name: order_line_to_product
    from: order_line
    to: product
    from_columns: [product_id]
    to_columns: [product_id]
```

**FK to parent (via `parent_fields`):**

The `order_id` lives on the parent `Order` object, not on the line item. Use `parent_fields` to pull it into scope:

```yaml
- name: api_orders_to_order_line
  source:
    schema_file: ./webshop-openapi.yaml
    schema_path: "#/components/schemas/Order"
    schema_format: openapi
  source_path: lines
  parent_fields:
    parent_order_id:
      path: ""
      field: order_id
  field_mappings:
    - target_field: order_ref
      forward_expression:
        dialects:
          - dialect: ANSI_SQL
            expression: parent_order_id    # pulled from parent Order
```

Same source, same value space — direct match.

**FK to sibling entity (cross-entity reference):**

Each line item has a `product_id` that references the product catalog. The product is also extracted from the same API (embedded in product-type line items), and the ERP contributes products too:

```yaml
# Line item mapping
- target_field: product_id
  forward_expression:
    dialects:
      - dialect: ANSI_SQL
        expression: product_id             # value on the line item itself

# Product extracted from same line items (embedded)
- name: api_orders_to_product
  embedded: true
  id: product_id
  source_path: lines
  target: { dataset: product }
  field_mappings:
    - target_field: product_id
      forward_expression: product_id
```

Tooling sees `order_line.product_id → product.product_id`. The product was mapped with `id: product_id`, so the webshop's value directly matches. If the ERP also contributes the same product, resolution merges them, but the FK still resolves because `product_id` is the shared target PK.

## Summary

| FK type | Mechanism | Example |
|---------|-----------|---------|
| Same-source | Direct value match | `order_line.order_ref` = `order.order_id` |
| Parent → child | `parent_fields` pulls parent key into scope | `order_line.order_ref` via `parent_order_id` alias |
| Child → sibling | Direct field reference; same value space | `order_line.product_id` matches `product.product_id` |
| Cross-source | Source identity tracing via `id` field on mapping | CRM company + ERP company merge; FK resolves to merged entity |
| Vocabulary | Entity mapping + resolution + source identity tracing | Norwegian "Norge" → country entity → iso_code "NO" |
