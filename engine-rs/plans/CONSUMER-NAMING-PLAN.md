# Consumer-facing naming consistency

**Status:** Planned

Rename two consumer-facing identifiers to follow the convention that
the `_` prefix means internal plumbing and consumer-facing objects use
unprefixed names. This applies to **view and table names**, not to
**column names** within those views — output columns keep their `_`
prefix to avoid collisions with user-defined source/target fields.

## Problem

The engine uses a `_` prefix convention for internal views: `_fwd_`,
`_id_`, `_resolved_`, `_ordered_`, `_rev_`. The analytics view correctly
breaks this convention — it uses the bare target name (`customer`, not
`_customer`) because it's consumer-facing.

Two other consumer-facing identifiers violate this pattern:

1. **`_delta_{source}`** — the view ETL pipelines query for sync actions.
   Uses the `_` prefix despite being a primary consumer touchpoint.

2. **`_cluster_members_{mapping}`** — the table the ETL writes insert
   feedback to. Uses the `_` prefix despite being an external table the
   mapping author declares and the ETL maintains.

## Changes

"Delta" is also a misnomer — the view emits the **desired state** per
source row, not a diff. An ETL reading it gets the full field values to
write, annotated with `_action`, not a before/after comparison.

### 1. Rename `_delta_{source}` → `sync_{source}`

"Sync" describes what the view is for: it tells the ETL what to
synchronise. The view contains every row the ETL needs to act on, with
the desired field values and an `_action` column.

The `_delta_` prefix in `validate_expr.rs` (reserved prefix list) stays
as-is until the old name is fully removed. During a transition period,
both could be reserved.

### 2. Rename `_cluster_members_{mapping}` → `cluster_members_{mapping}`

Drop the `_` prefix. The table name, property name (`cluster_members`),
and column names (`cluster_id`, `source_key`) stay the same — only the
default table name loses its leading underscore.

## Impact surface

### `_delta_` → `sync_`

| File | What to change |
|------|---------------|
| `dag.rs` | `ViewNode::Delta` view_name: `_delta_{name}` → `sync_{name}` |
| `dag.rs` | Comment on `Delta` variant |
| `dag.rs` | Label: `DELTA: {name}` → `SYNC: {name}` |
| `dag.rs` | Graphviz shape match arm (no change, just Delta variant) |
| `validate_expr.rs` | Add `"sync_"` to reserved prefix list (keep `"_delta_"` during transition) |
| `render/delta.rs` | View name construction: `format!("_delta_{source_name}")` → `format!("sync_{source_name}")` |
| `render/delta.rs` | SQL comment: `-- Delta:` → `-- Sync:` |
| `render/mod.rs` | Match arm `ViewNode::Delta` — annotation comment |
| `render/mod.rs` | Materialized view index references |
| `engine-rs/docs/design-decisions.md` | Pipeline diagram + decision text |
| `engine-rs/docs/README.md` | Pipeline diagram |
| `engine-rs/docs/view-pipeline.md` | View table + section heading |

Unit tests in `render/delta.rs` and `render/mod.rs` that assert on
`_delta_` strings need updating.

No examples reference `_delta_` directly — they use `expected.deltas`
in test YAML which maps to source names, not view names.

### `_cluster_members_` → `cluster_members_`

| File | What to change |
|------|---------------|
| `model.rs` | Default in `ClusterMembers::table_name`: `_cluster_members_{mapping_name}` → `cluster_members_{mapping_name}` |
| `model.rs` | Doc comment on `ClusterMembers.table` |
| `docs/reference/schema-reference.md` | Default table name in schema table |
| `docs/design/design-rationale.md` | Example default name |
| `docs/design/ai-guidelines.md` | Example comment |

## Migration

This is a breaking change for existing ETL pipelines. Options:

### Option A: hard rename (recommended)

Change all names at once. Document in release notes. Since the engine has
no published stable release yet, this is the cleanest path.

### Option B: transition period with aliases

Generate both old and new names temporarily:

```sql
CREATE OR REPLACE VIEW sync_crm AS SELECT ...;
CREATE OR REPLACE VIEW _delta_crm AS SELECT * FROM sync_crm;  -- compat
```

Adds complexity for marginal benefit. Only worthwhile if external users
are already referencing the old names in production.

## Recommendation

**Option A.** The project has no stable release and no published API
contract. Rename now while the cost is low. The change is mechanical —
string replacements in ~12 code locations and ~8 doc locations.

## What this plan does NOT change

- Internal view prefixes (`_fwd_`, `_id_`, `_resolved_`, `_ordered_`,
  `_rev_`) — these are plumbing, correctly prefixed.
- Internal column names (`_entity_id`, `_mapping`, `_priority`, etc.) —
  never consumer-facing.
- **Output column `_` prefixes** (`_action`, `_cluster_id`, `_base`) —
  these are consumer-facing but the `_` prefix serves a different purpose
  here: it distinguishes engine-generated columns from user-defined
  source/target fields in the same SELECT. Without the prefix, a source
  table with a column named `action` or `base` would collide. The `_`
  prefix on output columns is a namespace convention, not a visibility
  convention.
- `_src_id` — internal to the reverse view, not in sync output.
