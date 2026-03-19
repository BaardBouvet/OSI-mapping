# Derived per-field timestamps from written state

**Status:** Done

When multi-source resolution uses `strategy: last_modified`, every source
must provide a timestamp column. But many source systems don't expose
per-field or per-row timestamps. Today these sources can only use
`strategy: coalesce` (priority-based), forfeiting recency-based resolution.

The `_written` JSONB already stores every field's last-written value.
By comparing each field's current source value against its written value,
the engine can detect **which fields changed** this sync cycle — and
derive per-field timestamps from the written state.

## Problem

### Sources without timestamps

Many source systems provide data snapshots (CSV exports, API dumps,
flat files) without meaningful `updated_at` columns. The data is "current
as of the last sync" but carries no intrinsic timestamp.

Today these sources cannot participate in `last_modified` resolution.
The mapping author is forced to pick a static priority order via
`strategy: coalesce`, even when time-based resolution would be more
appropriate.

### Written state already has per-field change detection

The `_written` JSONB records every field's value from the last ETL write.
The `_written_at` column records when that write happened. Together they
answer two questions per field:

1. **Did this field change?** Compare current source value against
   `_written->>'{field}'`.
2. **When did it last change?** If changed this cycle: `_written_at`.
   If unchanged: carry forward the previously derived timestamp.

This is exactly the per-field `_ts_{field}` that `last_modified`
resolution needs — without the source system providing any timestamps.

## Design

### Per-field timestamp derivation

The engine compares each mapped field against the written JSONB. For
each field, the derived timestamp is:

```sql
CASE
  WHEN src.email IS NOT DISTINCT FROM (_ws._written->>'email')
    THEN (_ws._written_ts->>'email')::timestamptz  -- unchanged: carry forward
  WHEN (_ws._written_ts->>'email') IS NOT NULL
    THEN _ws._written_at                           -- changed + have baseline: stamp
  ELSE NULL                                         -- changed + no baseline: bootstrap
END AS _ts_email
```

Three cases:
1. **Unchanged**: carry forward the existing per-field timestamp.
2. **Changed + have baseline**: stamp with `_written_at`. We have
   change history, so we know this is a real change.
3. **Changed + no baseline (bootstrap)**: NULL. First cycle — we
   have no change history, so resolution falls to coalesce order.

After the ETL writes back resolved timestamps into `_written_ts`,
subsequent cycles always hit case 1 or 2.

### Written table schema

The ETL maintains `_written_at` and `_written_ts` columns:

```sql
CREATE TABLE _written_crm_contacts (
    _cluster_id   text PRIMARY KEY,
    _written      jsonb NOT NULL,             -- field values
    _written_at   timestamptz NOT NULL,       -- entity-level write time
    _written_ts   jsonb NOT NULL DEFAULT '{}' -- per-field timestamps
);
```

`_written_ts` stores `{ "email": "2024-01-15T...", "name": "2024-01-10T..." }`.
The ETL is responsible for maintaining these timestamps — the engine
reads them but never writes them.

### Feedback loop

This is a closed loop between the engine and the ETL:

1. **Engine reads** `_written` + `_written_ts` from the written table.
2. **Forward view** compares each field's current value against
   `_written` and derives `_ts_{field}` using the CASE expression above.
3. **Resolution** uses `_ts_{field}` to pick winners (latest wins).
4. **Delta** outputs resolved values with their per-field timestamps.
5. **ETL writes** the delta results back to the written table, storing
   the resolved values in `_written` and the per-field timestamps in
   `_written_ts`.
6. **Next cycle**: goto 1.

### YAML syntax

```yaml
mappings:
  - name: csv_import
    source: csv_dump
    target: customer
    written_state: true
    derive_timestamps: true
    fields:
      - source: name
        target: full_name
      - source: email
        target: email
```

No `last_modified` needed on individual fields — the engine derives
timestamps for all mapped fields automatically. If a field also has an
explicit `last_modified`, the explicit one takes precedence.

### Mixed explicit and derived timestamps

A mapping can mix fields with explicit timestamps and derived ones:

```yaml
fields:
  - source: name
    target: full_name
    last_modified: name_updated_at    # source provides this — used directly
  - source: email
    target: email
    # no last_modified — derived from written state comparison
```

For `name`: the source's `name_updated_at` is used.
For `email`: the engine generates the CASE expression comparing against
`_written->>'email'`.

### Bootstrap

On first sync (no `_written_ts` yet), all fields appear changed
(IS DISTINCT FROM NULL). The engine emits NULL timestamps because it
has no change history — resolution falls to coalesce order (mapping
priority tiebreaker).

After the ETL writes back resolved timestamps into `_written_ts`,
subsequent cycles produce meaningful derived timestamps. The first
cycle is a "learning" cycle.

## Implementation

The forward view LEFT JOINs the written state table (alias `_ws`)
and generates CASE expressions for each field that lacks an explicit
`last_modified`. The `WrittenState` struct gains a `written_ts` field
(default `_written_ts`) for the JSONB column name, and a `written_at`
field (default `_written_at`) for the timestamp column.

## Interaction with other derive_* flags

| Flag | Derives from written state | Purpose |
|------|---------------------------|---------|
| `derive_noop` | `_written` JSONB values | Noop detection (entity-level) |
| `derive_tombstones` | `_written` JSONB arrays | Element removal detection |
| `derive_timestamps` | `_written` values + `_written_ts` + `_written_at` | Per-field change timestamps |

All three are independent opt-ins on top of `written_state: true`.
