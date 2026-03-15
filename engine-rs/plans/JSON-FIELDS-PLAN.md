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

## Proposed Design

### Option A: Dot-notation in source field names (recommended)

Allow `source: column.key` syntax in field mappings to access JSON sub-fields:

```yaml
fields:
  - source: metadata.tier
    target: tier
  - source: metadata.language
    target: language
```

**Forward mapping**: detect the dot, generate `src."metadata"->>'tier'` instead
of `src."tier"`.

**Reverse mapping**: group all fields that share the same JSON column prefix
(`metadata.tier`, `metadata.language` → `metadata`), and rebuild the column:
```sql
jsonb_build_object('tier', resolved_tier, 'language', resolved_language) AS metadata
```

**`_base` storage**: store each sub-field individually:
```sql
jsonb_build_object('metadata.tier', src."metadata"->>'tier', 'metadata.language', src."metadata"->>'language')
```

**Noop detection**: compare per sub-field, same as regular fields:
```sql
_base->>'metadata.tier' IS NOT DISTINCT FROM resolved_tier::text
```

**Pros**:
- Minimal schema change — no new properties, just a naming convention
- Consistent with how `parent_fields` uses `alias: source_field` (dot = path)
- Each sub-field participates in resolution independently (correct behavior)
- AI agents and humans can read it naturally

**Cons**:
- Ambiguous if a source table has a column literally named `metadata.tier`
  (unlikely in practice, but possible)
- Only supports one level of JSON nesting (`metadata.tier` but not
  `metadata.address.city`) without further extension
- Reverse mapping must group by column prefix and generate `jsonb_build_object`

### Option B: Explicit `json_path` property

```yaml
fields:
  - source: tier
    target: tier
    json_path: metadata.tier
```

A new property explicitly declares the JSON source location. `source` becomes
a logical name for the extracted value.

**Pros**: No ambiguity with literal column names. Clear separation.
**Cons**: More verbose. New schema property. `source` field becomes misleading
  (it's not the actual column name).

### Option C: `source_expression` as the escape hatch

```yaml
fields:
  - source: metadata_tier   # logical name for _base
    target: tier
    expression: "metadata->>'tier'"
    reverse_expression: "jsonb_build_object('tier', tier)"
```

No schema changes. Users write expressions.

**Pros**: Works today. No engine changes needed.
**Cons**: Verbose. Error-prone. Reverse expression must manually reconstruct the
  JSON column. No automatic noop detection or grouping. Not AI-friendly.

## Recommendation

**Option A (dot-notation)** for the common case, with Option C as the existing
escape hatch for complex cases.

## Implementation Plan

### Phase 1: Forward mapping (read from JSON)

1. **Detect dot-notation in `source`**: In `render_forward_body`, check if a
   field mapping's `source` contains a dot. Split on the first dot:
   `metadata.tier` → column `metadata`, key `tier`.

2. **Generate JSON extraction**: Instead of `src."metadata.tier"`, generate
   `src."metadata"->>'tier'`. For deeply nested paths (`metadata.address.city`),
   chain operators: `src."metadata"->'address'->>'city'`.

3. **`_base` uses the dotted name**: Store as
   `'metadata.tier', src."metadata"->>'tier'` in `jsonb_build_object`.

### Phase 2: Reverse mapping (write back to JSON)

4. **Group reverse fields by column prefix**: When generating the reverse view,
   identify all fields that share a JSON column prefix. Instead of projecting
   them as separate columns, generate:
   ```sql
   jsonb_build_object(
     'tier', resolved_tier,
     'language', resolved_language
   ) AS metadata
   ```

5. **Handle mixed columns**: A source might have both regular columns (`name`)
   and JSON sub-fields (`metadata.tier`). Regular columns project normally.
   JSON-grouped columns produce a single `jsonb_build_object`.

### Phase 3: Delta / noop detection

6. **Per-sub-field comparison**: Noop detection already compares `_base->>'field'`
   with the reverse-mapped value. Since `_base` stores `metadata.tier` as a key,
   the comparison works: `_base->>'metadata.tier' IS NOT DISTINCT FROM tier::text`.

7. **Delta output**: The delta view should output the reconstructed JSON column
   (`metadata`), not individual sub-fields, since the ETL writes back to the
   source table which has a single `metadata` column.

### Phase 4: Validation

8. **Schema validation**: Ensure dotted source names are valid (at least 2 segments,
   no empty segments). Warn if the same JSON column prefix is used in some fields
   with dots and in other fields without (could be intentional but suspicious).

9. **Test updates**: Add a new example (`json-fields/` or extend `types/`) showing
   JSON sub-field extraction and reverse reconstruction.

## Open Questions

1. **Deep JSON paths**: Should `metadata.address.city` be supported in Phase 1,
   or only single-level (`metadata.tier`)? Single-level covers 90% of use cases
   and is simpler to implement. Deep paths can be added later.

2. **JSON arrays inside objects**: `metadata.tags` is a JSON array, not a scalar.
   Should dot-notation support extracting arrays? This overlaps with `source.path`
   but for arrays inside a JSON object rather than a JSONB column. Defer unless
   there's a concrete use case.

3. **Partial reconstruction**: If `metadata` has 5 sub-fields but only 2 are
   mapped, the reverse `jsonb_build_object` only includes the 2 mapped ones —
   the other 3 are lost. Options:
   - Accept the loss (mapped fields are the contract)
   - Merge with original: `original_metadata || jsonb_build_object('tier', ...)` —
     this preserves unmapped fields but adds complexity
   - Require all sub-fields to be mapped (too restrictive)

   Recommendation: Phase 1 uses simple reconstruction (mapped fields only).
   Phase 2 adds `json_preserve: true` option on the mapping to merge with the
   original value.

4. **Ambiguity with literal column names**: If a table has a column literally named
   `metadata.tier` (some databases allow this with quoting), the dot-notation
   would misinterpret it. The escape hatch: use an expression instead. This is
   an edge case not worth optimizing for.

## Estimated Scope

- Phase 1 (forward): ~40 lines in forward.rs
- Phase 2 (reverse): ~60 lines in reverse.rs (grouping + jsonb_build_object)
- Phase 3 (delta): ~20 lines in delta.rs (output grouped columns)
- Phase 4 (validation + tests): ~30 lines + 1 new example
- Total: ~150 lines of code + 1 example
