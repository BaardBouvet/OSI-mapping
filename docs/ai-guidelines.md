# AI Agent Guidelines — Integration Mapping Files

This document helps AI agents (code generators, copilots, validation tools) produce correct mapping files that conform to the Integration Mapping Schema.

## File Structure

Every mapping file is a YAML document with four top-level keys:

```yaml
version: "1.0"           # Required — always "1.0"
description: "..."        # Optional — human-readable summary

targets:                  # Target entity definitions with resolution rules
  <entity_name>:
    fields:
      <field>: <strategy>

mappings:                 # Source-to-target field mappings
  - name: <unique_name>
    source: { dataset: <name> }
    target: <entity_name>
    fields:
      - source: <src_field>
        target: <tgt_field>

tests:                    # Inline test cases
  - description: "..."
    input: { ... }
    expected: { ... }
```

At least one of `targets` or `mappings` must be present. Both are present in most files.

## Naming Rules

- Entity names (target keys): `^[a-z][a-z0-9_]*$` — lowercase, underscores
- Mapping names: `^[a-z][a-z0-9_]*$` — must be unique across the file
- Field names: no enforced pattern, but lowercase_snake_case is conventional

## Target Fields

Each target field declares a resolution strategy — either as a string shorthand or as an object:

```yaml
# String shorthand
email: identity
name: coalesce

# Object form (required for expression, optional for others)
price:
  strategy: expression
  expression: "max(price)"
```

### Strategy Reference

| Strategy | When to use | Required companion |
|---|---|---|
| `identity` | Field participates in record matching | — |
| `coalesce` | Pick the best non-null value by priority | `priority` on field mappings |
| `last_modified` | Most recent value wins | `last_modified` on the mapping or field |
| `expression` | Compute via SQL | `expression` on the target field |
| `collect` | Gather all values, no resolution | — |

**Every target must have at least one `identity` field.** This is how records from different sources are matched.

## Field Mappings

Each field mapping connects one source field to one target field:

```yaml
fields:
  - source: src_field
    target: tgt_field
```

### Optional Properties

| Property | Type | Purpose |
|---|---|---|
| `priority` | integer | Coalesce priority (lower wins) |
| `expression` | string | SQL forward transform |
| `reverse_expression` | string | SQL reverse transform |
| `direction` | enum | `bidirectional` (default), `forward_only`, `reverse_only` |
| `reverse_required` | boolean | Exclude row from reverse if resolved value is null |
| `last_modified` | string/object | Per-field timestamp override |

### Rules

- At least one of `source` or `target` must be present
- Omit `source` for computed/constant fields (forward_only)
- Omit `target` when reconstructing a source field during reverse (reverse_only)
- Each `target` field should appear at most once per mapping (no duplicate targets)
- `priority` is per-mapping-field, not per-target — different mappings assign different priorities

## Common Patterns

### Two sources, coalesce resolution

```yaml
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
        priority: 1    # CRM wins

  - name: erp
    source: { dataset: erp }
    target: contact
    fields:
      - source: email
        target: email
      - source: name
        target: name
        priority: 2
```

### Composite identity (link_group)

When matching requires multiple fields together (not individually):

```yaml
targets:
  person:
    fields:
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

All fields in the same `link_group` must match as a tuple for records to link.

### Atomic groups

When related fields must resolve from the same source:

```yaml
targets:
  address:
    fields:
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

### Embedded sub-entities

```yaml
mappings:
  - name: order_header
    source: { dataset: orders }
    target: order
    fields: [...]

  - name: order_address
    source: { dataset: orders }
    target: shipping_address
    embedded: true
    fields: [...]
```

### Nested arrays

```yaml
mappings:
  - name: order_lines
    source:
      dataset: orders
      path: lines              # Iterate over lines[]
      parent_fields:
        order_id: order_id     # Import parent field
    target: order_line
    fields:
      - source: order_id
        target: order_id
      - source: sku
        target: sku
```

### Filters / routing

```yaml
mappings:
  - name: active_customers
    source: { dataset: crm }
    target: customer
    filter: "status = 'active'"              # Forward: only active rows
    reverse_filter: "segment = 'retail'"     # Reverse: only retail back to CRM
    fields: [...]
```

## Tests Section

Tests define input data and expected output after the full pipeline (forward → resolution → reverse):

```yaml
tests:
  - description: "What this test verifies"
    input:
      <dataset_name>:
        - { id: "1", field: "value" }
    expected:
      <dataset_name>:
        updates:
          - { id: "1", field: "resolved_value" }
        inserts: []
        deletes: []
```

### Test Rules

- `input` and `expected` keys must be dataset names matching `mappings[].source.dataset`
- `expected` values are **always objects** with explicit `updates`, `inserts`, `deletes` arrays
- Never use a bare array for expected — always the `{ updates, inserts, deletes }` form
- Omit a key (`updates`, `inserts`, or `deletes`) only when that category is empty
- `updates`: rows that exist in input and survive resolution (potentially with changed values)
- `inserts`: new rows to create (originated from another source)
- `deletes`: rows to remove (failed `reverse_required` or filter)

## Validation Checklist

Before submitting a mapping file, verify:

1. `version: "1.0"` is present
2. Every target has at least one `identity` field
3. Every mapping `target` references a defined target entity (or uses a dataset ref)
4. Every field mapping `target` references a declared target field
5. `coalesce` fields have `priority` set on their field mappings
6. `last_modified` fields have a timestamp source (mapping-level or field-level)
7. `expression` target fields have an `expression` property
8. Mapping names are unique within the file
9. No duplicate `target` field names within a single mapping's fields
10. Test dataset names match mapping source datasets

## Expressions

All expressions are ANSI SQL strings. They can reference:
- Source field names (in field-level `expression` / `reverse_expression`)
- Target field names (in target-level `expression` and `default_expression`)
- Standard SQL functions and operators

```yaml
# Field-level forward transform
expression: "upper(name)"

# Field-level reverse transform  
reverse_expression: "lower(name)"

# Target-level aggregation
strategy: expression
expression: "max(price)"

# Default fallback
default_expression: "current_timestamp"
```

## Anti-Patterns to Avoid

1. **Missing identity** — Every target needs at least one identity field for record matching
2. **Priority on identity** — Identity fields don't use priority; it's ignored
3. **Bare array in expected** — Always use `{ updates: [...], inserts: [...], deletes: [...] }`
4. **Duplicate mapping names** — Each mapping must have a unique `name`
5. **Duplicate field targets** — Don't map two source fields to the same target field in one mapping
6. **Forgetting `embedded: true`** — Sub-entities from the same source row need this flag
7. **Missing `parent_fields`** — Nested arrays usually need parent-level fields imported
