# Example coverage for remaining schema properties

**Status:** Done

Seven schema properties lack dedicated example coverage. This plan proposes five new examples (grouping naturally related properties) to close the gap.

---

## Properties to cover

| Property | Current coverage | Plan |
|---|---|---|
| `array_path` | None | New example: `nested-array-path` |
| `links` / `LinkRef` | None | New example: `external-links` |
| `link_key` | None | Same example: `external-links` |
| `elements: last_modified` | None (`element-priority` covers `elements: coalesce`) | New example: `element-last-modified` |
| `scalar` | None | New example: `scalar-array` |
| `soft_delete` on child | None (root-level shown in `soft-delete`, `soft-delete-resurrect`) | New example: `soft-delete-child` |
| `order_prev` / `order_next` | None (`crdt-ordering` covers `order: true` only) | Extend `crdt-ordering` or new example |

---

## Proposed examples

### 1. `nested-array-path` — JSONB array at a dotted path

**Demonstrates:** `array_path`

**Scenario:** An e-commerce source stores product specifications as nested JSON:
`{ "spec": { "measurements": [{ "type": "height", "value": 180 }, ...] } }`.
A warehouse source stores measurements as a top-level JSONB array.
Both map to a `product_measurement` child target. The e-commerce mapping uses
`array_path: spec.measurements` to reach the nested array without intermediate
flattening.

**Key fields:**
```yaml
mappings:
  - name: shop_measurements
    parent: shop_products
    array_path: spec.measurements
    target: product_measurement
    fields:
      - source: type
        target: measurement_type
      - source: value
        target: measurement_value
```

**Test:** Shop product with nested measurements merges with warehouse product.
Reverse output reconstructs the nested JSON path.

---

### 2. `external-links` — linking table with pre-computed clusters

**Demonstrates:** `links`, `LinkRef`, `link_key`

**Scenario:** Two CRMs each have their own customer table. An MDM system produces a
`customer_xref` table with columns `cluster_id`, `crm_a_id`, `crm_b_id` — the output
of a matching process. The engine uses `links` to declare identity edges and `link_key`
to consume the pre-computed cluster ID, enabling IVM-safe identity resolution.

**Key fields:**
```yaml
mappings:
  - name: mdm_xref
    source: customer_xref
    target: customer
    link_key: cluster_id
    links:
      - field: crm_a_id
        references: crm_a_customers
      - field: crm_b_id
        references: crm_b_customers

  - name: crm_a_customers
    source: crm_a
    target: customer
    fields:
      - source: email
        target: email
      - source: name
        target: name
        priority: 1

  - name: crm_b_customers
    source: crm_b
    target: customer
    fields:
      - source: email
        target: email
      - source: name
        target: name
        priority: 2
```

**Test:** Two CRM customers linked by the xref table merge into one golden record.
CRM A name wins (priority 1), CRM B gets an update.

---

### 3. `element-last-modified` — timestamp-based element set resolution

**Demonstrates:** `elements: last_modified`

**Scenario:** Two project management tools contribute task lists to the same project.
The `task` child target uses `elements: last_modified` — whichever source most recently
modified its task set provides the entire list for that project. This prevents stale
tasks from one source mixing with current tasks from another.

**Key fields:**
```yaml
targets:
  task:
    elements: last_modified
    fields:
      task_name:
        strategy: identity
      assignee:
        strategy: last_modified

mappings:
  - name: tool_a_tasks
    parent: tool_a_projects
    array: tasks
    target: task
    fields:
      - source: name
        target: task_name
        last_modified: updated_at
      - source: owner
        target: assignee
        last_modified: updated_at
```

**Test:** Tool A has tasks updated at 10:00, Tool B at 09:00 — Tool A's task set wins.
Tool B's tasks are excluded entirely (not merged element-by-element).

---

### 4. `scalar-array` — bare scalar array extraction

**Demonstrates:** `scalar: true`

**Scenario:** A CRM stores customer tags as a bare JSONB array `["vip", "newsletter"]`.
An ERP stores tags in a normalized `customer_tags` table with one row per tag. Both map
to a `customer_tag` child target. The CRM mapping uses `scalar: true` to extract bare
string values directly from array elements.

**Key fields:**
```yaml
mappings:
  - name: crm_tags
    parent: crm_customers
    array: tags
    target: customer_tag
    fields:
      - target: tag
        scalar: true
```

**Test:** CRM customer with `["vip", "newsletter"]` and ERP customer with one `vip` tag
merge — the combined tag set contains both tags. Delta reconstructs the CRM array as
`["vip", "newsletter"]` (bare scalars, not objects).

---

### 5. `soft-delete-child` — element-level soft delete on nested arrays

**Demonstrates:** `soft_delete` on child mapping

**Scenario:** An invoicing system stores line items as a JSONB array where each element
has a `voided_at` timestamp. When a line is voided, it should be suppressed from the
resolved output — other systems should not see voided lines. If the line is later
un-voided (timestamp set back to null), it reappears.

**Key fields:**
```yaml
mappings:
  - name: invoice_lines
    parent: invoices
    array: lines
    target: invoice_line
    soft_delete: voided_at
    fields:
      - source: line_number
        target: line_number
      - source: description
        target: description
      - source: amount
        target: amount
```

**Test:** Invoice with three lines, one voided — delta view shows two active lines.
Second system's reverse output excludes the voided line.

---

### 6. `order_prev` / `order_next` — extend `crdt-ordering` or new example

**Demonstrates:** `order_prev`, `order_next` (Tier 2 CRDT linked-list ordering)

**Decision needed:** The existing `crdt-ordering` example covers Tier 1 (`order: true`).
Options:

- **Option A:** Add a second test case to `crdt-ordering` that adds `order_prev` and
  `order_next` fields, demonstrating the upgrade from ordinal to linked-list merge.
- **Option B:** New standalone `crdt-ordering-linked` example.

Option A is preferred (avoids proliferation), but depends on whether a single mapping
can cleanly demonstrate both tiers in separate tests.

**Key fields (Tier 2):**
```yaml
- target: task_order
  order: true
- target: prev_task
  order_prev: true
- target: next_task
  order_next: true
```

**Test:** Two sources contribute interleaved items — linked-list pointers enable
deterministic merge without positional conflicts.

---

## Execution order

Implement in dependency order (simplest first, building on prior patterns):

1. `scalar-array` — minimal, self-contained
2. `element-last-modified` — straightforward child target strategy
3. `nested-array-path` — extends nested-arrays pattern
4. `soft-delete-child` — extends soft-delete pattern
5. `external-links` — new identity concept (links + link_key)
6. `crdt-ordering` extension — depends on decision (Option A vs B)

Each example needs: `mapping.yaml`, `README.md`, catalog entry in `examples/README.md`,
and must pass `cargo test`.
