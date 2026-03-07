# Pattern: Derived Fields and Resolution Groups

When different sources store the same concept in different shapes, the target model needs a strategy to reconcile them. This document walks through the name derivation pattern as a concrete example, discusses where the derivation logic should live, and explains how resolution groups keep related fields consistent.

## The Problem

Two sources describe the same person:

| Source | Fields | Example |
|--------|--------|---------|
| ERP | `first_name`, `last_name` | "Alice", "Smith" |
| CRM | `customer_name` (full name) | "Alice Jones" |

The canonical person model needs all three: `first_name`, `last_name`, and `full_name`. But each source only provides a subset — the missing values must be derived.

## Solution: OSI Model Expressions + Resolution Groups

### 1. Target model declares derivation on each field

```yaml
# Acme person model
- name: first_name
  expression:
    dialects:
      - dialect: ANSI_SQL
        expression: "COALESCE(first_name, SPLIT_PART(full_name, ' ', 1))"

- name: last_name
  expression:
    dialects:
      - dialect: ANSI_SQL
        expression: "COALESCE(last_name, SPLIT_PART(full_name, ' ', 2))"

- name: full_name
  expression:
    dialects:
      - dialect: ANSI_SQL
        expression: "COALESCE(full_name, first_name || ' ' || last_name)"
```

Each field uses `COALESCE` — if a direct value exists, use it; otherwise derive from the related fields. The derivation logic lives in the OSI model, defined once.

### 2. Each source maps only what it genuinely has

```yaml
# ERP mapping — has first + last
- target_field: first_name
  timestamp_field: modified_at
  forward_expression: first_name
- target_field: last_name
  timestamp_field: modified_at
  forward_expression: last_name

# CRM mapping — has full name only
- target_field: full_name
  timestamp_field: last_updated
  forward_expression: customer_name
```

ERP maps `first_name` and `last_name`. CRM maps `full_name`. Neither pretends to have data it doesn't.

### 3. Resolution group ensures atomicity

```yaml
# In person_resolution
groups:
  name:
    fields: [first_name, last_name, full_name]

fields:
  first_name:
    strategy: { type: LAST_MODIFIED }
  last_name:
    strategy: { type: LAST_MODIFIED }
  full_name:
    strategy: { type: LAST_MODIFIED }
```

The `name` group ensures all three fields resolve from the **same winning source**. Without the group, `first_name` and `full_name` could come from different sources, producing "Alice Jones" as `full_name` but "Alice Smith" as `first_name` + `last_name` — an inconsistent state.

### 4. The derivation flow

**If ERP wins** (newer timestamp across any name field):
- `first_name = "Alice"` (direct from ERP)
- `last_name = "Smith"` (direct from ERP)
- `full_name = COALESCE(NULL, "Alice" || ' ' || "Smith")` → `"Alice Smith"`

**If CRM wins:**
- `full_name = "Alice Jones"` (direct from CRM)
- `first_name = COALESCE(NULL, SPLIT_PART("Alice Jones", ' ', 1))` → `"Alice"`
- `last_name = COALESCE(NULL, SPLIT_PART("Alice Jones", ' ', 2))` → `"Jones"`

## Alternative: Derivation in the Mapping Layer

Instead of OSI model expressions, CRM could split the name in its mapping:

```yaml
# CRM mapping — splits into three target fields
- target_field: full_name
  forward_expression: customer_name
- target_field: first_name
  forward_expression: "SPLIT_PART(customer_name, ' ', 1)"
- target_field: last_name
  forward_expression: "SPLIT_PART(customer_name, ' ', 2)"
```

The Acme model fields would then be simple passthroughs.

### Comparison

| | Model expressions (chosen) | Mapping-layer split |
|---|---|---|
| **DRY** | Derivation defined once in the model | Repeated in every mapping that provides a full name |
| **Semantic honesty** | CRM maps what it actually has — a full name | CRM pretends to know the first and last name |
| **Adding a new source** | Maps to `full_name`; derivation is automatic | Must duplicate the split logic |
| **Source-specific formats** | Doesn't help if CRM stores "Last, First" | Natural home for source-specific parsing |
| **Resolution groups** | Clean — each source contributes only fields it has | CRM contributes all 3 fields with synthetic timestamps |
| **Reverse direction** | Implicit via OSI expression | CRM reverse for first/last must reconstruct the full name |
| **Testability** | Model expressions testable in isolation | Split logic buried in each mapping |

### When to use which

**Use model expressions when** the derivation is universal — the same logic applies regardless of which source provides the data. "Split on space" is the same for every source that has a full name.

**Use mapping expressions when** the derivation is source-specific — e.g., CRM stores "Last, First" while HR stores "First Last". The split logic genuinely differs per source and belongs in the mapping.

## Generalization

This pattern applies to any case where:

1. Sources provide **overlapping but different shapes** of the same concept
2. The target model needs a **complete representation** with derivable fields
3. A **resolution group** ensures atomicity across the related fields

Other examples:

| Concept | Source A | Source B | Derived |
|---------|----------|----------|---------|
| Address | street, city, postal_code | single `address_line` | Parse/compose |
| Currency | `amount` in local currency, `currency_code` | `amount_usd` | Convert via rate |
| Date/time | `date` + `time` separate fields | `datetime` combined | Combine/split |

In each case: model expressions define the universal derivation, mappings contribute only what they genuinely have, and resolution groups ensure consistency.
