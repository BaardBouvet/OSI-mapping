# Composite Key Reference Resolution Plan

## Problem

When a source PK column is also mapped to a reference field, the reverse view
emits the raw PK value instead of the resolved reference.

### Example

In the `composite-keys` example, `erp_order_lines` has:
```yaml
primary_key: [order_id, line_no]
fields:
  - source: order_id
    target: order_ref     # order_ref references: purchase_order
  - source: line_no
    target: line_number
```

The reverse view currently produces:
```sql
(id._src_id::jsonb->>'order_id') AS order_id   -- raw PK extraction
(id._src_id::jsonb->>'line_no') AS line_no       -- raw PK extraction
```

For **update** rows this is correct — the row exists, the PK is its own PK.

For **insert** rows (`_src_id IS NULL`), the PK columns are NULL because
there's no source row. The `order_id` should instead be resolved via the
reference to `purchase_order` — find the same-system mapping's `_src_id`.

## Current Behavior

`reverse_select_exprs` always emits `id._src_id::jsonb->>'col'` for each PK
column. The field loop then **skips** any source column that's in the PK set
(`pk_columns.contains(s.as_str())`). This means PK columns never get
reference resolution, even when their target field has `references:`.

## Proposed Fix

Change the reverse view to handle PK-mapped reference fields specially:

1. For each PK column that is ALSO mapped to a target field with `references:`:
   - For update rows (have `_src_id`): use the PK extraction (existing behavior)
   - For insert rows (`_src_id IS NULL`): use the reference resolution subquery

2. Implementation: wrap in a `COALESCE`:
   ```sql
   COALESCE(
     (id._src_id::jsonb->>'order_id'),     -- PK extraction (non-NULL for updates)
     (SELECT ref_local._src_id ...)          -- reference resolution (for inserts)
   ) AS order_id
   ```

3. Modify `render_reverse_view`:
   - After emitting PK columns via `reverse_select_exprs`, check if each PK
     column is mapped to a reference field.
   - If so, replace the simple extraction with the COALESCE pattern.
   - Non-reference PK columns keep the simple extraction.

## Scope

- Only affects composite keys where a PK component is also a reference field
- Single-key PKs might also need this if the PK is mapped to a reference field
  (less common but possible)
- Non-PK reference fields already work correctly

## Status: Done

Implemented via COALESCE wrapping in `pk_base_expr_map` + field loop in `reverse.rs`.
PK columns with reverse field mappings get `COALESCE(pk_extraction, field_expr)`
where `field_expr` is either a reference subquery or identity/resolved fallback.
Insert rows (where `_src_id IS NULL`) now resolve PK values through references
or identity, while update rows still use the PK extraction (first non-NULL wins).
