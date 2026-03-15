# Typed Fields in Nested Array Noop Detection

## Problem

Adding `type: numeric` (or `type: boolean`) to target fields that participate in
nested array mappings (source.path) breaks noop detection. Every row is
classified as "update" even when nothing changed.

## Root Cause

The noop comparison for nested arrays currently works like this:

```sql
-- Left side: original source JSONB, normalized to all-text scalars
_osi_text_norm(p._base->'lines')::text
-- Right side: reconstructed from reverse view columns via jsonb_build_object
_nested_lines."lines"::text
```

The `_osi_text_norm()` function recursively converts every scalar to a text
string. This was designed to handle the case where the raw source JSONB has
native integers (`{"qty": 2}`) but the reverse view always carried text
(`"qty"` column is `text`), producing `{"qty": "2"}` on both sides.

When `type: numeric` is declared on a target field:

1. **Forward view**: `(item.value->>'qty')::numeric AS "qty"` — column is now numeric
2. **Pipeline**: numeric type flows through identity → resolution → reverse (SELECT *)
3. **Nested CTE**: `jsonb_build_object('qty', n."qty")` — builds `{"qty": 2}` (numeric JSONB)
4. **_base side**: `_osi_text_norm(...)` normalizes to `{"qty": "2"}` (text JSONB)
5. **Mismatch**: `{"qty": "2"} ≠ {"qty": 2}` → always classified as update

## Design Constraint

Simple (non-nested) fields don't have this problem. Their noop comparison is:

```sql
_base->>'field' IS NOT DISTINCT FROM "field"::text
```

Both sides extract/cast to text, so the comparison is type-agnostic.

## Solution: Also Normalize the Reconstructed Side

Apply `_osi_text_norm()` to both sides of the nested array comparison:

```sql
-- Before (current):
COALESCE(_osi_text_norm(p._base->'lines')::text, '[]')
IS NOT DISTINCT FROM
COALESCE(_nested_lines."lines"::text, '[]')

-- After:
COALESCE(_osi_text_norm(p._base->'lines')::text, '[]')
IS NOT DISTINCT FROM
COALESCE(_osi_text_norm(_nested_lines."lines")::text, '[]')
```

This normalizes both sides to all-text scalars, making the comparison
type-agnostic regardless of what types flow through the pipeline.

### Why This Works

- `_osi_text_norm` is already defined as `IMMUTABLE` — safe to call multiple
  times, deterministic
- Both sides are forced into the canonical text-only JSONB form
- No type information is lost in the delta output — the output columns still
  carry their declared types. The normalization is only used inside the CASE
  expression for action classification
- The function handles arrays, objects, and scalars recursively — works at
  any nesting depth

### Why Not Other Approaches

| Alternative | Drawback |
|---|---|
| Store typed values in `_base` | Requires changing forward view `_base` construction to cast each field to its target type. Complicates `_base` which currently uses raw source values. Would also need matching casts in non-nested noop detection. |
| Skip normalization, keep types on both sides | The raw source JSONB may have mixed types (some fields string, some int). Would need per-field type awareness in reconstruction CTE. Complex. |
| Cast reverse fields to text in the CTE | Loses the benefit of type declarations — the delta output would emit text even when a type was declared. |

## Implementation

### Changes

**File: `engine-rs/src/render/delta.rs`**

Single change in `render_delta_with_nested()`, in the nested array noop checks
loop (~line 464-469):

```rust
// Before:
noop_parts.push(format!(
    "COALESCE(_osi_text_norm(p._base->'{col}')::text, '[]') \
     IS NOT DISTINCT FROM COALESCE({alias}.{qcol}::text, '[]')",
    col = rr.column,
    alias = rr.alias,
));

// After:
noop_parts.push(format!(
    "COALESCE(_osi_text_norm(p._base->'{col}')::text, '[]') \
     IS NOT DISTINCT FROM COALESCE(_osi_text_norm({alias}.{qcol})::text, '[]')",
    col = rr.column,
    alias = rr.alias,
));
```

**File: example mapping.yaml files (nested arrays)**

After the fix, add `type: numeric` to numeric target fields:

- `nested-arrays`: `purchase_order.total`, `order_line.quantity`
- `nested-arrays-deep`: (no numeric value fields — child_id/grandchild_id are
  identity so stay as text for hash compatibility)
- `nested-arrays-multiple`: `department.budget`, `employee.salary`

Update expected data to use bare numbers instead of quoted strings.

### No Changes Needed

- `_osi_text_norm()` function — already handles all cases
- Forward view `_base` construction — stays as raw source JSONB
- Non-nested field noop comparison — already type-agnostic via `->>'` extraction
- Identity field handling — identity fields should remain untyped (text) for hash
  compatibility

## Testing

1. Add `type: numeric` to nested-arrays `total` and `quantity`
2. Change expected data from `"150"` → `150`, `"2"` → `2`, etc.
3. Verify noop round-trip test still passes (test 1)
4. Verify merge/update test still works with numeric values (test 2)
5. Repeat for nested-arrays-multiple (`budget`, `salary`)
6. Run full test suite — all 35 examples + 11 tests green

## Performance

`_osi_text_norm` is a PL/pgSQL function with recursive calls. Calling it on the
reconstructed side adds one extra function call per parent row. Since noop
detection is per-row (not per-item), the overhead is modest — one call with an
array of N items per parent, same cost as the existing left-side call.

For very large arrays, both sides are already paying this cost (one side
explicitly, the other via `::text` serialization). The extra `_osi_text_norm`
call replaces a cheaper `::text` cast, but correctness trumps marginal
performance.
