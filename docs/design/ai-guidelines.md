# AI Agent Guidelines — Integration Mapping Files

This document helps AI agents (code generators, copilots, validation tools) produce correct mapping files that conform to the Integration Mapping Schema.

## One File Per Integration — Not One File Per Entity

**All target entities for an integration belong in a single YAML file.** Do not split entities into separate files (e.g., `customer.yaml`, `company.yaml`, `country.yaml`). The schema is designed so that one file describes the complete integration picture.

Why this matters:
- **Cross-entity references** (`references: company` on a contact field) only resolve when both entities are in the same file
- **Test cases** can exercise interactions between entities (e.g., FK resolution across ID namespaces)
- **The resolution engine** needs the full entity graph to link records and translate foreign keys

A typical file defines multiple targets, with multiple mappings per target (one per source system):

```yaml
targets:
  company:
    fields:
      domain: identity
      name: coalesce
  contact:
    fields:
      email: identity
      company_id:
        strategy: coalesce
        references: company

mappings:
  - name: crm_companies
    source: crm
    target: company
    fields: [...]
  - name: erp_companies
    source: erp
    target: company
    fields: [...]
  - name: crm_contacts
    source: crm
    target: contact
    fields: [...]
  - name: erp_contacts
    source: erp
    target: contact
    fields: [...]
```

See [`examples/references/mapping.yaml`](../examples/references/mapping.yaml) and [`examples/relationship-mapping/mapping.yaml`](../examples/relationship-mapping/mapping.yaml) for complete multi-entity examples.

## File Structure

Every mapping file is a YAML document with five top-level keys:

```yaml
version: "1.0"           # Required — always "1.0"
description: "..."        # Optional — human-readable summary

sources:                  # Source metadata: primary keys, table names
  <dataset_name>:
    primary_key: <column>  # or [col1, col2] for composite

targets:                  # Target entity definitions with resolution rules
  <entity_name>:
    fields:
      <field>: <strategy>

mappings:                 # Source-to-target field mappings
  - name: <unique_name>
    source: <name>
    target: <entity_name>
    fields:
      - source: <src_field>
        target: <tgt_field>

tests:                    # Inline test cases
  - description: "..."
    input: { ... }
    expected: { ... }
```

At least one of `targets` or `mappings` must be present. Both are present in most files. The `sources:` section declares primary keys used throughout the pipeline.

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
| `bool_or` | True if any source is true (boolean flags) | — |

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
    source: crm
    target: contact
    fields:
      - source: email
        target: email
      - source: name
        target: name
        priority: 1    # CRM wins

  - name: erp
    source: erp
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
    source: orders
    target: order
    fields: [...]

  - name: order_address
    parent: order_header
    target: shipping_address
    fields: [...]
```

### Nested arrays

```yaml
mappings:
  - name: shop_orders
    source: orders
    target: order
    fields: [...]

  - name: order_lines
    parent: shop_orders
    array: lines
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
    source: crm
    target: customer
    filter: "status = 'active'"              # Forward: only active rows
    reverse_filter: "segment = 'retail'"     # Reverse: only retail back to CRM
    fields: [...]
```

### External identity links

When records are linked by an external system (MDM, record linkage tool):

```yaml
mappings:
  - name: match_links
    source: match_results
    target: contact
    links:
      - field: crm_id
        references: crm
      - field: erp_id
        references: erp
```

A mapping with `links` and no `fields` is a linkage-only mapping.

### ETL feedback (cluster_members)

For insert tracking — prevents duplicate inserts by feeding back generated IDs:

```yaml
mappings:
  - name: erp
    source: erp
    target: contact
    cluster_members: true    # creates _cluster_members_erp table
    fields: [...]
```

### Written state (written_state / derive_noop)

For target-centric noop detection — prevents redundant writes when source changes don't affect the resolved value:

```yaml
mappings:
  - name: erp
    source: erp
    target: contact
    written_state: true      # creates _written_erp table
    derive_noop: true        # compare resolved vs last-written
    fields: [...]
```

The ETL writes `(_cluster_id, _written JSONB)` after each sync. The engine reads this to detect when the target already has the correct value.

### Precision loss (normalize)

Handles lossy noop comparison when a target system has lower fidelity:

```yaml
fields:
  - source: price
    target: price
    normalize: "trunc(%s::numeric, 0)::integer::text"
```

Applied to both sides of the delta comparison. Also prevents low-precision echoes from winning `last_modified` resolution.

### Array element ordering (order / order_prev / order_next)

For nested array mappings, `order: true` generates a sortable position key from the source array index:

```yaml
  - name: recipe_steps
    parent: recipes
    array: steps
    target: recipe_step
    priority: 1
    fields:
      - source: instruction
        target: instruction
      - target: step_order
        order: true             # generates lpad ordinal from array position
      - source: duration
        target: duration
```

`order_prev` / `order_next` emit adjacent-element identities for graph-based ordering:

```yaml
      - target: prev_step
        order_prev: true        # LAG over identity field
      - target: next_step
        order_next: true        # LEAD over identity field
```

The target field should use `coalesce` strategy so the highest-priority source's ordering wins. See [`examples/crdt-ordering/`](../../examples/crdt-ordering/).

### Nested array sort (sort)

For child mappings, `sort` provides static field-based ordering of the reconstructed array — simpler than CRDT ordering when you just need a deterministic sort:

```yaml
  - name: person_orders
    parent: person_mapping
    array: orders
    target: order
    sort:
      - field: amount
        direction: desc
    fields:
      - source: order_id
        target: order_id
      - source: amount
        target: amount
```

Mutually exclusive with `order: true`. See [`examples/sesam-annotated/`](../../examples/sesam-annotated/).

### Enriched expressions

When a target field's `expression` references other target names via `FROM`/`JOIN`, it becomes an *enriched expression*. The engine renders it as a `LEFT JOIN LATERAL` subquery in a dedicated view layer, allowing correlated subqueries across resolved targets:

```yaml
targets:
  global_person:
    fields:
      person_id: identity
      name: coalesce
      order_count:
        strategy: expression
        expression: |
          COALESCE((
            SELECT count(*)
            FROM global_order o
            WHERE o.person_ref = global_person.person_id
          ), 0)
        type: numeric
```

Rules:
- Bare target names in `FROM`/`JOIN` are rewritten to resolved view names automatically
- Referenced targets must be declared in the same file
- DML/DDL statements are blocked
- Enriched fields are typically paired with `direction: reverse_only` on consuming mappings (computed values don't write back)

See [`examples/sesam-annotated/`](../../examples/sesam-annotated/).

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

- `input` and `expected` keys must be source names matching `mappings[].source`
- `expected` values are **always objects** with explicit `updates`, `inserts`, `deletes` arrays
- Never use a bare array for expected — always the `{ updates, inserts, deletes }` form
- Omit a key (`updates`, `inserts`, or `deletes`) only when that category is empty
- `updates`: rows that exist in input and survive resolution (potentially with changed values)
- `inserts`: new rows to create (originated from another source). Must include `_cluster_id` — a seed like `"mapping:src_id"` identifying which entity the insert belongs to
- `deletes`: rows to remove (failed `reverse_required` or filter)

### Matching Policy

Expected rows must be an **exact match** of the complete actual delta-view output — every column, every value. Partial assertions (listing only some fields) are not allowed.

- Include **all source columns** from the input (including unmapped columns such as timestamps).
- Include **all reverse-mapped fields** with their resolved values (use `null` when resolution produces no value).
- **`_base` may be omitted** for brevity — the test harness strips it from both sides before comparison.
- **Insert rows** must include `_cluster_id` (seed notation) plus every reverse-mapped field.

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
11. Every source dataset used by a mapping has a `sources:` entry with `primary_key`
12. Insert rows in test expected sections include `_cluster_id`

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
6. **Forgetting `parent:`** — Sub-entities from the same source row need `parent:` referencing their parent mapping
7. **Missing `parent_fields`** — Nested arrays usually need parent-level fields imported
8. **Splitting entities into separate files** — All entities for an integration go in one file. Separate files break cross-entity references and prevent holistic resolution
