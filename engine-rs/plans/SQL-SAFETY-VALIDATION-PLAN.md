# SQL safety validation

**Status:** Proposed

**Goal:** The engine must never produce invalid SQL from a valid-looking mapping.
Every naming conflict or identifier collision should be caught at validation time
with a clear error, not at SQL execution time.

## Root cause

The `Analytics(name)` view uses the **bare target name** (`CREATE OR REPLACE VIEW "person" AS ...`).
This means user-chosen target names occupy the same SQL namespace as user-chosen
source table names.  No validation currently checks for collisions between these
namespaces, nor between user names and the engine's internal `_fwd_`, `_id_`,
`_rev_`, `_delta_`, `_resolved_`, `_ordered_` prefixed names.

## Proposed validation passes

### 1. `pass_name_collisions` — SQL namespace collision detection

All of these identifiers end up as top-level SQL objects (tables or views):

| Origin | SQL name pattern | Type |
|--------|-----------------|------|
| Source dataset | `{source_name}` | TABLE (external) |
| Mapping name | `_fwd_{mapping_name}` | VIEW |
| Target name | `{target_name}` (analytics view) | VIEW |
| Target name | `_id_{target_name}` | VIEW |
| Target name | `_resolved_{target_name}` | VIEW |
| Target name | `_ordered_{target_name}` | VIEW |
| Mapping name | `_rev_{mapping_name}` | VIEW |
| Mapping name | `_delta_{mapping_name}` | VIEW |
| cluster_members | `{table}` | TABLE |
| written_state | `{table}` | TABLE |

**Checks needed:**

- **Source name = target name** — direct collision between source table and
  analytics view (e.g. source `person` and target `person`).
  This is the bug that triggered this plan.

- **Source name = internal view name** — e.g. source `_fwd_orders` collides
  with the forward view of mapping `orders`.

- **Mapping name = another mapping name** — already checked by `pass_unique_names`,
  but verify it covers parent-only mappings too.

- **Target name = another target name** — already structurally impossible (map keys),
  but internal-prefix collisions are possible: target `_id_foo` would collide with
  the identity view of target `foo`.

- **Collect all final SQL object names, error if any duplicates.**

### 2. `pass_reserved_prefixes` — reject user names starting with `_`

The engine reserves identifiers starting with `_` for internal views
(`_fwd_`, `_id_`, `_rev_`, `_delta_`, `_resolved_`, `_ordered_`) and internal
columns (`_cluster_id`, `_action`, `_base`, `_row_id`, `_mapping`, `_src_id`,
`_priority`, `_last_modified`, `_ts_*`, `_priority_*`).

Any user-provided name (source, target, mapping, field) that starts with `_`
risks colliding with internal identifiers.

**Option A (strict):** Error on any name starting with `_`.
**Option B (safe):** Error only on names matching the known prefix patterns.

Recommend **Option A** — it's simple, easy to explain, and forward-compatible
with new internal prefixes.  Since the project is pre-1.0, this is a free rename.

### 3. `pass_source_field_columns` — reserved column names

Source columns named `_row_id`, `_action`, `_base`, `_cluster_id`, `_json`,
`_mapping`, `_src_id` would collide with internal SQL columns used in views.

Validate that no `source:` field name, `primary_key` column, `last_modified`
column, or `passthrough` column uses a reserved internal name.

### 4. `pass_sql_identifier_length` — PostgreSQL 63-byte limit

PostgreSQL silently truncates identifiers longer than 63 bytes (NAMEDATALEN - 1).
Two different long names could truncate to the same value.

Validate that all generated SQL identifiers (especially the prefixed ones like
`_fwd_{mapping_name}`) are ≤ 63 bytes.

## Implementation priority

| # | Pass | Severity | Effort |
|---|------|----------|--------|
| 1 | `pass_name_collisions` | **High** — produces invalid SQL | Medium |
| 2 | `pass_reserved_prefixes` | **High** — produces invalid SQL | Small |
| 3 | `pass_source_field_columns` | **Medium** — produces wrong results | Small |
| 4 | `pass_sql_identifier_length` | **Low** — edge case | Small |

## Test harness fix (done)

The integration test `load_test_data` now does `DROP VIEW IF EXISTS ... CASCADE`
before `DROP TABLE IF EXISTS ... CASCADE`, so stale views from previous examples
no longer block table creation. This is a band-aid — the real fix is validation
pass #1 to prevent the collision at definition time.
