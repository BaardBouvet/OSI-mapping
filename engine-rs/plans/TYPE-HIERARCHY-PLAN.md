# Type hierarchy

**Status:** Design

Support type hierarchies on target fields — when a resolved value is
`vip-customer`, consumers filtering for the broader category `customer`
should automatically include it.

## Problem

A `company` target has a `customer_type` field.  Different sources classify
entities at different granularity:

- CRM: `vip-customer`, `standard-customer`, `prospect`
- ERP: `customer`, `lead`

Implicit hierarchy:

```
customer
├── vip-customer
└── standard-customer
lead
└── prospect
```

When ERP's reverse mapping uses `reverse_filter: "customer_type = 'customer'"`,
it should receive companies typed as `customer`, `vip-customer`, **or**
`standard-customer`.

Today `reverse_filter` is a literal SQL WHERE clause with no knowledge of type
relationships.

## Patterns that work today

### Pattern A — Explicit IN-list

Spell out all subtypes in every reverse_filter:

```yaml
reverse_filter: "customer_type IN ('customer', 'vip-customer', 'standard-customer')"
```

Trade-offs: simple, no engine changes. The hierarchy is duplicated in every
mapping that filters by type — no single source of truth.

### Pattern B — Multi-valued type tracking

Like the `types` example — emit all roles via `string_agg`, filter via LIKE:

```yaml
# Target field
customer_type:
  strategy: expression
  expression: "string_agg(distinct customer_type, ',' order by customer_type)"

# Forward mapping
- target: customer_type
  expression: "'vip-customer'"
  direction: forward_only

# Reverse
reverse_filter: "customer_type LIKE '%customer%'"
```

Trade-offs: works for naming-convention hierarchies where subtypes always
contain the parent name. Fragile for arbitrary names (`LIKE '%lead%'` would
match `misleading`).

### Pattern C — Boolean category flags

Add one boolean field per category level, resolved with `bool_or`:

```yaml
# Target fields
is_customer:
  strategy: bool_or
  type: boolean
is_vip:
  strategy: bool_or
  type: boolean

# Forward mapping
- target: is_customer
  expression: "customer_type IN ('customer', 'vip-customer', 'standard-customer')"
  direction: forward_only
- target: is_vip
  expression: "customer_type = 'vip-customer'"
  direction: forward_only

# Reverse
reverse_filter: "is_customer"
```

Trade-offs: clean, type-safe, uses existing `bool_or`. Requires N boolean
fields for N categories — doesn't scale for deep hierarchies.

### Comparison

| Pattern | Source of truth | Scales | Engine changes |
|---------|----------------|--------|----------------|
| A: IN-list | Scattered in reverse_filters | ✗ | None |
| B: LIKE | Naming convention | ✗ | None |
| C: Bool flags | Forward expressions | ~10 types | None |

## Proposed: `hierarchy` on target fields

Add an optional `hierarchy:` property to target field definitions:

```yaml
targets:
  company:
    fields:
      customer_type:
        strategy: coalesce
        hierarchy:
          vip-customer: customer
          standard-customer: customer
          prospect: lead
```

Map from child → parent.  Multiple levels chain:
`specialist-vip: vip-customer` + `vip-customer: customer` → three-level
hierarchy.

### Resolution

Unchanged.  The winning type is the literal value (`vip-customer`).

### Reverse filter expansion

When generating the reverse view, the engine:

1. Builds the full descendant closure for each type.
2. Generates a CTE with the transitive closure.
3. Rewrites type equality checks in `reverse_filter` to include descendants.

Given `reverse_filter: "customer_type = 'customer'"`, the reverse view becomes:

```sql
WITH _hierarchy_customer_type(child, parent) AS (
  VALUES
    ('vip-customer',      'customer'),
    ('standard-customer', 'customer'),
    ('prospect',          'lead')
),
_closure_customer_type AS (
  SELECT child AS leaf, child AS ancestor
    FROM _hierarchy_customer_type
  UNION ALL
  SELECT c.leaf, h.parent
    FROM _closure_customer_type c
    JOIN _hierarchy_customer_type h ON h.child = c.ancestor
)

-- original:  WHERE customer_type = 'customer'
-- rewritten:
WHERE customer_type = 'customer'
   OR customer_type IN (
        SELECT leaf FROM _closure_customer_type
        WHERE ancestor = 'customer'
      )
```

### Alternative: helper function instead of rewrite

Rewriting arbitrary SQL is fragile. A safer approach: require the mapping author
to use a placeholder that the engine replaces:

```yaml
reverse_filter: "type_matches(customer_type, 'customer')"
```

The engine replaces `type_matches(field, value)` with the CTE-backed expansion.
Easier to implement, no SQL parsing, explicitly declares intent.

### Complexity estimate

- **Parser:** `hierarchy: HashMap<String, String>` on `TargetFieldDef` (~5 lines)
- **Validate:** cycle detection, check that field has `hierarchy` before
  using `type_matches` (~15 lines)
- **Reverse renderer:** generate closure CTE, replace `type_matches` calls (~40 lines)
- **Total:** ~60 lines

### Concerns

1. **Hierarchy vs. vocabulary:** Overlaps with vocabulary targets. A vocabulary
   with a `parent` column could express the same thing. `hierarchy:` is simpler
   for small, static type sets (5-20 values).

2. **Scope creep:** Large hierarchies belong in a database table, not YAML.
   This feature targets small, static taxonomies.

3. **Forward direction:** `filter: "customer_type = 'customer'"` could also
   benefit from hierarchy expansion (include source rows typed as subtypes).
   Same mechanism applies but doubles the surface area.

## Recommendation

Start with **Pattern A** or **Pattern C** for current mappings — they work today
and are explicit.  Consider the `hierarchy:` property (with `type_matches`
helper) when multiple mappings duplicate the same type hierarchy in their
reverse_filters.
