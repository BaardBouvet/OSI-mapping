# Written noop state

Target-centric noop detection via ETL-maintained written state.

## Scenario

Two systems share a customer. CRM has higher priority for `name`.
ERP's source table always contains ERP's native value (`"Bob"`), while
the resolved golden record picks CRM's value (`"Alice"`).

Without `written_state`, the engine compares the resolved value against
`_base` (ERP's raw source) every cycle. Because `"Alice"` ≠ `"Bob"`,
it emits an update — even though on the previous cycle the ETL already
wrote `"Alice"` to ERP. This redundant update happens on every sync.

With `written_state: true`, the engine LEFT JOINs the `_written_erp`
table (maintained by the ETL) and adds a second noop check: does the
resolved value match what was previously written? Since `"Alice"` =
`"Alice"`, the row is classified as noop instead of update.

## Key features

- **`written_state: true`** — declares that the ETL maintains a
  `_written_{mapping}` table with the last-written field values (JSONB).
  Used for delete detection (row existence) and as input for noop.
- **`derive_noop: true`** — opt-in: use the `_written` values for
  noop detection. Assumes the ETL is the sole writer to the target.
- **Target-centric noop** — compares resolved fields against what the
  ETL last wrote, not what the source currently provides.
- **Complementary to `_base`** — the `_base` comparison is a fast path
  (source unchanged → noop). The `_written` comparison is a second check
  when the source differs from the resolved value but writing would be
  redundant.

## How it works

1. The forward view extracts source fields and builds `_base` as usual.
2. The delta view LEFT JOINs `_written_erp` on `_cluster_id`.
3. The action CASE has two noop branches:
   - `_base` match (fast path): source unchanged → noop.
   - `_written` match: resolved value matches last-written → noop.
4. Only if neither matches is the row classified as `update`.

## Caveat

`_written` records what the ETL last wrote — not what the target
currently has. If an external actor modifies the target after the ETL
write, `_written` becomes stale and the engine may incorrectly suppress
the update. This is acceptable when the ETL is the sole writer. When
external modifications are expected, consider the conflict detection
approach (see ETL-STATE-INPUT-PLAN §4) instead of relying purely on
the noop optimisation.

## When to use

When lower-priority sources permanently differ from the resolved value
(because a higher-priority source wins). Without `written_state`, these
differences trigger redundant writes on every sync cycle.
