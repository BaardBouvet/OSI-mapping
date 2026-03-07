# OSI Resolution Schema Reference

> **Schema file:** [`specs/osi-resolution-schema.json`](../specs/osi-resolution-schema.json)  
> **JSON Schema draft:** 2020-12  
> **Version:** 1.0

The OSI Resolution Schema defines conflict resolution rules for when multiple mapping sources contribute data to the same target dataset. It specifies how to pick the winning value on a per-field basis — by priority, recency, aggregation, or custom logic.

---

## Document Structure

A resolution document is a JSON or YAML object with two required top-level properties:

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `version` | `string` | **Yes** | Must be `"1.0"` |
| `resolutions` | `Resolution[]` | **Yes** | One or more resolution configurations |

```yaml
version: "1.0"
resolutions:
  - name: company_resolution
    target: { ... }
    fields: { ... }
```

No additional properties are allowed at the top level.

---

## Resolution

Each entry in the `resolutions` array defines rules for a single target dataset.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `name` | `string` | **Yes** | Unique identifier. Must match `^[a-z][a-z0-9_]*$`. |
| `description` | `string` | No | Human-readable description. |
| `target` | [ModelRef](mapping-schema.md#modelref) | **Yes** | The target dataset these rules apply to. Uses the same ModelRef format as the mapping schema. |
| `fields` | `object` | **Yes** | Per-field resolution rules. Keys are target field names; values are [FieldResolution](#fieldresolution) objects. |
| `atomic_groups` | `object` | No | Named atomic resolution groups. Keys are group names; values are [AtomicGroup](#atomicgroup) objects. |
| `link_groups` | `object` | No | Named link groups for entity linking. Records are linked only when ALL fields in the group match (tuple equality). Keys are group names; values are [LinkGroup](#linkgroup) objects. |

### Example

```yaml
resolutions:
  - name: company_resolution
    description: Resolution rules for the company dataset

    target:
      semantic_model: acme_model
      dataset: company
      model_file: ./model-acme.yaml

    fields:
      name:
        strategy:
          type: COALESCE
      email:
        strategy:
          type: LAST_MODIFIED
      account:
        strategy:
          type: COALESCE
```

---

## FieldResolution

Defines the resolution rule for a single target field.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `strategy` | [Strategy](#strategy) | **Yes** | The resolution strategy to apply. |

---

## Strategy

A strategy determines how conflicting values from multiple sources are resolved. The `type` property acts as a discriminator.

### COALESCE

Pick the value from the highest-priority source (lowest `priority` number wins).

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `type` | `"COALESCE"` | **Yes** | Strategy discriminator |

Sources declare their priority in the mapping file via the `priority` property on each [FieldMapping](mapping-schema.md#fieldmapping). The source with the lowest priority number wins. If two sources have equal priority, the first non-null value wins.

```yaml
# In the resolution file:
fields:
  name:
    strategy:
      type: COALESCE

# In the mapping files:
# ERP mapping — priority 1 (wins):
- target_field: name
  priority: 1
  expression_forward: { ... }

# CRM mapping — priority 2 (fallback):
- target_field: name
  priority: 2
  expression_forward: { ... }
```

### LAST_MODIFIED

Pick the value from the most recently updated source.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `type` | `"LAST_MODIFIED"` | **Yes** | Strategy discriminator |

Each mapping must provide a timestamp field — either via the field mapping's `timestamp_field` property or the mapping-level `default_timestamp_field`.

```yaml
# Resolution:
fields:
  email:
    strategy:
      type: LAST_MODIFIED

# Mapping (declares the timestamp source):
default_timestamp_field: updated_at
```

### COLLECT

Collect all source values into an array. Optionally enables entity linking through transitive closure.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `type` | `"COLLECT"` | **Yes** | Strategy discriminator |
| `link` | `boolean` | No | When `true`, records sharing this field's value are linked as the same entity. All values are retained for transitive closure. |

```yaml
fields:
  source_ids:
    strategy:
      type: COLLECT
      link: true
```

When `link: true`, the collected values serve double duty: they form the merged array **and** drive entity matching. Two records from different sources that share any value in this field are considered the same entity.

### EXPRESSION

Resolve using a custom SQL expression operating in an aggregation context over all contributed values.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `type` | `"EXPRESSION"` | **Yes** | Strategy discriminator |
| `expression` | [Expression](mapping-schema.md#expression) | **Yes** | SQL aggregation expression. Reference the target field name as the column name. |

```yaml
fields:
  email:
    strategy:
      type: EXPRESSION
      expression:
        dialects:
          - dialect: ANSI_SQL
            expression: "MAX(email)"
```

The expression operates over all contributed values for the field, as if running a `SELECT <expression> FROM contributed_values` aggregation.

---

## Atomic Groups

Atomic groups enforce atomic resolution: all fields in a group resolve from the same winning source. The source with the highest timestamp across **any** field in the group wins **all** grouped fields.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `fields` | `string[]` | **Yes** | Target field names that resolve together. Minimum 2 fields. All must use `LAST_MODIFIED` strategy. |

### Example

```yaml
resolutions:
  - name: contact_resolution
    target:
      semantic_model: canonical
      dataset: contact

    fields:
      first_name:
        strategy:
          type: LAST_MODIFIED
      last_name:
        strategy:
          type: LAST_MODIFIED
      phone:
        strategy:
          type: LAST_MODIFIED

    atomic_groups:
      name_group:
        fields: [first_name, last_name]
```

In this example, `first_name` and `last_name` always come from the same source — whichever source most recently updated either name field wins both. The `phone` field resolves independently.

---

## Link Groups

Link groups define entity linking rules based on tuple equality. Unlike single-field `COLLECT` with `link: true` (which links records when any one field matches), a link group requires **all** listed fields to match for two records to be considered the same entity.

This is useful when individual field values are not unique enough for linking, but the combination is. For example, matching on `first_name` alone would produce false positives, but matching on `(first_name, last_name, date_of_birth)` together is reliable.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `fields` | `string[]` | **Yes** | Target field names that must ALL match for two records to be linked. Minimum 2 fields. Order does not matter. |

### Example

```yaml
resolutions:
  - name: person_resolution
    target:
      semantic_model: canonical
      dataset: person

    fields:
      first_name:
        strategy:
          type: COALESCE
      last_name:
        strategy:
          type: COALESCE
      date_of_birth:
        strategy:
          type: COALESCE
      email:
        strategy:
          type: LAST_MODIFIED

    link_groups:
      identity_match:
        fields: [first_name, last_name, date_of_birth]
```

In this example, two records from different sources are linked as the same person only when all three fields — `first_name`, `last_name`, and `date_of_birth` — are equal. A match on just first and last name is not sufficient.

### Link Groups vs. COLLECT with link

| Mechanism | Matching rule | Use when… |
|-----------|---------------|----------|
| `COLLECT` with `link: true` | Any single shared value links records | One field is a reliable unique identifier (e.g. email, tax ID) |
| `link_groups` | ALL fields in the group must match (tuple) | No single field is unique enough, but the combination is |

---

## Strategy Selection Guide

| Strategy | Use when… | Requires |
|----------|-----------|----------|
| **COALESCE** | One source is authoritative; others are fallbacks | `priority` on each field mapping |
| **LAST_MODIFIED** | The most recent update should win | `timestamp_field` or `default_timestamp_field` |
| **COLLECT** | You need all values (e.g. merging IDs, tags) | — |
| **EXPRESSION** | Custom aggregation logic is needed | SQL aggregation expression |

---

## Relationship to Mapping Files

Resolution files are separate from mapping files intentionally. A mapping defines **how** fields transform between systems. A resolution defines **which** transformed value wins when multiple sources contribute to the same target field.

```
┌──────────────┐     ┌──────────────┐
│  CRM Mapping │────▶│              │
│  (mapping)   │     │   Target     │◀── Resolution
│              │     │   Dataset    │    (which value wins?)
│  ERP Mapping │────▶│              │
│  (mapping)   │     └──────────────┘
└──────────────┘
```

Each mapping declares `id` (the source row identifier) so the resolution engine can match rows from different sources that represent the same entity.

---

## YAML Language Server Header

```yaml
# yaml-language-server: $schema=../specs/osi-resolution-schema.json
```
