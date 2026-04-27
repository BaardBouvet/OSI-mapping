# Schema Reference

Complete reference for every property in the Integration Mapping Schema v2.
Examples and links point to runnable mappings in [`examples/`](../../examples/).

---

## Document root

```yaml
version: "2.0"           # required — always the string "2.0"
description: string      # optional — human-readable summary
sources:  { ... }        # required — one entry per source dataset
targets:  { ... }        # required — one entry per target entity
mappings: [ ... ]        # required — one entry per source-to-target mapping
tests:    [ ... ]        # optional — inline test cases
```

| Property | Type | Required | Description |
|---|---|---|---|
| `version` | string | **yes** | Always `"2.0"` |
| `description` | string | no | Human-readable summary |
| `sources` | object | **yes** | Source dataset metadata (keys are dataset names) |
| `targets` | object | **yes** | Target entity definitions (keys are entity names) |
| `mappings` | array | **yes** | Source-to-target field mappings |
| `tests` | array | no | Inline test cases |

All four of `sources`, `targets`, `mappings`, and `tests` may appear in any order. Keep them in the order shown — it reads naturally as a declaration of structure → rules → verification.

---

## `sources`

Declares the physical source datasets and their primary keys.

```yaml
sources:
  crm:
    primary_key: id
  erp:
    primary_key: employee_id
```

Each key under `sources` is a **source name** referenced by mappings.

| Property | Type | Required | Description |
|---|---|---|---|
| `primary_key` | string | **yes** | Name of the primary-key column in this source dataset |

The primary key is used to generate row-level IRIs for identity closure and to anchor reverse projections.

---

## `targets`

Defines what unified entities look like and how conflicts between sources resolve.

```yaml
targets:
  contact:
    identity:
      - email              # single-field identity
    fields:
      email: { strategy: coalesce }
      name:  { strategy: coalesce }
      title: { strategy: last_modified }
```

Each key under `targets` is a **target name** referenced by mappings.

| Property | Type | Required | Description |
|---|---|---|---|
| `identity` | array | **yes** | Identity groups — see [Identity](#identity) |
| `fields` | object | **yes** | Target field definitions (keys are field names) — see [Field](#field) |

### Identity

`identity` is a list of identity groups. Each group is either:

- A **single field name** (`string`) — the simplest case; two rows from different sources that have the same value for this field refer to the same entity.
- A **tuple** (`[field, field, ...]`) — an AND-tuple; rows match only when *all* fields in the tuple match simultaneously.

```yaml
identity:
  - email                        # single-field: match on email alone

identity:
  - [first_name, last_name]      # tuple: match when both fields agree
```

The list may contain more than one group, forming an OR over groups (a row matches if *any* group matches). In practice, most targets use a single group.

→ See [`examples/composite-identity/`](../../examples/composite-identity/) for AND-tuple identity.

### Field

Each entry under `targets.<name>.fields` is a field definition.

```yaml
fields:
  email:  { strategy: coalesce }
  name:   { strategy: coalesce }
  title:  { strategy: last_modified }
```

| Property | Type | Required | Description |
|---|---|---|---|
| `strategy` | string | **yes** | Resolution strategy — `coalesce` or `last_modified` |

#### Resolution strategies

| Strategy | Behaviour | When to use |
|---|---|---|
| `coalesce` | Picks the highest-priority non-null value. Priority is set on the field mapping (lower number wins); ties broken by declaration order within the mapping file. | When one source is the authority on a field. |
| `last_modified` | Picks the value from the source row with the most recent `last_modified` timestamp. Mappings without a configured timestamp column contribute as NULL-timestamped (and lose to any timestamped candidate). Ties on timestamp are broken by declaration order. | When the most recently changed value is correct regardless of source. |

---

## `mappings`

Each mapping connects one source dataset to one target entity and declares which source columns feed which target fields.

```yaml
mappings:
  - name: crm
    source: crm
    target: contact
    last_modified: updated_at    # optional
    fields:
      - { source: email,     target: email }
      - { source: full_name, target: name, priority: 1 }
```

| Property | Type | Required | Description |
|---|---|---|---|
| `name` | string | **yes** | Unique identifier for this mapping (lowercase, underscores) |
| `source` | string | **yes** | Source dataset name (must match a key in `sources`) |
| `target` | string | **yes** | Target entity name (must match a key in `targets`) |
| `last_modified` | string | no | Source column carrying the row's last-modified timestamp. Required if the target has any `last_modified` field and this mapping should contribute as a timestamped candidate. Mappings without it lose to any timestamped candidate. |
| `parent` | string | no | Name of a parent mapping — makes this a **child mapping** that expands an array column from the parent source. See [Nested arrays](#nested-arrays). |
| `array` | string | no | Source column (or dotted path) holding the JSON array to expand. Required when `parent:` is set. |
| `parent_fields` | object | no | Aliases that lift parent-row columns into the child's scope. Keys are alias names used in child `fields`; values are the parent source column names. |
| `fields` | array | **yes** | Field mappings — see [Field mapping](#field-mapping) |

### Field mapping

Each entry in `mapping.fields` connects one source column to one target field.

```yaml
fields:
  - source: email          # source column name
    target: email          # target field name
  - source: full_name
    target: name
    priority: 1            # optional — lower wins in coalesce
  - source: company_id
    target: company        # optional — slice 4 cross-entity FK
    references: company
```

| Property | Type | Required | Description |
|---|---|---|---|
| `source` | string | **yes** | Source column name (must be a column in the source dataset, or an alias introduced by `parent_fields`) |
| `target` | string | **yes** | Target field name (must match a key in `targets.<target>.fields`) |
| `priority` | integer | no | Priority for `coalesce` resolution. Lower number wins. Omit to use declaration order as priority. |
| `references` | string | no | **(Slice 4, not yet implemented)** Name of another target whose canonical IRI should be used to resolve this field on reverse — declares a foreign-key relationship. |

### Nested arrays

A child mapping expands a JSON array from a parent source row into individual logical rows. This models one-to-many relationships (e.g. an order with multiple line items) without requiring a separate source table.

```yaml
mappings:
  - name: shop_orders          # parent mapping
    source: shop
    target: purchase_order
    fields:
      - { source: order_id, target: order_id }
      - { source: buyer,    target: buyer }

  - name: shop_lines           # child mapping
    source: shop
    parent: shop_orders        # declares parent
    target: order_line
    array: lines               # column holding the JSON array
    parent_fields:
      order_ref: order_id      # lifts parent's order_id as child's order_ref
    fields:
      - { source: order_ref,   target: order_ref }
      - { source: line_number, target: line_number }
      - { source: sku,         target: sku }
      - { source: quantity,    target: quantity }
```

The `parent_fields` map is evaluated before field mappings. Each key becomes a virtual source column available in the child's `fields`.

→ See [`examples/nested-arrays-shallow/`](../../examples/nested-arrays-shallow/).

---

## `tests`

Inline test cases validate that the mapping behaves correctly. Each test specifies input rows and the expected delta output.

```yaml
tests:
  - description: "CRM name wins (priority 1)"
    input:
      crm:
        - { id: "1", email: "alice@example.com", name: "Alice" }
      erp:
        - { id: "100", contact_email: "alice@example.com", contact_name: "A. Smith" }
    expected:
      erp:
        updates:
          - { id: "100", contact_email: "alice@example.com", contact_name: "Alice" }
```

| Property | Type | Required | Description |
|---|---|---|---|
| `description` | string | **yes** | Human-readable description of what this test asserts |
| `input` | object | **yes** | Current state of each source. Keys are source names; values are arrays of rows. |
| `expected` | object | **yes** | Expected delta output. Keys are mapping names (or source names for single-mapping files). |

### Expected delta structure

```yaml
expected:
  crm:
    updates: [ ... ]   # rows that should be updated in the source
    inserts: [ ... ]   # rows that should be inserted into the source
    deletes: [ ... ]   # rows that should be deleted from the source
```

All three keys are optional and default to `[]`. An entry with `{}` (empty object) asserts that no deltas are expected for that mapping.

**Updates** are existing rows where the canonical resolved value differs from the current source value. The row carries the source's primary key and all mapped field values, set to their canonical resolved values.

**Inserts** are entities present in the canonical model that have no row in this source. The row carries the source's primary key set to `null`, all mapped field values, and `_canonical_id` with the canonical entity identifier.

**Deletes** are source rows that no longer match any canonical entity. The row carries only the source's primary key.

### `_canonical_id` format

On insert rows, `_canonical_id` identifies the canonical entity that this row should be created for. Format: `<source_name>:<pk_value>` of the first source that contributed to this entity.

```yaml
inserts:
  - { _canonical_id: "erp:200", id: null, email: "carol@example.com", name: "Carol" }
```

---

## Naming conventions

- **Source names**: lowercase, underscores (e.g. `crm`, `erp`, `phone_book`). Must match the physical table or dataset name in the source system.
- **Mapping names**: lowercase, underscores. Usually match the source name unless multiple mappings read the same source.
- **Target names**: lowercase, underscores (e.g. `contact`, `purchase_order`).
- **Field names** (source and target): lowercase, underscores.

---

## Minimal complete example

```yaml
version: "2.0"
description: Two systems sharing contacts by email.

sources:
  crm:
    primary_key: id
  erp:
    primary_key: id

targets:
  contact:
    identity:
      - email
    fields:
      email: { strategy: coalesce }
      name:  { strategy: coalesce }

mappings:
  - name: crm
    source: crm
    target: contact
    fields:
      - { source: email, target: email }
      - { source: name,  target: name, priority: 1 }

  - name: erp
    source: erp
    target: contact
    fields:
      - { source: contact_email, target: email }
      - { source: contact_name,  target: name, priority: 2 }

tests:
  - description: "CRM name wins"
    input:
      crm: [ { id: "1", email: "a@x.com", name: "Alice" } ]
      erp: [ { id: "100", contact_email: "a@x.com", contact_name: "A. Smith" } ]
    expected:
      erp:
        updates:
          - { id: "100", contact_email: "a@x.com", contact_name: "Alice" }
```

→ Full walkthrough: [Annotated example](annotated-example.md)  
→ Runnable version: [`examples/hello-world/`](../../examples/hello-world/)
