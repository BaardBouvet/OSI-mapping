# Source column types

**Status:** Done

## Problem

Forward views cast all field expressions to `::text` for UNION ALL
compatibility (line ~99 of `forward.rs`):

```sql
SELECT person_id::text AS email, ...
UNION ALL
SELECT user_id::text AS email, ...
```

This works but loses type information. Consequences:
- Numeric comparisons in resolution become lexicographic (`"9" > "10"`)
- Date expressions need explicit casting in expressions
- `_base` comparisons in noop detection compare text representations
- Downstream analytics views expose text instead of native types

## Proposed Solution

### Schema Change

Add optional `type:` on source field definitions:

```yaml
sources:
  erp_orders:
    primary_key: order_id
    columns:
      order_id: text          # shorthand: just the type
      line_no: integer
      order_date: date
      amount:
        type: numeric(12,2)   # object form for precision
```

Or type hints on field mappings:

```yaml
fields:
  - source: line_no
    target: line_number
    type: integer             # ← forward view uses this type instead of ::text
```

### Rendering Change

In `forward.rs`, replace:
```rust
cols.push(format!("{expr}::text AS {fname}"));
```

With:
```rust
let cast = field_type.unwrap_or("text");
cols.push(format!("{expr}::{cast} AS {fname}"));
```

All mappings to the same target field must agree on the type. If they
disagree, emit a validation warning and fall back to `::text`.

### Model Change

Add to `FieldMapping`:
```rust
#[serde(default)]
pub sql_type: Option<String>,
```

Or add to `SourceMeta`:
```rust
#[serde(default)]
pub columns: IndexMap<String, ColumnDef>,
```

### Validation

- Warn when two mappings to the same target cast to different types
- Error when a declared type is not a valid PostgreSQL type name

### Migration

- No breaking change: `::text` remains the default
- Examples can gradually add type annotations
- Test harness `infer_column_types()` already infers types from JSON values —
  this could be used to auto-detect types from test data

## Recommendation

1. Add `sql_type: Option<String>` to `FieldMapping` (simplest, field-level)
2. Use it in `forward.rs` for casting
3. Add validation for cross-mapping type consistency
4. Default remains `::text` for backward compatibility
