# Derived per-field timestamps from written state

**Status:** Design

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
  WHEN src.email IS DISTINCT FROM (_written->>'email')
    THEN _written_at                        -- field changed: stamp with write time
  ELSE (_written_ts->>'email')::timestamptz -- field unchanged: carry forward
END AS _ts_email
```

This gives per-field granularity. If only `email` changed but `name`
didn't, `email` gets the current `_written_at` while `name` retains
its previous timestamp.

### Deriving time ranges

When the source also provides an entity-level `last_modified` (e.g.,
`updated_at`), and a field is detected as changed, the true change time
is bounded by a range:

- **min**: the previous `_written_at` — we confirmed the **old** value
  at this time, so the field was still unchanged then
- **max**: the source's `last_modified` — the entity was modified by
  this time at the latest

The field changed somewhere in `[previous_written_at, last_modified]`.

Timeline:
1. **T1** (previous `_written_at`): We confirmed the old value
2. **T2** (source `updated_at`): The source entity was modified
3. **T3** (current sync): We detect the field changed by comparing
   against `_written` — T3 is when we noticed, not when it happened

```sql
CASE
  WHEN src.email IS DISTINCT FROM (_written->>'email')
    THEN w._written_at       -- field changed: previous write time is lower bound
  ELSE (_written_ts_min->>'email')::timestamptz
END AS _ts_email_min,
CASE
  WHEN src.email IS DISTINCT FROM (_written->>'email')
    THEN src.updated_at      -- field changed: source timestamp is upper bound
  ELSE (_written_ts_max->>'email')::timestamptz
END AS _ts_email_max
```

This naturally produces the `[min, max]` time range described in
[TIME-RANGE-RESOLUTION-PLAN](TIME-RANGE-RESOLUTION-PLAN.md). The range
is an honest representation — we know the field changed, but we can
only bound when.

When the source has **no** `last_modified`, derived timestamps produce
a single point: `min = max = _written_at`. This is the degenerate case
(no range information available).

### Written table schema

The written table gains `_written_ts` JSONB columns to carry forward
per-field timestamps. With time range support, two columns are needed:

```sql
CREATE TABLE _written_crm_contacts (
    _cluster_id     text PRIMARY KEY,
    _written        jsonb NOT NULL,             -- field values
    _written_at     timestamptz NOT NULL,       -- entity-level write time
    _written_ts_min jsonb NOT NULL DEFAULT '{}', -- per-field timestamp lower bounds
    _written_ts_max jsonb NOT NULL DEFAULT '{}'  -- per-field timestamp upper bounds
);
```

When time ranges are not used, a single `_written_ts` column suffices
(min = max).

### Feedback loop

This is a closed loop between the engine and the ETL:

1. **Engine reads** `_written` + `_written_ts` from the written table.
2. **Forward view** compares each field's current value against
   `_written` and derives `_ts_{field}` using the CASE expression above.
3. **Resolution** uses `_ts_{field}` to pick winners (latest wins).
4. **Delta** outputs resolved values + their per-field timestamps.
5. **ETL writes** the delta results back to the written table, storing
   the resolved values in `_written` and the per-field timestamps in
   `_written_ts`.
6. **Next cycle**: goto 1.

On the first cycle (no written state yet), all fields are "changed"
(IS DISTINCT FROM NULL), so they all get `_written_at`.

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
explicit `last_modified`, the explicit one takes precedence (the source
system's own timestamp is more trustworthy than derived change
detection).

### Timestamp provenance matters

What `last_modified` means depends on who produced it:

- **Source system provided** (e.g., CRM's own `updated_at`): This is
  the entity's modification time. Combined with per-field change
  detection, it gives a time range: `[previous _written_at, updated_at]`
  — the field changed between when we last confirmed the old value and
  when the source says the entity was modified.

- **ETL added at fetch time** (e.g., ETL stamps each row with `now()`):
  This is really "when I last checked", not "when it changed". The
  meaningful uncertainty is `[last_fetch, current_fetch]` — the data
  changed sometime between the previous sync and this one. This is an
  ETL concern: the ETL should store a time range using the explicit
  `last_modified: { min: ..., max: ... }` syntax described in
  [TIME-RANGE-RESOLUTION-PLAN](TIME-RANGE-RESOLUTION-PLAN.md).

- **No timestamp at all**: `derive_timestamps: true` derives per-field
  timestamps as single points (`min = max = _written_at`).

| Has `last_modified`? | `derive_timestamps`? | Derived timestamp |
|----------------------|----------------------|-------------------|
| No                   | Yes                  | Single point: `_written_at` |
| Yes (source system)  | Yes                  | Range: `[previous _written_at, last_modified]` |
| Yes (source system)  | No                   | Explicit: exact `last_modified` (no change detection) |
| Yes (ETL fetch)      | N/A                  | ETL provides `min`/`max` range directly |

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

## Interaction with other derive_* flags

| Flag | Derives from written state | Purpose |
|------|---------------------------|---------|
| `derive_noop` | `_written` JSONB values | Noop detection (entity-level) |
| `derive_tombstones` | `_written` JSONB arrays | Element removal detection |
| `derive_timestamps` | `_written` JSONB values + `_written_ts` | Per-field change timestamps |

All three are independent opt-ins on top of `written_state: true`.

Note the parallel with `derive_noop`: both compare current values
against `_written`. `derive_noop` asks "did anything change?" (entity
level). `derive_timestamps` asks "which fields changed?" (field level)
and stamps them with `_written_at`.

## Open questions

1. **Column name**: Should `_written_ts` be the default, with override
   via `written_state: { written_ts: "field_timestamps" }`? Follows the
   existing `cluster_id` / `written` override pattern.

2. **Delta output**: Should the delta include the per-field timestamps
   so the ETL can write them back to `_written_ts`? Or should the ETL
   compute them itself? Engine-outputting them is cleaner (single source
   of truth), but adds columns to the delta.

3. **Clock skew**: If source A's ETL runs on machine A and source B's
   ETL runs on machine B, their `_written_at` values may not be
   comparable. This is an inherent limitation of any timestamp-based
   resolution, not specific to derived timestamps.

4. **Stale syncs**: If a source hasn't been synced in months, its
   per-field timestamps are old, and a recently-synced source will win
   even if its data hasn't actually changed more recently. This is
   arguably correct — recently confirmed data should take precedence.

5. **Bootstrap**: On first sync (no `_written_ts` yet), all fields
   appear changed. All get `_written_at`. This is correct behavior —
   the engine has no prior state, so the current sync is the best
   available information. But if two sources bootstrap simultaneously,
   the one that syncs last wins everything — which may surprise users.
