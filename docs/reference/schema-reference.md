# Schema Reference

Complete reference for every primitive in the Integration Mapping Schema. Each section documents one schema type, its properties, and links to examples that demonstrate it.

---

## Document Root

A mapping file is a YAML document with four top-level keys.

| Property | Type | Required | Description |
|---|---|---|---|
| `version` | string | **yes** | Always `"1.0"` |
| `description` | string | no | Human-readable summary of what this mapping does |
| `sources` | object | no | Source dataset metadata (keys are dataset names) ([Source](#source)) |
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
    source: crm_a
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

**Examples:** [hello-world](../examples/hello-world/)

---

## Source

Metadata for a source dataset. Declared under the top-level `sources:` key.

| Property | Type | Required | Description |
|---|---|---|---|
| `table` | string | no | Physical table/view name (defaults to the source key) |
| `primary_key` | string or string[] | **yes** | Column(s) that uniquely identify a source row |

Source names (keys under `sources`) must match `^[a-z][a-z0-9_]*$`.

```yaml
sources:
  crm:
    primary_key: id
  erp_order_lines:
    table: erp_order_lines
    primary_key: [order_id, line_no]
```

The primary key identifies each source row throughout the pipeline:
- Single-column PKs use the column value directly; composite PKs are serialized as a JSON object
- Used for entity identity, change detection, and reconstructing source PKs in the output

**Examples:** Every example with `sources:` declared. See [composite-keys](../examples/composite-keys/) for composite PKs, [references](../examples/references/) for multiple sources.

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

Allowed values: `identity`, `coalesce`, `last_modified`, `collect`, `bool_or`.

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

**Examples:** [hello-world](../examples/hello-world/), [composite-keys](../examples/composite-keys/)

#### `coalesce`

Picks the best non-null value based on priority. Requires `priority` on the corresponding field mappings (lower number wins).

```yaml
name: coalesce
```

**Examples:** [hello-world](../examples/hello-world/), [merge-threeway](../examples/merge-threeway/)

#### `last_modified`

Most recently changed value wins. Requires a `last_modified` timestamp on the mapping or field mapping.

When a mapping has no timestamp (both per-field timestamps and mapping-level `_last_modified` are NULL), its contribution sorts last — any mapping with an actual timestamp wins. If all contributions have NULL timestamps, the implicit row order (mapping declaration order) determines the winner, behaving like coalesce without priority.

```yaml
name: last_modified
```

**Examples:** [embedded-simple](../examples/embedded-simple/), [value-groups](../examples/value-groups/)

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

#### `bool_or`

Resolves to `true` if **any** contributing source has a truthy value . Useful for boolean flags that propagate across sources — e.g., a deletion flag where any system marking the record deleted makes the resolved value `true`.

```yaml
is_deleted:
  strategy: bool_or
```

The underlying SQL aggregation is `bool_or((field)::boolean)`. Source values are cast to boolean before aggregation.

**Examples:** [propagated-delete](../examples/propagated-delete/)

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

### `references_field`

Controls what value is returned when a reference is translated back to the source's FK column during reverse mapping. By default the engine returns the referenced source's primary key value — which is correct for standard foreign keys. Set `references_field` when the source stores a different representation of the entity reference.

**The problem it solves:** A vocabulary or lookup table might be keyed by `name` but the referencing source stores the ISO code, not the name. Without `references_field`, the reverse mapping would return the primary key (`name: "Denmark"`) when the source actually expects an ISO code (`country_code: "DK"`).

```yaml
# CRM stores ISO codes, ERP stores full names — same country entity
- name: crm_system
  source: crm_system
  target: customer
  fields:
    - source: country_code
      target: country
      references: country_vocabulary
      references_field: iso_code   # return iso_code, not the PK (name)

- name: erp_system
  source: erp_system
  target: customer
  fields:
    - source: country
      target: country
      references: country_vocabulary
      references_field: name       # return name (happens to be the PK)
```

**Examples:** [vocabulary-standard](../examples/vocabulary-standard/) (vocabulary with ISO codes and full names)

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
| `source` | string | * | Source name — must match a key in `sources` (required unless `parent` is set) |
| `target` | string | **yes** | Target entity name — must match a key in `targets` |
| `parent` | string | * | Name of parent mapping — inherits source (mutually exclusive with `source`) |
| `array` | string | no | JSONB array column to expand into rows (requires `parent`) |
| `array_path` | string | no | Dotted path to a JSONB array (requires `parent`, mutually exclusive with `array`) |
| `parent_fields` | object | no | Map of local field aliases to parent column names (used with `array`/`array_path`) |
| `fields` | array of [FieldMapping](#fieldmapping) | **yes** | Field-level mappings |
| `priority` | integer | no | Mapping-level coalesce priority (lower wins) |
| `last_modified` | [TimestampRef](#timestampref) | no | Mapping-level timestamp for last_modified resolution |
| `filter` | string | no | Forward filter: SQL WHERE condition |
| `reverse_filter` | string | no | Reverse filter: SQL WHERE condition |

| `links` | array of [LinkRef](#linkref) | no | External identity edges from a linking table |
| `link_key` | string | no | Column in linking table providing pre-computed cluster ID (IVM-safe) |
| `cluster_members` | boolean / object | no | ETL feedback table for insert tracking |
| `cluster_field` | string | no | Source column holding a pre-populated cluster ID |
| `written_state` | boolean / object | no | ETL-maintained table tracking last-written values |
| `derive_noop` | boolean | no | Target-centric noop detection via written state |
| `derive_timestamps` | boolean | no | Per-field timestamp derivation via written state |
| `derive_tombstones` | string | no | Target field to synthesize for absent entities or elements |
| `soft_delete` | string or object | no | Soft-delete detection configuration |
| `passthrough` | array of strings | no | Source columns carried through to delta output |

```yaml
mappings:
  - name: crm
    source: crm
    target: contact
    priority: 1
    last_modified: updated_at
    fields:
      - source: email
        target: email
      - source: name
        target: name
```

### `parent`

Explicitly names the parent mapping. The child inherits `source` from the parent and must not specify its own `source`. Without `array`/`array_path`, the child is an embedded sub-entity (flat columns from the same row). With `array`/`array_path`, the child expands a JSONB array.

**Embedded sub-entity:**

```yaml
  - name: order_header
    source: orders
    target: order
    fields: [...]

  - name: order_address
    parent: order_header
    target: shipping_address
    fields:
      - source: ship_street
        target: street
      - source: ship_city
        target: city
```

**Nested array expansion:**

```yaml
  - name: shop_orders
    source: shop
    target: order
    fields: [...]

  - name: order_lines
    parent: shop_orders
    array: lines
    parent_fields:
      parent_order_id: order_id
    target: order_line
    fields:
      - source: parent_order_id
        target: order_ref
      - source: line_num
        target: line_number
```

**Deep nesting** — each level references the previous as parent:

```yaml
  - name: source_children
    parent: source_parents
    array: children
    parent_fields: { parent_id: id }
    target: child
    fields: [...]

  - name: source_grandchildren
    parent: source_children
    array: grandchildren
    parent_fields: { parent_child_id: child_id }
    target: grandchild
    fields: [...]
```

**Examples:** [embedded-simple](../examples/embedded-simple/), [embedded-objects](../examples/embedded-objects/), [embedded-multiple](../examples/embedded-multiple/), [route-embedded](../examples/route-embedded/), [nested-arrays](../examples/nested-arrays/), [nested-arrays-deep](../examples/nested-arrays-deep/), [nested-arrays-multiple](../examples/nested-arrays-multiple/)

### `filter` / `reverse_filter`

SQL WHERE conditions that control which rows flow through the mapping.

- `filter` — forward: only source rows matching this condition map to the target
- `reverse_filter` — reverse: only resolved target rows matching this condition map back to this source

```yaml
  - name: active_contacts
    source: crm
    target: contact
    filter: "status = 'active'"
    reverse_filter: "type LIKE '%customer%'"
    fields: [...]
```

**Examples:** [route](../examples/route/), [route-combined](../examples/route-combined/), [route-multiple](../examples/route-multiple/), [types](../examples/types/), [inserts-and-deletes](../examples/inserts-and-deletes/)

### `links`

External identity edges from a linking table. Each link references a column in the linking table and a source mapping. The engine generates pairwise edges fed into the identity algorithm's connected-components computation.

A mapping with `links` but no `fields` is a "linkage-only" mapping — it contributes identity edges without business data.

```yaml
  - name: match_links
    source: match_results
    target: contact
    links:
      - field: crm_id
        references: crm
      - field: erp_id
        references: erp
```

### `link_key`

Column in the linking table providing a pre-computed cluster ID. Enables the IVM-safe path: the cluster ID and the source row arrive atomically without a separate join step.

```yaml
  - name: mdm_links
    source: mdm_xref
    target: contact
    link_key: cluster_id
    links:
      - field: crm_id
        references: crm
      - field: erp_id
        references: erp
```

Without `link_key`, links are processed in the identity layer via pairwise edges (batch-safe but not IVM-safe).

### `cluster_members`

ETL feedback table for insert tracking. When an insert is produced, the ETL writes the generated ID back to this table so the next run links the new row to its cluster.

`true` uses defaults; an object overrides table/column names.

| Property | Default | Description |
|---|---|---|
| `table` | `_cluster_members_{mapping}` | Table name |
| `cluster_id` | `_cluster_id` | Cluster ID column |
| `source_key` | `_src_id` | Source PK column |

```yaml
  - name: erp
    source: erp
    target: contact
    cluster_members: true                  # all defaults
    fields: [...]

  - name: legacy
    source: legacy
    target: contact
    cluster_members:                       # custom names
      table: legacy_feedback
      cluster_id: entity_id
      source_key: record_id
    fields: [...]
```

When an entity has an entry in the cluster_members table, its cluster ID is used for identity linking. Otherwise, a default identity is derived. Rows sharing the same cluster ID are linked by the identity algorithm.

**Examples:** [inserts-and-deletes](../examples/inserts-and-deletes/)

### `cluster_field`

Column in the source table holding a pre-populated cluster ID from ETL feedback. Simpler than `cluster_members` when the target system supports storing custom fields on records.

```yaml
  - name: billing
    source: billing
    target: customer
    cluster_field: entity_cluster_id
    fields: [...]
```

When the column is populated, its value is used as the cluster ID; otherwise, a default identity is derived. A mapping should declare `cluster_members` or `cluster_field`, not both.

### `written_state`

ETL-maintained table tracking what was last written to the target system. Enables target-centric noop detection (via `written_noop`) and provides the infrastructure for hard-delete detection. Follows the same pattern as `cluster_members`: the engine reads the table but never writes to it — the ETL updates it after each sync cycle.

`true` uses defaults; an object overrides table/column names.

| Property | Default | Description |
|---|---|---|
| `table` | `_written_{mapping}` | Table name |
| `cluster_id` | `_cluster_id` | Cluster ID column |
| `written` | `_written` | JSONB column holding last-written field values |
| `written_at` | `_written_at` | Row-level write timestamp column |
| `written_ts` | `_written_ts` | JSONB column holding per-field timestamps |

```yaml
  - name: erp
    source: erp
    target: contact
    written_state: true              # all defaults
    derive_noop: true                # opt-in target-centric noop
    fields: [...]

  - name: legacy
    source: legacy
    target: contact
    written_state:                   # custom names
      table: legacy_write_log
      cluster_id: entity_id
      written: last_payload
    fields: [...]
```

When source-level change detection sees a change, a second comparison checks whether the resolved values actually differ from what was last written. If not, the change is classified as noop — avoiding redundant writes.

**Table contract:** After each sync cycle, the ETL writes what it actually sent to the target:
- Insert: add row with the cluster ID and written field values
- Update: replace the written values with newly written values
- Delete: remove row
- Noop: no change

**Examples:** [derive-noop](../examples/derive-noop/)

### `derive_noop`

When `true` (requires `written_state`), enables target-centric noop detection after source-level change detection. If the resolved values match what was last written, the row is classified as noop even though the source changed.

Appropriate when the ETL is the sole writer to the target. If external actors modify the target after the ETL write, the written state becomes stale and the engine may incorrectly suppress updates.

```yaml
  - name: erp
    source: erp
    target: contact
    written_state: true
    derive_noop: true
    fields: [...]
```

**Examples:** [derive-noop](../examples/derive-noop/)

### `derive_timestamps`

When `true` (requires `written_state`), derives per-field timestamps by comparing current source values against previously written values. Changed fields get the latest write timestamp; unchanged fields carry forward their existing per-field timestamp. On bootstrap (no written state), timestamps fall back to the source timestamp or NULL.

```yaml
  - name: csv_import
    source: csv_import
    target: customer
    written_state: true
    derive_timestamps: true
    cluster_field: cluster_id
    fields: [...]
```

**Examples:** [derive-timestamps](../examples/derive-timestamps/)

### `soft_delete`

Soft-delete detection.  Declares a source column that signals deletion.  The `strategy` fully determines detection and undelete values — no overrides needed.

Accepts a string shorthand (field name, defaults to `timestamp` strategy) or an object with `field` and optional `strategy` and `target`.

| Property | Type | Required | Description |
|---|---|---|---|
| `field` | string | **yes** | Source column carrying the deletion signal |
| `strategy` | enum | no | `timestamp` (default), `deleted_flag`, or `active_flag` |
| `target` | string | no | Target field name. Routes detection into a resolved field instead of suppressing |

Strategy table:

| Strategy | Detection | Undelete value | Common fields |
|---|---|---|---|
| `timestamp` | `"field" IS NOT NULL` | `NULL` | `deleted_at`, `removed_at` |
| `deleted_flag` | `"field" IS NOT FALSE` | `FALSE` | `is_deleted`, `deleted` |
| `active_flag` | `"field" IS NOT TRUE` | `TRUE` | `is_active`, `active` |

Behavior depends on `target`:

| `target` | Effect |
|---|---|
| not set | Suppress — row excluded from delta (local handling) |
| set | Detection result routed to named target field. Non-identity fields auto-nullified so the source yields the floor in resolution. Consumers use `reverse_filter` to react. |

The `soft_delete` field is auto-included as a passthrough column.

```yaml
  # Suppress locally (no target — current default)
  - name: crm
    source: crm
    target: customer
    soft_delete: deleted_at
    fields: [...]
```

```yaml
  # Route detection into a resolved target field (propagated)
  - name: crm
    source: crm
    target: customer
    soft_delete: { field: deleted_at, target: is_deleted }
    reverse_filter: "is_deleted IS NOT TRUE"
    fields: [...]
```

```yaml
  # Active flag with propagation
  - name: crm
    source: crm
    target: customer
    soft_delete: { field: is_active, strategy: active_flag, target: is_deleted }
    reverse_filter: "is_deleted IS NOT TRUE"
    fields: [...]
```

**Examples:** [soft-delete](../examples/soft-delete/)

---

### `derive_tombstones`

Target field to synthesize for absent entities or elements.  Works at both levels:

- **Root mapping** (requires `cluster_members`): detects entity absence — entities present in `cluster_members` but missing from the source table contribute `TRUE` to the named field.
- **Child mapping** (requires parent with `written_state`): detects element absence — elements previously written but now absent from the source contribute `TRUE` to the named field.

Resolution propagates the deletion via the target field's strategy (typically `bool_or`).

**Entity-level:**

```yaml
  - name: erp_customers
    source: erp
    target: customer
    cluster_members: true
    derive_tombstones: is_deleted
    reverse_filter: "is_deleted IS NOT TRUE"
    fields: [...]
```

**Element-level:**

```yaml
  - name: blog_cms_recipes
    source: blog_cms
    target: recipe
    written_state: true
    fields: [...]

  - name: blog_cms_steps
    parent: blog_cms_recipes
    array: steps
    target: recipe_step
    derive_tombstones: is_removed
    fields: [...]
```

When an entity or element disappears from its source:

1. Absence detected: synthetic row with the named field = `TRUE`, all other fields NULL
2. Resolution: `bool_or(TRUE, ...)` → `TRUE`
3. Each consumer's `reverse_filter` determines the reaction

**Examples:** [hard-delete](../examples/hard-delete/), [element-hard-delete](../examples/element-hard-delete/)

---

## LinkRef

A link from a linking table field to a source mapping.

| Property | Type | Required | Description |
|---|---|---|---|
| `field` | string / string[] / object | **yes** | Column(s) in the linking table referencing the target source's PK |
| `references` | string | **yes** | Name of the source mapping being referenced |

```yaml
links:
  - field: crm_id
    references: crm
  - field: erp_id
    references: erp
```

For composite PKs, `field` can be an array (same-name columns) or an object (renamed columns):

```yaml
links:
  - field: [order_id, line_no]
    references: erp_lines
  - field: { src_order: order_id, src_line: line_no }
    references: erp_lines
```

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
| `order` | boolean | no | Generate deterministic per-array ordinal key in forward path |
| `order_prev` | boolean | no | Derive previous element key (adjacency metadata) |
| `order_next` | boolean | no | Derive next element key (adjacency metadata) |
| `references` | string | no | Mapping name for FK reverse resolution (see below) |
| `normalize` | string | no | SQL expression with `%s` placeholder for precision-loss noop comparison |
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

### Ordering metadata: `order`, `order_prev`, `order_next`

Use `order: true` on an array mapping field to generate deterministic ordering
metadata from source array position.

```yaml
- target: step_order
  order: true
```

`order_prev` and `order_next` are optional adjacency helpers for graph-like
ordering models.

```yaml
- target: prev_step
  order_prev: true
- target: next_step
  order_next: true
```

In mixed-order setups (some mappings contribute native sort keys and others use
`order: true`), reverse output arrays are emitted in canonical computed order.
Consumers should process reverse ETL arrays in emitted order instead of relying
on source-side recomputation of sort keys for generated-only rows.

**Examples:** [crdt-ordering](../examples/crdt-ordering/), [crdt-ordering-native](../examples/crdt-ordering-native/)

### `references` (field mapping)

Specifies which mapping's source identities to use when translating a target entity reference back to a source FK value.

**When to use:** When a target field has `references:` (on the [TargetFieldDef](#targetfielddef)) declaring it as an entity FK, and your mapping maps a source FK column to that target field.

**Key distinction:** There are two different `references:` in the system:

| Location | Purpose | Example |
|---|---|---|
| **Target field** (`targets.*.fields.*.references`) | Declares that this target field is an entity reference to another target type | `primary_contact: { strategy: coalesce, references: company }` |
| **Field mapping** (`mappings.*.fields.*.references`) | Tells the engine which mapping to use for translating the reference back to a source FK | `references: crm_company` |

The target-level one says *what* the reference points to. The field-mapping one says *how* to reverse-resolve it for this particular source system.

```yaml
# Target declares the entity reference
targets:
  person:
    fields:
      primary_contact:
        strategy: coalesce
        references: company     # FK to company entity

# Each mapping specifies which mapping to resolve through
mappings:
  - name: crm_contact
    source: crm_contacts
    target: person
    fields:
      - source: company_id
        target: primary_contact
        references: crm_company  # resolve via CRM company mapping

  - name: erp_contact
    source: erp_contacts
    target: person
    fields:
      - source: customer_ref
        target: primary_contact
        references: erp_customer # resolve via ERP company mapping
```

Without `references`, the raw target-level entity reference value is passed through without translating it back to the source namespace.

**Examples:** [references](../examples/references/), [reference-preservation](../examples/reference-preservation/), [composite-keys](../examples/composite-keys/), [vocabulary-standard](../examples/vocabulary-standard/), [vocabulary-custom](../examples/vocabulary-custom/)

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

### `normalize`

SQL expression with a `%s` placeholder, applied to both sides of the delta noop comparison. Handles precision loss when a target system has lower fidelity than the golden record — numeric rounding, string truncation, case folding.

The same expression is substituted into both the source snapshot and the reverse-projected value before comparison. If both normalize to the same result, the difference is classified as expected loss (noop) rather than a change.

```yaml
# Integer-precision system — truncate before comparing
- source: price
  target: price
  normalize: "trunc(%s::numeric, 0)::integer::text"

# 40-char VARCHAR system — truncate before comparing
- source: name
  target: name
  normalize: "left(%s, 40)"

# Uppercase-only system — case-fold before comparing
- source: name
  target: name
  normalize: "upper(%s)"
```

When `normalize` is declared on a field with `last_modified` strategy, the engine also uses it for echo-aware resolution: a lower-precision source whose normalized value matches a higher-precision source is not allowed to win resolution and degrade the golden record.

**Examples:** [precision-loss](../examples/precision-loss/)

---

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

**Examples:** [value-groups](../examples/value-groups/), [merge-internal](../examples/merge-internal/)

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

Omit a key when that category is empty. Rows not listed in any category are implicitly **noops** — source rows where the resolved values match the original values, requiring no write.

### Matching policy

Expected rows must match the **complete** actual output — every column, every value. Partial assertions (listing only a subset of fields) are not allowed; this ensures tests genuinely verify the full pipeline rather than silently ignoring regressions.

- **All source columns** present in the input must appear in the expected row (including unmapped columns like timestamps).
- **All reverse-mapped fields** must appear with their resolved values (or `null` when resolution produces no value).
- **`_base` may be omitted** from expected rows for brevity. The test harness strips `_base` from both sides before comparison.
- **Insert rows** must include `_cluster_id` (seed notation like `"mapping:src_id"`) plus every reverse-mapped field. Fields that cannot be reverse-resolved appear as `null`.

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
