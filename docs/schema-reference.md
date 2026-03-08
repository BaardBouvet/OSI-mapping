# Schema Reference

Complete reference for every primitive in the Integration Mapping Schema. Each section documents one schema type, its properties, and links to examples that demonstrate it.

---

## Document Root

A mapping file is a YAML document with four top-level keys.

| Property | Type | Required | Description |
|---|---|---|---|
| `version` | string | **yes** | Always `"1.0"` |
| `description` | string | no | Human-readable summary of what this mapping does |
| `targets` | object | * | Target entity definitions (keys are entity names) |
| `mappings` | array | * | Source-to-target field mappings |
| `tests` | array | no | Inline test cases |

\* At least one of `targets` or `mappings` must be present.

```yaml
version: "1.0"
description: Two CRM systems syncing contacts.

targets:
  contact:
    fields:
      email: identity
      name: coalesce

mappings:
  - name: crm_a
    source: { dataset: crm_a }
    target: contact
    fields:
      - source: email
        target: email
      - source: name
        target: name
        priority: 1

tests:
  - description: "Basic sync"
    input:
      crm_a:
        - { id: "1", email: "a@x.com", name: "Alice" }
    expected:
      crm_a:
        updates:
          - { id: "1", email: "a@x.com", name: "Alice" }
```

**Examples:** [hello-world](../examples/hello-world/), [minimal](../examples/minimal/)

---

## Target

Defines a target entity — the shape of the unified/golden record and how conflicts are resolved.

| Property | Type | Required | Description |
|---|---|---|---|
| `description` | string | no | Human-readable description |
| `fields` | object | **yes** | Field names → resolution rules ([TargetField](#targetfield)) |

Entity names (object keys under `targets`) must match `^[a-z][a-z0-9_]*$`.

```yaml
targets:
  company:
    description: Unified company record
    fields:
      domain: identity
      name: coalesce
      updated_at: last_modified
```

**Examples:** Every example defines at least one target. See [multiple-target-mappings](../examples/multiple-target-mappings/) for multiple targets in one file.

---

## TargetField

Resolution rule for a single target field. Can be a string shorthand or a full [TargetFieldDef](#targetfielddef) object.

### String shorthand

```yaml
fields:
  email: identity
  name: coalesce
  updated_at: last_modified
  tags: collect
```

Allowed values: `identity`, `coalesce`, `last_modified`, `collect`.

### Object form

Use the object form when you need additional configuration (expression strategy, references, defaults, groups).

```yaml
fields:
  score:
    strategy: expression
    expression: "max(score)"
```

---

## TargetFieldDef

Full target field definition with strategy and optional configuration.

| Property | Type | Required | Description |
|---|---|---|---|
| `strategy` | string | **yes** | Resolution strategy (see below) |
| `expression` | string | for `expression` | SQL aggregation expression |
| `references` | string | no | Foreign key → another target entity name |
| `default` | string / number / boolean | no | Static fallback value |
| `default_expression` | string | no | Computed fallback (SQL) |
| `group` | string | no | Atomic resolution group name |
| `link_group` | string | no | Composite identity group name |
| `description` | string | no | Human-readable description |

### Resolution Strategies

#### `identity`

Marks a field as a match key for record linking. Records from different sources with the same identity value(s) are merged into one entity via transitive closure. Every target needs at least one identity field.

```yaml
email: identity
```

**Examples:** [hello-world](../examples/hello-world/), [minimal](../examples/minimal/), [composite-keys](../examples/composite-keys/)

#### `coalesce`

Picks the best non-null value based on priority. Requires `priority` on the corresponding field mappings (lower number wins).

```yaml
name: coalesce
```

**Examples:** [hello-world](../examples/hello-world/), [merge-threeway](../examples/merge-threeway/)

#### `last_modified`

Most recently changed value wins. Requires a `last_modified` timestamp on the mapping or field mapping.

```yaml
name: last_modified
```

**Examples:** [minimal](../examples/minimal/), [embedded-simple](../examples/embedded-simple/), [value-groups](../examples/value-groups/)

#### `expression`

Custom SQL aggregation over contributed values. Only available in the object form.

```yaml
score:
  strategy: expression
  expression: "max(score)"
```

**Examples:** [custom-resolution](../examples/custom-resolution/), [types](../examples/types/), [merge-partials](../examples/merge-partials/)

#### `collect`

Gathers all contributed values without conflict resolution. No additional configuration needed.

```yaml
tags: collect
```

### `references`

Declares a foreign key to another target entity. This is one of the most valuable features of the schema — it enables automatic FK resolution during entity linking.

**The problem it solves:** When two systems share related entities (e.g., companies and contacts), each system uses its own ID namespace. CRM contact `CC1` might reference company `2000`, while ERP contact `C1` references the same real company as `CUST-001`. When entity linking determines that CRM company `2000` and ERP company `CUST-001` are the same entity (via domain identity), the `references` declaration lets the engine translate foreign keys back to each source's local namespace during reverse mapping.

Without this, you'd need to manually build and maintain cross-system ID translation tables — one of the hardest parts of integration.

```yaml
# person.primary_contact points to a company entity
primary_contact:
  strategy: coalesce
  references: company
```

**Reference preservation:** When duplicate entities merge (e.g., two company records with the same domain), referencing records preserve their original FK values on reverse. CRM contact pointing to company `100` keeps `company_id: 100` even after company `100` and `200` merge — because `100` is still a valid local ID in that source.

```yaml
# Two companies merge → contacts keep original company_id
targets:
  company:
    fields:
      domain: identity
      name: last_modified
  contact:
    fields:
      id: identity
      company:
        strategy: identity
        references: company    # FK to company entity
```

**Examples:** [references](../examples/references/) (cross-system FK resolution), [reference-preservation](../examples/reference-preservation/) (FK preservation after merge), [embedded-objects](../examples/embedded-objects/), [nested-arrays](../examples/nested-arrays/), [vocabulary-custom](../examples/vocabulary-custom/), [vocabulary-standard](../examples/vocabulary-standard/)

### `default` / `default_expression`

Fallback values when no source provides data:

```yaml
is_active:
  strategy: coalesce
  default: true

full_name:
  strategy: last_modified
  group: name
  default_expression: "first_name || ' ' || last_name"
```

**Examples:** [value-defaults](../examples/value-defaults/) (default), [value-derived](../examples/value-derived/) (default_expression)

### `group`

Atomic resolution group. All fields sharing the same group resolve from the same winning source — the source with the most recent timestamp across any field in the group. Prevents mixing address parts from different sources.

```yaml
street:
  strategy: last_modified
  group: addr
city:
  strategy: last_modified
  group: addr
zip:
  strategy: last_modified
  group: addr
```

**Examples:** [value-groups](../examples/value-groups/), [merge-groups](../examples/merge-groups/), [value-derived](../examples/value-derived/)

### `link_group`

Composite identity group. Records link only when ALL fields in the same link_group match as a tuple. Without link_group, each identity field matches independently.

```yaml
first_name:
  strategy: identity
  link_group: name_dob
last_name:
  strategy: identity
  link_group: name_dob
dob:
  strategy: identity
  link_group: name_dob
```

Multiple link_groups on the same target act as OR — a match on *any* group links the records.

**Examples:** [composite-keys](../examples/composite-keys/), [merge-groups](../examples/merge-groups/), [relationship-embedded](../examples/relationship-embedded/)

---

## Mapping

Maps fields from one source dataset to one target entity.

| Property | Type | Required | Description |
|---|---|---|---|
| `name` | string | **yes** | Unique identifier (`^[a-z][a-z0-9_]*$`) |
| `description` | string | no | Human-readable description |
| `source` | [SourceRef](#sourceref) | **yes** | Source dataset reference |
| `target` | string / [DatasetRef](#datasetref) | **yes** | Target entity name or external dataset |
| `fields` | array of [FieldMapping](#fieldmapping) | **yes** | Field-level mappings |
| `embedded` | boolean | no | Extract sub-entity from same source row (default: false) |
| `priority` | integer | no | Mapping-level coalesce priority (lower wins) |
| `last_modified` | [TimestampRef](#timestampref) | no | Mapping-level timestamp for last_modified resolution |
| `filter` | string | no | Forward filter: SQL WHERE condition |
| `reverse_filter` | string | no | Reverse filter: SQL WHERE condition |
| `include_base` | boolean | no | Include original values in reverse output (default: false) |

```yaml
mappings:
  - name: crm
    source: { dataset: crm }
    target: contact
    priority: 1
    last_modified: updated_at
    fields:
      - source: email
        target: email
      - source: name
        target: name
```

### `embedded`

Marks a mapping as extracting a sub-entity from the same source row as a parent mapping. The embedded entity shares the parent's source identity.

```yaml
  - name: order_header
    source: { dataset: orders }
    target: order
    fields: [...]

  - name: order_address
    source: { dataset: orders }
    target: shipping_address
    embedded: true
    fields:
      - source: ship_street
        target: street
      - source: ship_city
        target: city
```

**Examples:** [embedded-simple](../examples/embedded-simple/), [embedded-objects](../examples/embedded-objects/), [embedded-multiple](../examples/embedded-multiple/), [route-embedded](../examples/route-embedded/)

### `filter` / `reverse_filter`

SQL WHERE conditions that control which rows flow through the mapping.

- `filter` — forward: only source rows matching this condition map to the target
- `reverse_filter` — reverse: only resolved target rows matching this condition map back to this source

```yaml
  - name: active_contacts
    source: { dataset: crm }
    target: contact
    filter: "status = 'active'"
    reverse_filter: "type LIKE '%customer%'"
    fields: [...]
```

**Examples:** [route](../examples/route/), [route-combined](../examples/route-combined/), [route-multiple](../examples/route-multiple/), [types](../examples/types/), [inserts-and-deletes](../examples/inserts-and-deletes/)

### `include_base`

When true, reverse output includes `_base_` columns with original source values alongside resolved values. Enables optimistic locking and concurrent modification detection.

```yaml
  - name: crm
    source: { dataset: crm }
    target: contact
    include_base: true
    fields: [...]
```

**Examples:** [concurrent-detection](../examples/concurrent-detection/)

---

## FieldMapping

Maps a single source field to a single target field.

| Property | Type | Required | Description |
|---|---|---|---|
| `source` | string | * | Source field name |
| `target` | string | * | Target field name |
| `expression` | string | no | Forward transform (SQL) |
| `reverse_expression` | string | no | Reverse transform (SQL) |
| `direction` | string | no | `bidirectional` (default), `forward_only`, `reverse_only` |
| `priority` | integer | no | Per-field coalesce priority (overrides mapping-level) |
| `last_modified` | [TimestampRef](#timestampref) | no | Per-field timestamp (overrides mapping-level) |
| `reverse_required` | boolean | no | Exclude row from reverse if resolved value is null |
| `description` | string | no | Human-readable description |

\* At least one of `source` or `target` must be present.

### Basic field copy

```yaml
- source: email
  target: email
```

### With priority

```yaml
- source: name
  target: name
  priority: 1
```

**Examples:** [hello-world](../examples/hello-world/), [merge-threeway](../examples/merge-threeway/)

### `expression` / `reverse_expression`

SQL expressions for forward and reverse transforms. When `expression` is omitted, the source value is copied directly.

```yaml
# Split full name → first/last
- source: full_name
  target: first_name
  expression: "split_part(full_name, ' ', 1)"
  reverse_expression: "first_name || ' ' || last_name"

# Constant injection (no source)
- target: type
  expression: "'customer'"
  direction: forward_only
```

**Examples:** [value-conversions](../examples/value-conversions/), [value-derived](../examples/value-derived/), [custom-resolution](../examples/custom-resolution/), [route](../examples/route/)

### `direction`

Controls whether a field mapping flows forward, reverse, or both.

| Value | Meaning |
|---|---|
| `bidirectional` | Both directions (default when `source` is present) |
| `forward_only` | Source → target only (default when `source` is omitted) |
| `reverse_only` | Target → source only |

```yaml
# Forward-only constant
- target: type
  expression: "'employee'"
  direction: forward_only

# Reverse-only reconstruction
- source: entity_type
  direction: reverse_only
  reverse_expression: "'contact'"
```

**Examples:** [route](../examples/route/), [types](../examples/types/), [embedded-vs-many-to-many](../examples/embedded-vs-many-to-many/)

### `reverse_required`

When true, the entire row is excluded from reverse output if this field's resolved value is null. This enables insert/delete propagation — rows that don't have a required field are treated as deletes.

```yaml
- source: active
  target: is_active
  reverse_required: true
```

**Examples:** [inserts-and-deletes](../examples/inserts-and-deletes/)

### Per-field `last_modified`

Overrides the mapping-level timestamp for a specific field. Useful when different fields in the same source have independent last-modified timestamps.

```yaml
- source: name
  target: name
  last_modified: name_updated_at

- source: phone
  target: phone
  last_modified: phone_updated_at
```

**Examples:** [value-groups](../examples/value-groups/)

---

## SourceRef

Reference to a source dataset, with optional nested array extraction.

| Property | Type | Required | Description |
|---|---|---|---|
| `dataset` | string | **yes** | Source dataset/table name |
| `path` | string | no | Dot-delimited path to a nested array |
| `parent_fields` | object | no | Import ancestor fields into scope (keys are aliases) |

### Simple dataset

```yaml
source: { dataset: crm }
```

### Nested array with parent fields

When `path` is set, the mapping iterates over each item in the nested array. Use `parent_fields` to bring ancestor-level fields into scope.

```yaml
source:
  dataset: orders
  path: lines
  parent_fields:
    order_id: order_id
```

For deep nesting, use dot notation in `path` and object form in `parent_fields`:

```yaml
source:
  dataset: orders
  path: lines.sub_items
  parent_fields:
    order_id: order_id               # from root
    line_id:                          # from intermediate level
      path: lines
      field: line_id
```

**Examples:** [nested-arrays](../examples/nested-arrays/), [nested-arrays-deep](../examples/nested-arrays-deep/), [nested-arrays-multiple](../examples/nested-arrays-multiple/)

---

## DatasetRef

Reference to an external dataset, used when the mapping target is not defined in the same file's `targets` section.

| Property | Type | Required | Description |
|---|---|---|---|
| `dataset` | string | **yes** | Dataset/table name |

```yaml
target: { dataset: external_contacts }
```

---

## Expression

An ANSI SQL expression string. Used in multiple contexts:

| Context | Example |
|---|---|
| Target-level aggregation | `"max(score)"` |
| Target default_expression | `"first_name \|\| ' ' \|\| last_name"` |
| Field forward transform | `"upper(name)"` |
| Field reverse transform | `"lower(name)"` |
| Mapping filter | `"status = 'active'"` |
| Mapping reverse_filter | `"type LIKE '%customer%'"` |

Expressions reference field names as SQL column identifiers — no placeholder syntax.

**Examples:** [custom-resolution](../examples/custom-resolution/), [value-conversions](../examples/value-conversions/), [route](../examples/route/), [value-derived](../examples/value-derived/)

---

## TimestampRef

Specifies the timestamp source for `last_modified` resolution. Can appear on mappings or individual field mappings.

### String shorthand

```yaml
last_modified: updated_at
```

References a source field containing the timestamp.

### Object form

```yaml
last_modified:
  field: updated_at

# Or expression-based
last_modified:
  expression: "coalesce(updated_at, created_at)"
```

| Property | Type | Required | Description |
|---|---|---|---|
| `field` | string | * | Source field with the timestamp |
| `expression` | string | * | SQL expression producing a timestamp |

\* At least one of `field` or `expression` must be present.

**Examples:** [minimal](../examples/minimal/), [value-groups](../examples/value-groups/), [merge-internal](../examples/merge-internal/)

---

## ParentFieldRef

References an ancestor-level field when mapping nested arrays. Used as values in `source.parent_fields`.

### String shorthand

References a field from the root source object (parent of the nested path):

```yaml
parent_fields:
  order_id: order_id       # alias: root_field_name
```

### Object form

For deep nesting, references a field from an intermediate ancestor:

```yaml
parent_fields:
  line_id:
    path: lines            # intermediate scope
    field: line_id         # field within that scope
```

| Property | Type | Required | Description |
|---|---|---|---|
| `path` | string | no | Dot-delimited path to ancestor scope |
| `field` | string | **yes** | Field name within the ancestor scope |

**Examples:** [nested-arrays-deep](../examples/nested-arrays-deep/), [nested-arrays-multiple](../examples/nested-arrays-multiple/)

---

## TestCase

Inline test case verifying the full pipeline: forward transform → resolution → reverse transform.

| Property | Type | Required | Description |
|---|---|---|---|
| `description` | string | no | What this test verifies |
| `input` | object | **yes** | Input data keyed by dataset name → array of row objects |
| `expected` | object | **yes** | Expected output keyed by dataset name → result object |

### Expected result format

Expected values are **always objects** with explicit `updates`, `inserts`, `deletes` arrays. Never bare arrays.

| Key | Description |
|---|---|
| `updates` | Rows that exist in input and survive resolution (potentially modified) |
| `inserts` | New rows to create in this source (from other sources) |
| `deletes` | Rows to remove (failed reverse_required or filter) |

Omit a key when that category is empty.

```yaml
tests:
  - description: "CRM name wins, propagates to ERP"
    input:
      crm:
        - { id: "1", email: "a@x.com", name: "Alice" }
      erp:
        - { id: "10", email: "a@x.com", name: "A. Smith" }
    expected:
      crm:
        updates:
          - { id: "1", email: "a@x.com", name: "Alice" }
      erp:
        updates:
          - { id: "10", email: "a@x.com", name: "Alice" }
```

**Examples:** Every example includes tests. See [inserts-and-deletes](../examples/inserts-and-deletes/) for all three categories, [hello-world](../examples/hello-world/) for simplest case.
