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

### Nested Array Interaction

`source_path` is context-aware. In a mapping with `source.path` (nested arrays),
all segments are treated as JSON keys under the nested item (`item.value`):

```yaml
# Source: data.items = [{"meta": {"tier": "gold"}, "qty": 2}]
- name: items
  source:
    dataset: source
    path: items
  target: line_item
  fields:
    - source_path: meta.tier    # → (item.value->'meta'->>'tier')
      target: tier
```

In root-level mappings (no `source.path`), the first segment is the JSONB column
name: `source_path: metadata.tier` → `"metadata"->>'tier'`.

### Extended Path Syntax: Brackets and Array Indices

`source_path` supports a subset of JSONPath-style bracket notation. Segments
are separated by `.` unless inside `[...]` brackets.

#### Segment types

| Syntax | Meaning | SQL |
|---|---|---|
| `.key` | Property access | `->>'key'` (leaf) or `->'key'` (intermediate) |
| `.['key.name']` | Property with dots/special chars | `->>'key.name'` |
| `[N]` | Array index (integer) | `->>N` (leaf) or `->N` (intermediate) |

Single quotes inside brackets are canonical JSONPath style. No shorthand forms.

#### Examples

```yaml
# Standard property access (unchanged):
- source_path: metadata.tier
# → "metadata"->>'tier'

# Dotted JSON key — use bracket notation:
- source_path: "config.['api.endpoint']"
# → "config"->>'api.endpoint'

# Deep path with dotted key:
- source_path: "config.['app.settings'].timeout"
# → "config"->'app.settings'->>'timeout'

# Array index — extract from a JSON array:
- source_path: contacts[0].email
# → "contacts"->0->>'email'

# Array index as leaf:
- source_path: tags[0]
# → "tags"->>0
```

#### Parsing algorithm

```
parse_source_path("config.['api.endpoint'].items[0].name")
→ segments: [
    PathSegment::Key("config"),
    PathSegment::Key("api.endpoint"),
    PathSegment::Key("items"),
    PathSegment::Index(0),
    PathSegment::Key("name"),
  ]
```

Split on `.` respecting `[...]` brackets. A `[N]` suffix on any segment is
split into its own `Index` segment. Bracket-quoted keys strip `['` and `']`.

#### SQL generation

For each segment, emit `->` (intermediate) or `->>` (leaf):
- `Key(k)` → `->>'k'` or `->'k'`
- `Index(n)` → `->>n` or `->n`

In root context (no `source.path`), the first segment is always a quoted
column name: `qi("config")` → `"config"`. In nested context (`source.path`),
the base is `item.value` and all segments are JSON navigation operators.

## Implementation

### Changes (Done)

**model.rs**: Added `source_path: Option<String>` to `FieldMapping`. Helpers:
- `source_name()` → full dotted path (source_path) or source column name
- `source_column()` → physical column (first segment of source_path, or source);
  strips bracket suffixes (e.g. `contacts[0].email` → `contacts`)
- Updated `effective_direction()` to treat source_path like source

**forward.rs**: Added `PathSegment` enum (`Key(String)`, `Index(i64)`) and
`parse_path_segments()` bracket-aware parser. `json_path_expr()` /
`json_path_expr_with_base()` use the parser to generate PostgreSQL JSONB
navigation. Context-aware: uses `item.value` base in nested array mappings,
quoted column in root mappings. Single quotes in `_base` keys are escaped via
`sql_escape()`.

**lib.rs**: Added `sql_escape()` helper for single-quote doubling in SQL literals.

**reverse.rs**: Uses `source_name()` for the output column alias.

**delta.rs**: Uses `source_name()` for noop detection with `sql_escape()` for
keys containing single quotes. `JsonNode` tree (with `Leaf`, `Object`, `Array`
variants) and `delta_output_exprs()` use `parse_path_segments()` for correct
grouping. Object paths reconstruct via `jsonb_build_object()`, array index
paths reconstruct via `jsonb_build_array()` (gaps filled with NULL). Array
index fields work bidirectionally.

**mod.rs** (`render_input_table`): Uses `source_column()` for column collection;
marks source_path root columns as JSONB.

**validate.rs**: `source_path` must navigate into a column (dot or bracket after
root). `source` and `source_path` are mutually exclusive.

**mapping-schema.json**: Added `source_path` with bracket-aware pattern.

**New example**: `json-fields/` — CRM JSONB metadata + flat ERP. Demonstrates:
single-level and deep paths, bracket-quoted keys (`['api.endpoint']`), array
index access (`contacts[0].phone` as forward_only), and priority-based merge.
