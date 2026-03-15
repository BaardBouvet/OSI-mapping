# Composite Types Plan

**Status:** Proposed  
**Goal:** Replace raw JSONB columns with PostgreSQL composite types for nested array data, giving consumers typed, navigable structures while keeping JSON for the internal wire format.

---

## Problem

Today nested arrays flow through the pipeline as JSONB:

1. **Source tables** store nested data in JSONB columns (e.g., `lines jsonb`).
2. **Forward views** explode them with `CROSS JOIN LATERAL jsonb_array_elements(lines) AS item`, extracting text via `item.value->>'field'`.
3. **Delta views** re-assemble child rows with `jsonb_build_object(...)` + `jsonb_agg(...)`, producing a JSONB array column on the parent delta view.

This works but has drawbacks:

- **No type safety** — consumers receive `jsonb` and must know the shape. Typo in a key name silently returns NULL.
- **No IDE autocomplete** — tools can't introspect JSONB structure.
- **Optimizer limitations** — PostgreSQL can't push predicates into JSONB arrays as effectively as into typed columns.
- **Type round-trip** — numbers and booleans stored in JSONB survive as their native JSON types, but extracting them back requires casts. This is the root cause of the nested typed noop detection bug (NESTED-TYPED-NOOP-PLAN).

## Proposed Design

### 1. Composite Type Hierarchy

For each target with nested array children, emit `CREATE TYPE` statements that mirror the hierarchy:

```sql
-- Leaf: order_line fields
CREATE TYPE _type_order_line AS (
    order_ref   text,
    line_number text,
    product     text,
    quantity    numeric
);

-- Parent: purchase_order with typed array of children
CREATE TYPE _type_purchase_order AS (
    order_ref   text,
    total       numeric,
    lines       _type_order_line[]
);
```

For deep nesting (3+ levels), types compose bottom-up:

```sql
CREATE TYPE _type_grandchild AS (name text);
CREATE TYPE _type_child AS (
    child_id text,
    grandchildren _type_grandchild[]
);
CREATE TYPE _type_parent AS (
    parent_id text,
    children _type_child[]
);
```

### 2. Where Types Are Used

| Pipeline stage | Current | With composite types |
|---|---|---|
| Source tables | JSONB column | JSONB column (unchanged) |
| Forward views | `jsonb_array_elements` → text | Same (unchanged) |
| Identity / Resolution | Flat rows | Same (unchanged) |
| Reverse views | Flat rows | Same (unchanged) |
| Delta views (nested re-assembly) | `jsonb_build_object` + `jsonb_agg` → jsonb | `ROW(...)::_type_X` + `array_agg` → `_type_X[]` |
| Analytics views | jsonb | Composite type columns |

The change is **localized to delta view re-assembly** (`render_delta_with_nested` in delta.rs) and analytics view rendering. Forward, identity, resolution, and reverse views remain unchanged — they operate on flat, exploded rows.

### 3. Delta View Re-Assembly (Changed)

Current leaf CTE:

```sql
_nest_lines AS (
    SELECT
        n."_parent_key",
        COALESCE(jsonb_agg(
            jsonb_build_object(
                'order_ref', n."order_ref",
                'line_number', n."line_number",
                'product', n."product",
                'quantity', n."quantity"
            ) ORDER BY n."line_number"
        ), '[]'::jsonb) AS "lines"
    FROM _delta_shop_lines n
    GROUP BY n."_parent_key"
)
```

Becomes:

```sql
_nest_lines AS (
    SELECT
        n."_parent_key",
        COALESCE(array_agg(
            ROW(
                n."order_ref",
                n."line_number",
                n."product",
                n."quantity"
            )::_type_order_line ORDER BY n."line_number"
        ), ARRAY[]::_type_order_line[]) AS "lines"
    FROM _delta_shop_lines n
    GROUP BY n."_parent_key"
)
```

### 4. JSON Kept Inside Composite Types

For deeply nested data or variable-shape sub-documents that don't map to a fixed target, keep JSON embedded inside the composite type:

```sql
CREATE TYPE _type_order_line AS (
    product     text,
    quantity    numeric,
    metadata    jsonb      -- unstructured sub-document
);
```

This gives the best of both worlds: typed navigation for the known structure, JSON for the flexible parts. The engine already supports this naturally — any field without a declared type defaults to `text`, and `jsonb` can be declared via `type: jsonb`.

### 5. Noop Detection Simplification

With composite types, the nested noop comparison changes from:

```sql
-- Current: JSONB text normalization
_osi_text_norm(p._base->'lines')::text IS NOT DISTINCT FROM _nest."lines"::text
```

To:

```sql
-- With composite types: direct array comparison
p._base_lines IS NOT DISTINCT FROM _nest."lines"
```

The `_base` column would store `_type_order_line[]` directly (or remain JSONB with a cast). This eliminates the text normalization roundtrip and the NESTED-TYPED-NOOP bug entirely.

However, `_base` is currently a single JSONB column. Two options:

**Option A — Typed `_base` columns:** Replace single `_base jsonb` with per-nested-array base columns (`_base_lines _type_order_line[]`). Clean comparison but changes the `_base` contract.

**Option B — Keep `_base` as JSONB, cast on comparison:** `_base->'lines' IS NOT DISTINCT FROM to_jsonb(_nest."lines")`. Preserves `_base` contract. Comparison still requires one side to be converted.

**Recommendation:** Option A for new deployments, with a config flag to fall back to JSONB `_base` for compatibility.

---

## Implementation

### Phase 1 — Type Generation (render/types.rs)

New render module that walks the target/mapping tree and emits `CREATE TYPE` statements in dependency order (leaves first):

```rust
pub fn render_composite_types(
    targets: &IndexMap<String, Target>,
    mappings: &[Mapping],
) -> Result<Vec<String>> { ... }
```

Output goes at the top of the generated SQL, before any views. Types use `CREATE TYPE IF NOT EXISTS` or `DROP TYPE ... CASCADE` + `CREATE TYPE` (needs careful ordering for migrations).

### Phase 2 — Delta CTE Refactor (render/delta.rs)

Modify `build_nested_ctes` to emit `ROW(...)::_type_X` + `array_agg` instead of `jsonb_build_object` + `jsonb_agg`. The structure of the CTE tree is unchanged — only the aggregation expressions change.

Keep JSONB fallback behind a `composite_types: bool` flag on RenderOptions for backward compatibility.

### Phase 3 — Analytics View (render/analytics.rs)

If an analytics view exists, it can expose composite-typed columns directly. Consumers use PostgreSQL's record access syntax:

```sql
SELECT
    (lines[1]).product,
    (lines[1]).quantity
FROM _analytics_purchase_order;
```

Or `unnest()`:

```sql
SELECT po.order_ref, l.*
FROM _analytics_purchase_order po,
     LATERAL unnest(po.lines) AS l;
```

### Phase 4 — Noop Detection Update

Implement Option A (typed `_base` columns) or Option B (JSONB cast) for the noop comparison. This subsumes NESTED-TYPED-NOOP-PLAN — the type normalization problem goes away with native typed storage.

---

## Migration & Compatibility

- **New mappings:** Use composite types by default.
- **Existing deployments:** JSONB output remains the default. Opt-in via `output.composite_types: true` in the YAML or CLI flag `--composite-types`.
- **Type evolution:** Adding a field to a composite type requires `ALTER TYPE ... ADD ATTRIBUTE`. Removing or renaming requires `DROP TYPE CASCADE` + recreate (cascades to dependent views, which are recreated anyway).
- **pg_dump compatibility:** Composite types are fully supported by pg_dump/pg_restore.

## Risks

1. **Type ordering in DROP/CREATE:** Composite types have dependencies. Must drop in reverse order and create in forward order. The DAG already handles view ordering; extend it for types.
2. **ALTER TYPE limitations:** PostgreSQL doesn't support all ALTER TYPE operations (e.g., can't reorder attributes). Full DROP + CREATE is safer but cascades to views.
3. **ORMs and client libraries:** Some ORMs may not handle composite array types well. JSONB has better cross-platform support. The fallback flag mitigates this.
4. **Performance:** `ROW()::type` + `array_agg` should perform comparably to `jsonb_build_object` + `jsonb_agg`. May be slightly faster due to no JSON serialization overhead.

## Scope

This plan only changes the **output representation** of nested data in delta/analytics views. The internal pipeline (forward → identity → resolution → reverse) is unaffected — those operate on flat, exploded rows regardless of the output format.
