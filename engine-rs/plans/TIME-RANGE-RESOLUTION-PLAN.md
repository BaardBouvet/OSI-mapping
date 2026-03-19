# Time range resolution

**Status:** Design

The current `last_modified` strategy uses a single timestamp per field.
This works when sources provide exact modification timestamps, but many
real-world scenarios produce a time range rather than a point:

- **Batch imports**: "this data was extracted sometime between 2am and 3am"
- **Event streams**: "event was produced at X, processed at Y"
- **Derived timestamps**: "this entity was synced between start and finish
  of the ETL run"
- **Source system granularity**: "last modified on 2024-01-15" (day-level,
  not second-level)

A time range is more honest than pretending a batch sync timestamp is an
exact modification time.

## Problem

### Single timestamp is a convenient lie

When we say `last_modified: updated_at`, we assume `updated_at` is the
precise moment the field changed. In practice:

- **Batch ETL**: The source table snapshot was taken at T1. The ETL
  writes to the mapping engine's source table at T2. The "last modified"
  could reasonably be anywhere in [T1, T2].
- **CDC lag**: Change data capture captures a change at T1. It's
  replicated and available to the engine at T2. The engineering team
  uses T1 as `last_modified`, but the uncertainty window is [T1, T2].
- **Derived timestamps** (from `derive_timestamps`): The ETL writes
  `_written_at = now()` but the source data could have been current
  as of the sync start time, not the write time.

### Resolution with ranges

When two sources provide overlapping time ranges for the same field, the
engine can't definitively say which is "newer":

```
Source A: email = "a@x.com", modified in [Jan 15 02:00, Jan 15 03:00]
Source B: email = "b@x.com", modified in [Jan 15 02:30, Jan 15 02:45]
```

B's range is entirely contained within A's range. Neither clearly wins.

## Proposed representation

### Schema: two columns per timestamp

Instead of a single `_ts_{field}` column, the engine generates two:
- `_ts_{field}_min` — earliest possible modification time
- `_ts_{field}_max` — latest possible modification time

A single-point timestamp is the degenerate case where min = max.

### YAML syntax

```yaml
fields:
  - source: email
    target: email
    last_modified:
      min: batch_start_ts
      max: batch_end_ts
```

Or for single-point (backwards compatible):

```yaml
fields:
  - source: email
    target: email
    last_modified: updated_at    # min = max = updated_at
```

### Resolution strategies for ranges

Several options, from simplest to most nuanced:

#### Option 1: Latest-max wins (optimistic)

Use `_ts_max` for resolution ordering. The source that *could* be most
recent wins. Simple, matches current single-timestamp behavior when
ranges don't overlap.

```sql
ORDER BY _ts_email_max DESC NULLS LAST
```

**Pro**: Simple, familiar, deterministic.
**Con**: Optimistic — a wide range (batch) can beat a narrow range
(precise timestamp) even when the precise timestamp is more reliable.

#### Option 2: Latest-min wins (conservative)

Use `_ts_min` for resolution ordering. The source whose data is
*definitely at least as new as* this time wins.

```sql
ORDER BY _ts_email_min DESC NULLS LAST
```

**Pro**: Conservative — only claims recency when confident.
**Con**: A source with a narrow, recent range can lose to a source
with an earlier min but much later max.

#### Option 3: Narrowest range wins on overlap (precision preference)

When ranges overlap, prefer the narrower range (it carries more
information). When ranges don't overlap, the later one wins.

```sql
CASE
  WHEN ranges_overlap THEN ORDER BY range_width ASC   -- narrower wins
  ELSE ORDER BY _ts_max DESC                          -- later wins
END
```

**Pro**: Rewards precision — a source that can pinpoint its timestamp
beats a source that gives a vague range.
**Con**: Complex, harder to reason about.

#### Option 4: Configurable (mapping author decides)

```yaml
targets:
  customer:
    fields:
      email:
        strategy: last_modified
        range_resolution: latest_max | latest_min | narrowest
```

**Pro**: Flexible — different use cases get different semantics.
**Con**: More config surface, more to document and understand.

### Recommended: Option 1 (latest-max) as default

`latest_max` is the natural extension of current behavior. When both
timestamps are single-point (min = max = value), it collapses to exactly
today's `last_modified` semantics. When ranges are involved, it's
optimistic but simple.

Option 4 (configurable) can be added later if users need it. Start with
the simple default.

## Delta output

The delta could expose the time range to the ETL:

```sql
SELECT
  _action,
  email,
  _ts_email_min,    -- when the winning source's data was earliest-possible
  _ts_email_max,    -- when the winning source's data was latest-possible
  ...
```

This lets the ETL decide how to interpret the range. Some target systems
accept a "last modified" timestamp — the ETL can use `_ts_max` for that.
Others might want the full range for audit purposes.

## Interaction with derive_timestamps

`derive_timestamps` is the primary producer of time ranges. It compares
each field's current value against the `_written` JSONB to detect
per-field changes. When the source also provides an entity-level
`last_modified`, the result is naturally a range:

- `_ts_min` = previous `_written_at` (we confirmed the **old** value
  at this time — the field was still unchanged)
- `_ts_max` = source's `last_modified` (the entity was modified by
  this time at the latest)

The field changed somewhere in `[previous_written_at, last_modified]`.

Timeline:
1. **T1** (previous `_written_at`): We confirmed the old value
2. **T2** (source `updated_at`): The source entity was modified
3. **T3** (current sync): We detect the change — T3 is when we
   noticed, not when it happened

This is an honest representation: we know the old value was current at
T1, and the source says the entity was modified at T2.

### Three cases

| Has `last_modified`? | `derive_timestamps`? | Result |
|----------------------|----------------------|--------|
| No                   | Yes                  | Single point: `min = max = _written_at` |
| Yes                  | Yes                  | Range: `[previous _written_at, last_modified]` |
| Yes                  | No                   | Single point: `min = max = last_modified` |

### ETL-provided ranges

Ranges can also come directly from the ETL when it knows the data
changed within a window but can't pinpoint the exact moment:

- The ETL syncs source A at 2am. Next sync is at 3am.
- Any changes in source A happened in `[2am, 3am]`.
- The ETL records this as `last_modified: { min: last_sync_ts, max: sync_ts }`.

```yaml
fields:
  - source: email
    target: email
    last_modified:
      min: last_sync_ts    # previous sync time
      max: sync_ts         # current sync time
```

This explicit range is separate from `derive_timestamps` — the ETL
provides the range columns directly in the source table.

See [DERIVED-TIMESTAMPS-PLAN](DERIVED-TIMESTAMPS-PLAN.md) for the full
design of per-field change detection and the feedback loop.

## Open questions

1. **Storage cost**: Two columns per field per timestamp doubles the
   timestamp overhead in forward/resolution views. Is this acceptable,
   or should range support be opt-in per field?

2. **Group resolution**: For atomic groups (`group:`), the group
   currently uses `GREATEST(_ts_field1, _ts_field2, ...)`. With ranges,
   should it use `GREATEST(_ts_max_field1, _ts_max_field2, ...)`?

3. **Noop detection**: Should the delta compare both min and max against
   written state, or just max? If only max changes (tighter range, same
   data), is that a noop?

4. **Display in analytics views**: Should the analytics view expose the
   range, or just max (for simplicity)?

5. **Is this actually needed?** Most systems using `last_modified` have
   exact timestamps. Time ranges may be an over-engineering risk. The
   strongest case is `derive_timestamps` where the range is inherent —
   but even there, using `_written_at` as a single point works fine in
   practice.
