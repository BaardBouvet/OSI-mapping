# Resolution Schema Reference

Schema file: [`osi-resolution-schema.json`](../specs/osi-resolution-schema.json)

A resolution document defines how to handle conflicts when **multiple mappings write to the same target dataset**. Each field in the target gets an explicit strategy that determines which value wins.

Resolution is only needed when a target dataset receives data from more than one source. Single-source targets need no resolution rules.

## Document Structure

```yaml
version: "1.0"
resolutions:
  - name: ...
    # ... Resolution entries
```

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `version` | string | yes | Must be `"1.0"` |
| `resolutions` | array of [Resolution](#resolution) | yes | One or more resolution entries |

---

## Resolution

Resolution rules for a single target dataset.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `name` | string | yes | Unique identifier. Must match `^[a-z][a-z0-9_]*$` |
| `description` | string | no | Human-readable description |
| `target` | [ModelRef](mapping-schema.md#modelref) | yes | The target dataset these rules apply to |
| `fields` | object&lt;string, [FieldResolution](#fieldresolution)&gt; | yes | Per-field resolution rules. Keys are target field names. |

### Example

```yaml
- name: company_resolution
  description: Resolution rules for the company dataset
  target:
    semantic_model: acme_inc_model
    dataset: company
    model_file: ./model-acme.yaml
  fields:
    email:
      strategy:
        type: COLLECT
        link: true
    name:
      strategy:
        type: LAST_MODIFIED
    account:
      strategy:
        type: COALESCE
```

---

## FieldResolution

Resolution rules for a single target field.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `strategy` | [Strategy](#strategy) | yes | The resolution strategy to apply |

---

## Strategy

Discriminated by `type`. Exactly one of the following:

### COALESCE

Pick the value from the **highest-priority source** (lowest `priority` number wins). Sources declare their priority via the `priority` property on [FieldMapping](mapping-schema.md#fieldmapping).

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `type` | `"COALESCE"` | yes | Strategy discriminator |

```yaml
strategy:
  type: COALESCE
```

**Mapping-side requirement**: Each FieldMapping contributing to this field should set `priority` (integer ≥ 1). Lower number = higher priority. If two sources have the same priority, the result is undefined.

**Reverse direction**: The winning source's `reverse_expression` is used to write back.

### LAST_MODIFIED

Pick the value from the **most recently updated source**. Sources declare their timestamp via the `timestamp_field` property on [FieldMapping](mapping-schema.md#fieldmapping).

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `type` | `"LAST_MODIFIED"` | yes | Strategy discriminator |

```yaml
strategy:
  type: LAST_MODIFIED
```

**Mapping-side requirement**: Each FieldMapping contributing to this field should set `timestamp_field` to the name of a source column containing a modification timestamp.

**Reverse direction**: The winning source's `reverse_expression` is used to write back.

### COLLECT

Collect **all source values into an array**. No single winner — all contributions are kept.

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `type` | `"COLLECT"` | yes | Strategy discriminator |
| `link` | boolean | no | When true, values are used for **entity linking** via transitive closure |

```yaml
# Simple collection
strategy:
  type: COLLECT

# Collection with entity linking
strategy:
  type: COLLECT
  link: true
```

**Entity linking** (`link: true`): Records from different sources that share a collected value are linked as the same logical entity. All values are retained for transitive closure — if source A has email `x@co.com` and source B has email `x@co.com`, they merge into one entity with all their respective data.

**Reverse direction**: COLLECT fields are inherently one-way. The FieldMappings for COLLECT fields should omit `reverse_expression`.

---

## How Resolution Connects to Mappings

Resolution and mapping schemas work together through shared conventions:

| Resolution Strategy | FieldMapping Property | Purpose |
|--------------------|-----------------------|---------|
| COALESCE | `priority` | Determines winner (lower wins) |
| LAST_MODIFIED | `timestamp_field` | Determines winner (latest wins) |
| COLLECT | — | All values kept; omit `reverse_expression` |

The `target` in a Resolution entry matches the `target` in one or more Mapping entries. The field names in `fields` correspond to `target_field` values in those mappings' `field_mappings`.
