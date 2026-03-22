# Required Unique Priority for Coalesce Strategy

## Problem

When multiple mappings contribute to the same `strategy: coalesce` field, the
winner is determined by priority (lower number = higher precedence). Currently,
if no priority is set the engine falls back to `999` for all mappings — making
the result non-deterministic.

The validator emits a **warning** when a coalesce field has no priority, but
allows it. Examples like `composite-keys` and `element-hard-delete` use
coalesce without explicit priorities.

### Why this matters

- **Non-determinism**: Without priorities, `COALESCE(x, 999)` gives all mappings
  the same rank. `array_agg(...)[1]` then picks an arbitrary non-null value.
  The result can change between runs or PostgreSQL versions.
- **Duplicate priorities**: If two mappings share the same priority number for
  the same field, the tie-break is again non-deterministic.

## Proposed Change

**Upgrade the warning to a validation error** when `strategy: coalesce` has
multiple contributing mappings and:
1. Any contributing mapping lacks a priority (field-level or mapping-level), OR
2. Two contributing mappings have the same effective priority for the same field

### Effective priority

The effective priority for a mapping's contribution to a coalesce field is:
```
field.priority ?? mapping.priority ?? <missing>
```

### Validation rules

```
error: Mapping 'X' → 'target.field': coalesce with multiple contributors
       requires priority (field or mapping level)

error: Mappings 'X' and 'Y' → 'target.field': duplicate coalesce priority N
```

## Implementation

### Files to Modify

| File | Change |
|------|--------|
| `src/validate.rs` | Upgrade warning to error for missing priority; add duplicate check |

### Code Change (validate.rs ~line 589)

Replace the current `result.warning(...)` with `result.error(...)`.

Then add duplicate detection after the loop:
```rust
// Collect effective priorities and check for duplicates.
let mut priorities: Vec<(i64, &str)> = Vec::new();
for c in contribs {
    let eff = c.field_priority.or(c.mapping_priority);
    match eff {
        Some(p) => priorities.push((p, c.mapping_name)),
        None => {
            result.error("Strategy", format!(
                "Mapping '{}' → '{tname}.{fname}': coalesce with multiple \
                 contributors requires priority", c.mapping_name
            ));
        }
    }
}
priorities.sort_by_key(|(p, _)| *p);
for w in priorities.windows(2) {
    if w[0].0 == w[1].0 {
        result.error("Strategy", format!(
            "Mappings '{}' and '{}' → '{tname}.{fname}': duplicate \
             coalesce priority {}", w[0].1, w[1].1, w[0].0
        ));
    }
}
```

### Example Updates Required

These examples will need explicit priorities added:

| Example | Field(s) | Fix |
|---------|----------|-----|
| `composite-keys` | `order_date`, `customer_name`, `order_ref`, `line_number`, `product_name`, `quantity`, `unit_price` | Add `priority:` to each ERP/CRM mapping field |
| `element-hard-delete` | `cuisine`, `step_order`, `duration` | Add `priority:` to `recipe_db_*` and `blog_cms_*` |
| `embedded-vs-many-to-many` | `customer_id`, `contact_id`, `relation_type` | Add `priority:` to the two customer_contact mappings |

Other examples already set priorities or use `last_modified` strategy.

## Scope

The check applies ONLY when `contrib_count > 1` — single-contributor coalesce
fields (common for child/embedded mappings) don't need priority since there's
no ambiguity.

## Migration Impact

Pre-1.0 — no backwards compatibility needed. Users with multi-contributor
coalesce fields that lack priorities get a validation error and must add them.
