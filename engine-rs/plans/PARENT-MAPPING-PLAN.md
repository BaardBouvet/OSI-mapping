# Parent mapping

**Status:** Planned

Unify `embedded: true` and `source.path` nested arrays under a single `parent:`
property. Replace implicit same-source discovery with explicit parent references.
Rename `source.path` to `array` / `array_path` at the mapping level.

## Motivation

The engine has two implicit parent-child mechanisms:

| Mechanism | How child is identified | How parent is discovered |
|-----------|------------------------|------------------------|
| **Embedded** | `embedded: true` on child | Implicit: find non-embedded mapping on same `source.dataset` |
| **Nested array** | `source.path` on child | Implicit: find mapping on same `source.dataset` without `path` |

Both mean "this mapping is subordinate to another mapping on the same source."
Neither explicitly names its parent. Problems:

- **Implicit coupling** — nothing in the YAML links child → parent.
- **Ambiguity** — if two non-embedded mappings share a source, which is the parent?
- **Misleading names** — "embedded" describes source shape, not the relationship.
  `source.path` lives inside `source:` but the child has no independent source.
- **`path` is ambiguous** — could point to a single object or an array. No way to
  tell from the name alone.

## Design

### Unified `parent:` property

Every child mapping explicitly names its parent via `parent: <mapping_name>`.
The child inherits `source` from the parent and must not specify its own.

**Embedded sub-entity** (flat columns from same row):
```yaml
- name: crm_customers
  source: { dataset: crm }
  target: customer
  fields: [...]

- name: crm_billing_address
  parent: crm_customers
  target: billing_address
  fields: [...]
```

**Nested array** (JSONB array expansion):
```yaml
- name: shop_orders
  source: { dataset: shop }
  target: order
  fields: [...]

- name: order_lines
  parent: shop_orders
  array: lines
  parent_fields:
    parent_order_id: order_id
  target: order_line
  fields: [...]
```

### `array` vs `array_path`

Consistent with `source` / `source_path` at the field level:

| Mapping property | Meaning | Example |
|-----------------|---------|---------|
| `array` | Direct JSONB array column name | `array: lines` |
| `array_path` | Dotted path to a JSONB array | `array_path: metadata.contacts` |

`array` is for single-segment column names (the common case).
`array_path` is for paths into nested JSON structure without needing an
intermediate mapping.

### Disambiguating child type

`parent:` alone → embedded (flat sub-entity from same row).
`parent:` + `array`/`array_path` → nested array (JSONB array expansion).

No overlap — the presence of `array`/`array_path` is unambiguous.

### Deep nesting

Today: multi-segment `source.path` like `children.grandchildren`.

After: each level is its own mapping with `parent:` referencing the previous:

Before:
```yaml
- name: source_children
  source:
    dataset: source
    path: children
    parent_fields: { parent_id: id }
  target: child

- name: source_grandchildren
  source:
    dataset: source
    path: children.grandchildren
    parent_fields: { parent_child_id: child_id }
  target: grandchild
```

After:
```yaml
- name: source_children
  parent: source_parents
  array: children
  parent_fields: { parent_id: id }
  target: child

- name: source_grandchildren
  parent: source_children
  array: grandchildren
  parent_fields: { parent_child_id: child_id }
  target: grandchild
```

Each level uses a single-segment `array` — the hierarchy is expressed by
the `parent:` chain, not by dotted compound paths.

### Semantics of `parent`

| Property | Behavior |
|----------|----------|
| `source` | **Inherited** from parent. Child must not specify `source`. |
| Primary key | Shared — child uses parent's `_src_id` for identity linkage. |
| Insert suppression | Embedded children cannot produce `insert` delta rows. |
| Merged delta | Embedded: engine merges parent + children into one delta row (LEFT JOIN). |
| Nested delta | Array: engine reconstructs JSONB arrays from child rows (GROUP BY + jsonb_agg). |
| Forward view | Each mapping still produces its own `_fwd_` view. |
| Identity/resolution | Each mapping targets its own entity independently. |

### Validation rules

1. `parent` must reference an existing mapping name within the same file.
2. `parent` and `source` are mutually exclusive on the child. Child inherits source.
3. Embedded children (no `array`/`array_path`) cannot have a parent that itself
   has `array`/`array_path` — no "embedded inside a nested array."
4. Nested children (`array`/`array_path`) can chain: a nested mapping can be
   parent to another nested mapping (deep nesting).
5. `array` and `array_path` are mutually exclusive.
6. `array`/`array_path` requires `parent` (cannot appear on a root mapping).

### Properties promoted to mapping level

These move from inside `source:` to the mapping level:

| Before (inside `source:`) | After (mapping level) |
|---------------------------|----------------------|
| `source.path` | `array` or `array_path` |
| `source.parent_fields` | `parent_fields` |

The `source:` block on child mappings disappears entirely. The child has:
```yaml
- name: child_mapping
  parent: parent_mapping_name
  array: column_name           # optional — only for nested arrays
  parent_fields: { ... }       # optional — only for nested arrays
  target: target_name
  fields: [...]
```

## Implementation

### Phase 1 — Model (`model.rs`)

**Remove from `Mapping`:**
- `pub embedded: bool`

**Add to `Mapping`:**
- `pub parent: Option<String>`
- `pub array: Option<String>`
- `pub array_path: Option<String>`
- `pub parent_fields: IndexMap<String, ParentFieldRef>` (moved from SourceRef)

**Keep on `SourceRef` for parsing compatibility** during the transition:
- `path`, `parent_fields` — remove after all examples are migrated.

**Add helpers:**
- `fn is_child(&self) -> bool` → `self.parent.is_some()`
- `fn is_nested(&self) -> bool` → `self.array.is_some() || self.array_path.is_some()`
- `fn effective_array(&self) -> Option<&str>` → returns `array` or `array_path`

### Phase 2 — Parser post-processing

After deserialization, for each mapping with `parent`:
1. Resolve the parent mapping by name.
2. Copy `source` from parent to child (populate `SourceRef`).
3. If `array`/`array_path` is set, populate `source.path` internally so the
   render pipeline sees the same structure it expects today.
4. Move `parent_fields` from mapping level into `source.parent_fields`.

This **keeps the render pipeline unchanged** initially — it still reads
`source.path` and `source.parent_fields`. The new properties are the
public API; the old SourceRef fields become internal.

### Phase 3 — Validation (`validate.rs`)

Add:
- `parent` must reference an existing mapping name.
- `parent` and `source` on the same mapping is an error.
- `array` and `array_path` are mutually exclusive.
- `array`/`array_path` requires `parent`.
- Embedded parent chain depth = 1 (no embedded-of-embedded).
- Nested chains allowed (array parent can have array parent).

### Phase 4 — Delta render (`delta.rs`)

Replace all 27 `embedded` references:

| Current code | Replacement |
|-------------|-------------|
| `mapping.embedded` | `mapping.is_child() && !mapping.is_nested()` |
| `!m.embedded` (find primary) | `!m.is_child()` |
| `m.embedded` (find embedded children) | `m.is_child() && !m.is_nested()` |
| `render_delta_with_embedded(...)` | `render_delta_with_children(...)` |
| `embedded_mappings` | `child_mappings` |
| `embedded_with_reverse` | `children_with_reverse` |

Nested array detection (currently `source.path.is_some()`) stays the same
internally since Phase 2 populates `source.path` from `array`/`array_path`.

### Phase 5 — Forward render (`forward.rs`)

No changes needed — Phase 2 ensures `source.path` and `source.parent_fields`
are populated, so the LATERAL jsonb_array_elements logic works unchanged.

### Phase 6 — Schema (`spec/mapping-schema.json`)

Remove:
- `embedded` property
- `source.path` property
- `source.parent_fields` property

Add to mapping level:
```json
"parent": {
  "type": "string",
  "description": "Name of the parent mapping. Inherits source from parent."
},
"array": {
  "type": "string",
  "description": "JSONB array column to expand into rows. Requires parent."
},
"array_path": {
  "type": "string",
  "pattern": "^[^.]+\\..+$",
  "description": "Dotted path to a JSONB array to expand. Requires parent."
},
"parent_fields": {
  "type": "object",
  "description": "Map of local field aliases to parent column names."
}
```

Constraints: mapping needs either `source` or `parent` (not both).

### Phase 7 — Examples

**Embedded examples (9 files):**

| Example | Parent mapping | Child mapping(s) |
|---------|---------------|-------------------|
| embedded-simple | erp_accounts | crm_billing, crm_shipping, billing_addr |
| embedded-multiple | crm_customers | crm_billing_address, crm_shipping_address, billing_accounts |
| embedded-objects | crm_address | (ERP side) |
| embedded-vs-many-to-many | crm_company_contacts, crm_contacts | crm_association |
| multiple-target-mappings | crm_billing | crm_shipping |
| relationship-mapping | erp_contacts | erp_contact_company |
| relationship-embedded | erp_companies | erp_company_contacts, erp_company_primary |
| merge-partials | crm_organizations | crm_invoice_flag |
| route-embedded | crm_customers | crm_billing_address, crm_shipping_address |

Changes: remove `embedded: true`, remove `source:`, add `parent:`.

**Nested array examples (3 files):**

| Example | Mappings with path | Change |
|---------|-------------------|--------|
| nested-arrays | shop_lines (`path: lines`) | `parent: shop_orders`, `array: lines` |
| nested-arrays-deep | source_children (`path: children`), source_grandchildren (`path: children.grandchildren`) | Chain: `parent: source_parents` + `array: children`, then `parent: source_children` + `array: grandchildren` |
| nested-arrays-multiple | source_departments (`path: departments`), source_employees (`path: departments.employees`), source_projects (`path: projects`), source_tasks (`path: projects.tasks`) | Four parent references, each with single-segment `array` |

Changes: remove `source.path` and `source.parent_fields`, add `parent:`,
`array:`, `parent_fields:` at mapping level.

### Phase 8 — Documentation

Update all doc files:

| File | Change |
|------|--------|
| docs/design-rationale.md | "Embedded Entities" → "Sub-Mappings (parent)" |
| docs/schema-reference.md | Property table, `source.path` → `array`/`array_path` |
| docs/ai-guidelines.md | Embedded + nested sections unified |
| engine-rs/docs/design-decisions.md | Delta merge + nested reconstruction docs |

## Execution order

1. Model (add `parent`, `array`, `array_path`, `parent_fields` to Mapping)
2. Parser post-processing (resolve parent, populate internal SourceRef)
3. Validation (new rules)
4. Delta render (replace `embedded` → `is_child()`)
5. Schema update
6. Embedded examples (9 files)
7. Nested array examples (3 files)
8. Documentation
9. Test — all 36 examples must pass

## Risk

Low. The render pipeline sees the same internal structures (SourceRef with path
and parent_fields). The change is primarily in the YAML surface and the model
layer that maps new properties to existing internals.
