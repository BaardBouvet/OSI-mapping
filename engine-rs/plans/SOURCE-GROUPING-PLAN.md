# Source grouping

**Status:** Design

## Problem

Some mapping files have multiple source datasets that belong to the same logical
system. In the `nested-arrays` example:

```yaml
sources:
  shop:
    primary_key: order_id
  warehouse_lines:
    primary_key: line_id
  warehouse_orders:
    primary_key: id
```

`warehouse_lines` and `warehouse_orders` are both "the warehouse system."
There's no way to express this grouping today. In the DOT graph, all three
source nodes appear as siblings with no visual relationship.

## Use Case

Visual grouping is useful for:
- **DOT/GraphViz output**: `subgraph cluster_warehouse { ... }` draws a box
  around related source nodes
- **Documentation**: makes the mapping file self-documenting about system
  boundaries
- **AI agents**: knowing which datasets belong together helps when generating
  or reviewing mapping files

## Proposed Schema Change

Add an optional `system` property to `SourceMeta`:

```yaml
sources:
  shop:
    primary_key: order_id
    system: shop

  warehouse_lines:
    primary_key: line_id
    system: warehouse

  warehouse_orders:
    primary_key: id
    system: warehouse
```

When omitted, the source is its own system (no grouping). When present, sources
sharing the same `system` value are visually grouped.

### Alternatives Considered

**Option A: `system` on SourceMeta** (recommended above)
- Flat, simple, one extra optional property
- Familiar naming (most teams call these "systems")

**Option B: Top-level `systems` section with nested sources**
```yaml
systems:
  warehouse:
    sources:
      warehouse_lines: { primary_key: line_id }
      warehouse_orders: { primary_key: id }
  shop:
    sources:
      shop: { primary_key: order_id }
```
- More structured but requires restructuring the `sources` section
- Breaking schema change — existing files would need migration
- Over-engineered for a visual-only feature

**Option C: `group` on SourceMeta**
- Same as Option A but different name
- `group` already used in target field context (atomic resolution groups)
- Could be confusing

**Recommendation**: Option A (`system` property). Minimal schema change,
backward-compatible, clear semantics.

## Concerns

**Does it add confusion?** The property is optional and purely visual. It has
no effect on identity linking, resolution, or delta generation. Sources within
the same system are NOT automatically related — they still need separate
primary keys, separate mappings, and separate delta views.

The risk is that users might assume `system` implies shared identity or
automatic cross-dataset linking. The documentation should be clear:
> `system` is a visual grouping label. It has no effect on the engine pipeline.
> Datasets within the same system are still independent sources.

**When does it hurt?** If the mapping only has 2-3 sources, grouping adds
noise. The feature is most valuable with 4+ sources from 2+ systems. Making it
optional means simple files stay simple.

## Implementation

### Phase 1: Schema + Model

1. **spec/mapping-schema.json**: Add `system` to `SourceMeta`:
   ```json
   "system": {
     "type": "string",
     "description": "Visual grouping label for this source dataset. Sources sharing the same system are rendered as a group in DOT output. Has no effect on the engine pipeline."
   }
   ```

2. **model.rs**: Add `pub system: Option<String>` to `Source` struct.

### Phase 2: DOT Rendering

3. **dag.rs / `to_dot()`**: When rendering DOT output, collect sources by
   their `system` value. For each group with 2+ members, emit a
   `subgraph cluster_{system}` block:

   ```dot
   subgraph cluster_warehouse {
     label="warehouse";
     style=dashed;
     color=gray;
     "warehouse_lines" [label="SRC: warehouse_lines" shape=cylinder];
     "warehouse_orders" [label="SRC: warehouse_orders" shape=cylinder];
   }
   ```

   Sources without a `system` (or sole members) render as top-level nodes
   as they do today.

   Note: `to_dot()` currently doesn't receive the `MappingDocument`, only the
   `ViewDag`. Options:
   - Pass the document (or just the source metadata) to `to_dot()`
   - Store system grouping in the `ViewDag` struct
   - Preferred: add `pub source_systems: BTreeMap<String, Vec<String>>` to
     `ViewDag`, populated in `build_dag()`

### Phase 3: Example + Docs

4. Update `nested-arrays/mapping.yaml` to use `system: warehouse` on both
   warehouse sources.

5. Add a brief note in `docs/schema-reference.md` under the Source section.

## Estimated Scope

- Schema: 4 lines (JSON Schema)
- Model: 2 lines (one field + serde)
- DAG: ~20 lines (collect groups, store in ViewDag)
- DOT: ~15 lines (subgraph rendering)
- Example: 2 lines
- Docs: 5 lines
- Total: ~50 lines
