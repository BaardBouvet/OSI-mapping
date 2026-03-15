# JSON Field Support Plan

## Problem Statement

Some source systems store structured data as a JSON column. For example:

```sql
-- Source table
CREATE TABLE crm_contacts (
  id TEXT PRIMARY KEY,
  name TEXT,
  metadata JSONB  -- {"preferred_language": "en", "tier": "gold", "tags": ["vip"]}
);
```

Today the engine has no way to:
1. **Read sub-fields** from a JSON column (`metadata->>'tier'`)
2. **Write sub-fields back** into a JSON column during reverse mapping
3. **Detect changes** at the sub-field level (noop detection compares the entire
   stringified JSON blob)

The `source.path` mechanism exists but is designed for **arrays of objects**
(`jsonb_array_elements`), not for accessing named sub-fields from a single JSON
object.

## Use Cases

### Case 1: Flat extraction — JSON sub-fields map to target fields

```yaml
# Source has: metadata JSONB = {"tier": "gold", "language": "en"}
# Target has: tier (coalesce), language (coalesce)
mappings:
  - name: crm
    source: { dataset: crm }
    target: customer
    fields:
      - source: metadata.tier        # <-- JSON path
        target: tier
      - source: metadata.language
        target: language
```

Forward: `src."metadata"->>'tier'` extracts the value.
Reverse: rebuilds `metadata` as `jsonb_build_object('tier', resolved_tier, 'language', resolved_language)`.

### Case 2: JSON-to-JSON — structured source maps to structured target

```yaml
# Source A has: config JSONB = {"theme": "dark", "locale": "nb-NO"}
# Source B has: preferences JSONB = {"display_theme": "light", "lang": "en"}
# Target has: preferences (coalesce, type: jsonb)
mappings:
  - name: source_a
    fields:
      - source: config
        target: preferences
        expression: "jsonb_build_object('theme', config->>'theme', 'locale', config->>'locale')"
```

This already works today via expressions — no new features needed. The user writes
the JSON construction SQL. The engine treats it as an opaque value.

### Case 3: Preserve-and-patch — resolve some sub-fields, keep the rest

```yaml
# Source has: settings JSONB = {"a": 1, "b": 2, "c": 3}
# Only "a" and "b" participate in resolution. "c" is passthrough.
```

This is the hardest case: the engine needs to know which sub-fields to resolve
and which to pass through unchanged.

## Design Decision: `source_path` Property

After evaluating three options (dot-notation in `source`, separate `source_path`
property, expression escape hatch), we chose **`source_path`** — a new property
on field mappings, mutually exclusive with `source`.

```yaml
fields:
  # Regular column
  - source: name
    target: name

  # JSON sub-field extraction (single level)
  - source_path: metadata.tier
    target: tier

  # Deep JSON path
  - source_path: metadata.address.city
    target: city

  # JSON array inside object
  - source_path: metadata.tags
    target: tags
```

### Semantics

- **`source_path`**: dotted path where the first segment is the JSONB column and
  remaining segments navigate into the JSON structure.
- **`source`** and **`source_path`** are mutually exclusive. `source_path` must
  contain at least one dot.
- The full dotted path is the field's **logical source identity** — used as the
  `_base` key and reverse view column alias.
- The first segment is the **physical source column** — used for input table DDL
  (typed JSONB) and reverse reconstruction grouping.

### Why `source_path` over dot-notation in `source`

| Criterion | `source: metadata.tier` | `source_path: metadata.tier` |
|---|---|---|
| Ambiguity | Breaks if column has literal dot | Zero — `source` is always a column |
| AI failure mode | Silent misinterpretation | Forgot property → reads whole column (safe) |
| Schema validation | Convention-based | Structural |
| Deep paths | Same | Same |

### Pipeline Mechanics

**Forward view**: `source_path: metadata.tier` generates:
```sql
"metadata"->>'tier' AS "tier"   -- single key
```
`source_path: metadata.address.city` generates:
```sql
"metadata"->'address'->>'city' AS "city"   -- chained navigation
```
The last segment uses `->>'` (text extraction); intermediate segments use `->'`
(JSONB navigation).

**`_base` storage**: each sub-field stored individually:
```sql
jsonb_build_object('metadata.tier', "metadata"->>'tier', ...)
```

**Reverse view**: output column aliased to the full dotted path:
```sql
r."tier" AS "metadata.tier"
```
This keeps noop detection simple — `_base->>'metadata.tier'` compares directly.

**Delta noop**: standard per-field comparison works unchanged:
```sql
_base->>'metadata.tier' IS NOT DISTINCT FROM "metadata.tier"::text
```

**Delta output**: groups dotted columns by root segment and reconstructs JSONB:
```sql
jsonb_build_object(
  'tier', "metadata.tier",
  'address', jsonb_build_object(
    'city', "metadata.address.city",
    'zip', "metadata.address.zip"
  )
) AS "metadata"
```

**Input table DDL**: root columns from `source_path` are typed JSONB (not TEXT).

### Partial Reconstruction

If the source JSON has 5 sub-fields but only 2 are mapped, the reverse
`jsonb_build_object` includes only the 2 mapped ones — unmapped sub-fields are
lost in the reverse direction. This is acceptable: mapped fields define the
contract. A future `json_preserve: true` option could merge with the original.

## Implementation

### Changes

**model.rs**: Add `source_path: Option<String>` to `FieldMapping`. Add helpers:
- `source_name()` → full dotted path (source_path) or source column name
- `source_column()` → physical column (first segment of source_path, or source)
- Update `effective_direction()` to treat source_path like source

**forward.rs**: Add `json_path_expr()` helper. When `source_path` is set, use
it for both the field extraction SQL and the `_base` key.

**reverse.rs**: Use `source_name()` for the output column alias.

**delta.rs**: Use `source_name()` for noop detection. In delta output, detect
`source_path` fields, group by root column, and emit `jsonb_build_object()`
trees instead of individual dotted columns.

**mod.rs** (`render_input_table`): Use `source_column()` for column collection;
mark source_path root columns as JSONB.

**validate.rs**: `source_path` must contain a dot. `source` and `source_path`
are mutually exclusive.

**mapping-schema.json**: Add `source_path` to FieldMapping properties.

**New example**: `json-fields/` demonstrating single-level and deep JSON paths.
