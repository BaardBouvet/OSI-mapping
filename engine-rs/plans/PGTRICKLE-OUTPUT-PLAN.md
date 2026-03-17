# pg_trickle deployment without dbt

**Status:** Design

A **separate post-processor** that reads the engine's SQL output and
rewrites views as pg_trickle stream tables with unique indexes.  Governed
by a YAML config file.  Lives outside the engine — the compiler stays
pure and knows nothing about pg_trickle.

---

## Motivation

The engine is a compiler: `mapping.yaml → SQL views`.  Deployment
concerns — how those views are materialised, on what schedule, with
which PostgreSQL extension — are a separate layer.

The [DBT-OUTPUT-PLAN](DBT-OUTPUT-PLAN.md) routes this through dbt,
which is appropriate for teams already using dbt.  But for teams that
want pg_trickle without dbt, a thin post-processor is the cleanest
path: no coupling, no engine changes, full per-view control.

### Why not `--output pgtrickle` in the engine

Embedding pg_trickle DDL generation in the engine would:

1. **Couple the compiler to a deployment target.** The engine shouldn't
   know about pg_trickle, dbt, materialized views, or any other
   execution strategy.  Those are deployment-time decisions.
2. **Force schedule policy into CLI flags.** Real deployments need
   different schedules per view — sub-second for sync-facing delta,
   hourly for analytics dashboards.  A CLI flag can't express that.
3. **Grow the engine's surface area.** Every new deployment target
   would add more flags and codepaths to the compiler.

A separate tool avoids all three problems.

### When to use what

| Scenario | Tool |
|----------|------|
| Plain views, `psql -f` | Engine only |
| Incremental views, no dbt | Engine + **this post-processor** |
| Incremental views, dbt | Engine + dbt + dbt-pgtrickle |
| Materialized views, no IVM | Engine + hand-written DDL (or future mat-view tool) |

---

## pg_trickle DDL primer

pg_trickle extends PostgreSQL with stream tables — incrementally
maintained materialised views that refresh automatically on a schedule
or in response to dependency changes.

```sql
SELECT pgtrickle.create_stream_table(
  '_id_contact',
  $def$ SELECT ... $def$,
  schedule     => '1m',
  refresh_mode => 'DIFFERENTIAL'
);
```

### Refresh modes

| Mode | Meaning | Best for |
|------|---------|----------|
| `DIFFERENTIAL` | Recompute only changed rows | Most views |
| `FULL` | Full recomputation each refresh | Fallback for unsupported constructs |
| `IMMEDIATE` | Synchronous on source change | Low-latency requirements |

### Schedule

- Clock interval: `'100ms'`, `'1s'`, `'1m'`, `'1h'`.
- `NULL` (CALCULATED): triggered by upstream stream table refresh —
  pg_trickle chains dependencies automatically from the defining query.

---

## Scheduling model

### Data flows from source tables to output leaves

pg_trickle uses triggers and CDC to detect changes in source tables.
The entire internal pipeline is CALCULATED — each layer refreshes
automatically when its upstream dependency changes.  The only views
that need a clock schedule are the **output leaves**: delta views
(for sync consumers) and analytics views (for dashboards/BI).

```
source tables  (external — pg_trickle observes via triggers/CDC)
      │
  _fwd_*       ← CALCULATED: refreshes when source table changes
      │
  _id_*        ← CALCULATED: refreshes when _fwd_* refreshes
      │
  _resolved_*  ← CALCULATED: refreshes when _id_* refreshes
      │
  ┌───┴───┐
  │       │
{target}  _rev_*    ← CALCULATED
  │           │
  │       _delta_*  ← output leaf: clock schedule (e.g. 100ms for sync)
  │
  └── output leaf: clock schedule (e.g. 1h for dashboards)
```

**Default:** every view becomes a DIFFERENTIAL stream table.  All are
CALCULATED except the output leaves — delta and analytics views — which
get a clock schedule that controls how often consumers see fresh data.

This means the **minimal config** is just a default output schedule:

```yaml
# trickle.yaml — minimal
schedule: "1s"
```

Every delta and analytics view gets a 1-second refresh.  Everything
upstream cascades from source-table CDC.

### Why delta and analytics are the leaves

Delta views (`_delta_*`) are read by ETL/sync consumers that write
changes back to source systems.  Analytics views (`{target}`) are
read by dashboards and BI tools.  These are the only views that
external consumers query — everything upstream is internal pipeline.

The schedule on these leaves controls **consumer-visible latency**:
how stale the data can be before the next refresh.  Sub-second for
real-time sync, minutes-to-hours for analytics.

### Per-view schedule overrides

Real deployments have different latency requirements per output:

- **Delta views** (sync targets): near-real-time — sub-second.
- **Analytics views** (dashboards): minutes to hours is fine.

The config file lets you override the schedule on any view:

```yaml
# trickle.yaml — production example
schedule: "1s"               # default output leaf schedule

overrides:
  contact:
    schedule: "1h"           # analytics dashboard, hourly is fine
  _delta_crm:
    schedule: "100ms"        # outbound sync, near-real-time
  _delta_erp:
    schedule: "500ms"        # sync, slightly less urgent
```

---

## Design

### D1: Separate tool consuming `--materialize` output

The post-processor consumes the engine's `--materialize` output (see
[MATERIALIZED-VIEW-INDEX-PLAN](MATERIALIZED-VIEW-INDEX-PLAN.md)), which
already emits `CREATE MATERIALIZED VIEW IF NOT EXISTS` with correct
`CREATE UNIQUE INDEX` statements per layer.  The post-processor rewrites
each materialized view into a `pgtrickle.create_stream_table()` call
and passes the indexes through unchanged.

```
osi-engine render mapping.yaml --materialize \
  | osi-trickle apply --config trickle.yaml
```

Or as a two-step workflow:

```
osi-engine render mapping.yaml --materialize > matviews.sql
osi-trickle apply --config trickle.yaml --input matviews.sql
```

No engine changes beyond what MATERIALIZED-VIEW-INDEX-PLAN already
provides.

### D2: Configuration file

```yaml
# trickle.yaml — pg_trickle deployment configuration

# Default clock schedule for output leaves (delta + analytics views).
# Required.  All delta and analytics views get this schedule unless overridden.
schedule: "1s"

# Default refresh mode for all stream tables (optional, default: DIFFERENTIAL).
refresh_mode: DIFFERENTIAL

# Per-view overrides.
# Set `schedule` to give a view its own clock.
# Set `mode: view` to keep a view as a plain SQL view (not a stream table).
# Set `refresh_mode` to override the default for a specific view.
overrides:
  contact:
    schedule: "1h"
  _delta_crm:
    schedule: "100ms"
  _delta_erp:
    schedule: "500ms"
  _some_debug_view:
    mode: view               # keep as plain view for debugging
```

**Resolution logic:**

1. If a view has a per-view override, use it.
2. Otherwise, if the view is an output leaf (`_delta_*` prefix or
   analytics view — no underscore prefix), assign the default `schedule`.
3. Otherwise, make it a CALCULATED stream table (no clock schedule).

Views with `mode: view` are rewritten back to plain
`CREATE OR REPLACE VIEW` (dropping the materialization).

### D3: What the tool transforms

The input from `--materialize` has three kinds of statements:

| Input | Transformation |
|-------|---------------|
| `CREATE MATERIALIZED VIEW IF NOT EXISTS "name" AS SELECT ...` | → `pgtrickle.create_stream_table('name', $def$ SELECT ... $def$, schedule => ..., refresh_mode => ...)` with drop-if-exists guard |
| `CREATE UNIQUE INDEX IF NOT EXISTS idx_name ON "name" (...)` | → passed through unchanged (pg_trickle supports indexes on stream tables) |
| `REFRESH MATERIALIZED VIEW CONCURRENTLY "name"` | → dropped (pg_trickle handles refresh scheduling internally) |
| Everything else (functions, `CREATE TABLE`, `BEGIN`/`COMMIT`) | → passed through unchanged |

This is much simpler than parsing plain `CREATE VIEW` output — the
`--materialize` output already has the correct unique keys, `NULLS NOT
DISTINCT`, and index naming.  The post-processor doesn't need to know
anything about pipeline semantics.

### D4: Idempotent output

pg_trickle's `create_stream_table` errors on duplicate names.  The
generated script wraps each stream table in a drop-if-exists guard:

```sql
DO $$ BEGIN
  PERFORM pgtrickle.drop_stream_table('_id_contact');
EXCEPTION WHEN undefined_object THEN NULL;
END $$;

SELECT pgtrickle.create_stream_table(
  '_id_contact',
  $def$
  SELECT ...
  $def$,
  schedule     => NULL,
  refresh_mode => 'DIFFERENTIAL'
);
```

The `CREATE UNIQUE INDEX IF NOT EXISTS` statements are already
idempotent from the engine's output.

### D5: What the tool does NOT do

- **No mapping YAML parsing** — it only reads the engine's SQL output.
- **No unique index computation** — the engine's `--materialize` output
  already includes the correct indexes per layer.
- **No DAG construction** — pg_trickle resolves dependencies from the
  defining queries automatically.
- **No expression or function rewriting** — the SQL body is opaque.

---

## Output example

Given `--materialize` engine output and `trickle.yaml` with
`schedule: "1s"` and `_delta_crm: { schedule: "100ms" }`:

```sql
BEGIN;

-- Passed through unchanged
CREATE OR REPLACE FUNCTION _osi_text_norm(...) ...;
CREATE TABLE IF NOT EXISTS "crm" (...);

-- Forward (CALCULATED — cascades from source table CDC)
DO $$ BEGIN PERFORM pgtrickle.drop_stream_table('_fwd_crm_contacts');
  EXCEPTION WHEN undefined_object THEN NULL; END $$;
SELECT pgtrickle.create_stream_table(
  '_fwd_crm_contacts',
  $def$ SELECT ... $def$,
  schedule => NULL, refresh_mode => 'DIFFERENTIAL'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx__fwd_crm_contacts
  ON "_fwd_crm_contacts" (_src_id);

-- Identity (CALCULATED)
DO $$ BEGIN PERFORM pgtrickle.drop_stream_table('_id_contact');
  EXCEPTION WHEN undefined_object THEN NULL; END $$;
SELECT pgtrickle.create_stream_table(
  '_id_contact',
  $def$ SELECT ... $def$,
  schedule => NULL, refresh_mode => 'DIFFERENTIAL'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx__id_contact
  ON "_id_contact" (_entity_id);

-- Resolution (CALCULATED)
DO $$ BEGIN PERFORM pgtrickle.drop_stream_table('_resolved_contact');
  EXCEPTION WHEN undefined_object THEN NULL; END $$;
SELECT pgtrickle.create_stream_table(
  '_resolved_contact',
  $def$ SELECT ... $def$,
  schedule => NULL, refresh_mode => 'DIFFERENTIAL'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx__resolved_contact
  ON "_resolved_contact" (_entity_id);

-- Analytics (output leaf — default 1s schedule)
DO $$ BEGIN PERFORM pgtrickle.drop_stream_table('contact');
  EXCEPTION WHEN undefined_object THEN NULL; END $$;
SELECT pgtrickle.create_stream_table(
  'contact',
  $def$ SELECT ... $def$,
  schedule => '1s', refresh_mode => 'DIFFERENTIAL'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_contact
  ON "contact" (_cluster_id);

-- Reverse (CALCULATED)
DO $$ BEGIN PERFORM pgtrickle.drop_stream_table('_rev_crm_contacts');
  EXCEPTION WHEN undefined_object THEN NULL; END $$;
SELECT pgtrickle.create_stream_table(
  '_rev_crm_contacts',
  $def$ SELECT ... $def$,
  schedule => NULL, refresh_mode => 'DIFFERENTIAL'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx__rev_crm_contacts
  ON "_rev_crm_contacts" (_cluster_id, _src_id)
  NULLS NOT DISTINCT;

-- Delta (output leaf — per-view override: 100ms)
DO $$ BEGIN PERFORM pgtrickle.drop_stream_table('_delta_crm');
  EXCEPTION WHEN undefined_object THEN NULL; END $$;
SELECT pgtrickle.create_stream_table(
  '_delta_crm',
  $def$ SELECT ... $def$,
  schedule => '100ms', refresh_mode => 'DIFFERENTIAL'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx__delta_crm
  ON "_delta_crm" (_cluster_id, "contact_id")
  NULLS NOT DISTINCT;

-- REFRESH MATERIALIZED VIEW lines from engine output are dropped
-- (pg_trickle handles refresh scheduling internally)

COMMIT;
```

Note: all `CREATE UNIQUE INDEX` statements are passed through verbatim
from the engine's `--materialize` output.  The post-processor doesn't
compute indexes — it only transforms `CREATE MATERIALIZED VIEW` →
`create_stream_table()`.

---

## Implementation options

The post-processor is intentionally simple — it doesn't need to be
Rust or live in the engine crate.

### Option A: Python script

Read SQL, parse view blocks with regex, load `trickle.yaml` with
PyYAML, emit rewritten DDL.  ~150 lines.  Easy to maintain,
no compilation step.

### Option B: Rust binary in a separate crate

A `osi-trickle` binary in its own crate (possibly in this repo as a
workspace member, or in a separate repo).  Shares no code with the
engine — just parses text.

**Recommendation:** Start with Option A for fastest iteration.  Move
to Option B if the tool grows or needs to ship as a single binary
alongside the engine.

---

## Relationship to other plans

| Plan | Relationship |
|------|-------------|
| MATERIALIZED-VIEW-INDEX-PLAN | **Prerequisite.** This tool consumes `--materialize` output. The engine provides materialized views + unique indexes; this tool rewrites them as stream tables. |
| DBT-OUTPUT-PLAN | Alternative path for dbt users.  Both build on the engine's output; neither changes the engine. |
| POLYGLOT-SQL-PLAN | Orthogonal.  If the engine gains dialect support, the post-processor still works — it wraps whatever SQL it receives. |

---

## Exit criteria

- Post-processor reads `--materialize` engine output and produces valid
  pg_trickle DDL.
- All materialized views become DIFFERENTIAL stream tables by default.
- Internal pipeline (forward through reverse) is CALCULATED — cascades from
  source-table CDC.
- Output leaves (delta + analytics) get the configured clock schedule.
- Per-view overrides control schedule, refresh mode, and view-vs-stream.
- Unique indexes from engine output passed through unchanged.
- `REFRESH MATERIALIZED VIEW` statements dropped.
- Idempotent output (drop-if-exists guards on stream tables).
- No engine changes required beyond MATERIALIZED-VIEW-INDEX-PLAN.
