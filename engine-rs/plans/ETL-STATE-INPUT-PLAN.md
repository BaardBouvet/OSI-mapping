# ETL state as engine input

**Status:** Design

The engine currently compares resolved values against `_base` (raw source
columns captured in the forward view) to detect noops and changes. But
`_base` answers "did my source change?" — not "does the target match what
I last wrote?" These are different questions, and several features need the
latter.

This plan analyses what the engine gains from reading a "last written state"
table maintained by the ETL, and whether this is distinct from "last read
state" from the source.

## Two kinds of previous state

### Last read: what the source looked like

"What did this source row contain when I last ran the pipeline?"

This is what `_base` approximates today — but `_base` captures the *current*
raw source values, not the *previous* ones. There is no true "last read"
state in the engine. The engine is stateless; `_base` is rebuilt from the
source table on every run.

If we had last-read state, we could detect:
- **Source-level hard deletes** — row was there, now gone
- **Source-level field changes** — which specific fields changed since last run
- **Null-transition detection** — field was non-NULL, now NULL (relevant for
  null-wins)

But this is essentially asking "give the engine a snapshot of the source."
That's a large amount of state (entire source tables) with complex
invalidation semantics. Not worth it — the identity layer and forward views
already handle source changes reactively.

### Last written: what the ETL wrote to the target

"What values did the ETL actually write to the target system on the last
sync cycle?"

This is operationally different and much more valuable. It answers:
- Did the target accept what we wrote? (It might have truncated, rounded,
  or rejected values.)
- Is the target's current state what we expect? (Another process might have
  modified it.)
- Was this entity/element previously synced at all? (Hard-delete detection.)

The ETL is the only actor that knows what was actually written. The engine
can't infer this from source data — the write might have been lossy,
rejected, or modified after the fact.

## What "last written state" enables

### 1. Hard-delete detection (already proposed)

From [HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md):
`synced_entities` tracks which entities were previously written. The engine
LEFT JOINs this to distinguish "never synced" (→ insert) from "was synced
but source row disappeared" (→ suppress or delete per `on_hard_delete`).

Row existence in `_written_{mapping}` is sufficient — the JSONB payload
is not needed for this use case, but comes for free.

### 2. Element-level deletion (already proposed)

From [ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md): `synced_elements`
tracks which array elements were previously written. The engine anti-joins
this to detect removed elements.

Row existence in `_written_elements_{mapping}` is sufficient — the JSONB
payload is not needed for this use case, but comes for free.

### 3. Precision-loss noop detection (new)

From [PRECISION-LOSS-PLAN](PRECISION-LOSS-PLAN.md): when the target system
has lower precision (truncated strings, rounded numbers), the resolved value
doesn't match what the target actually stores. The current noop check
compares resolved vs `_base` (source), which catches "did the source change?"
but not "will the target change?"

If the engine had the last-written values, it could compare resolved vs
last-written instead of resolved vs _base:

```sql
-- Current noop (source-centric):
WHEN _base->>'price' IS NOT DISTINCT FROM price::text THEN 'noop'

-- Enhanced noop (target-centric):
WHEN _written->>'price' IS NOT DISTINCT FROM price::text THEN 'noop'
```

If the ETL wrote `12` (after target truncation) and the resolved value is
still `12.50`, the comparison `'12' IS NOT DISTINCT FROM '12.50'` → not
noop → update. This is correct: we should try to write `12.50` again. But
if the target *always* truncates to `12`, this creates an infinite
update loop.

The precision-loss problem needs a `normalize` function on the engine side
(as proposed in PRECISION-LOSS-PLAN) to make the comparison aware of the
target's limitations. Last-written state doesn't solve precision loss by
itself — but it provides a complementary signal:

```sql
-- With last-written + normalize:
WHEN normalize(price) IS NOT DISTINCT FROM _written->>'price' THEN 'noop'
```

This says: "if the normalized resolved value matches what we last wrote,
don't bother writing again." This is strictly more correct than comparing
against `_base`, because `_base` reflects the *source* while `_written`
reflects the *target*.

### 4. External modification detection (new)

If the ETL stores what it wrote, and on the next cycle the source reflects
a *different* value than what was written, someone else modified the target.

```
Last written to B:  name = "Alice"
B's current value:  name = "Alicia"    ← modified outside the pipeline
Resolved:           name = "Alice"     ← unchanged
```

Current behavior: `_base->>'name' = 'Alicia'` vs resolved `'Alice'` →
update (overwrite B's local change).

With last-written state: the engine could detect that `_written->>'name' =
'Alice'` ≠ `_base->>'name' = 'Alicia'` → the target was externally
modified. This is a **conflict** — the pipeline wants to write one thing,
but someone changed it locally.

This doesn't change the delta action (still `'update'`), but it could
surface a `_conflict: true` flag for the ETL or monitoring to act on.

### 5. Incremental delta optimization (new)

If the engine knows what was last written, it can produce a narrower delta:
only the fields that actually differ between last-written and current
resolved. Today, an `'update'` row contains all fields — the ETL writes
all of them even if only one changed.

With last-written state:

```sql
CASE WHEN _written->>'name' IS NOT DISTINCT FROM name::text
     THEN NULL ELSE name END AS name,
CASE WHEN _written->>'phone' IS NOT DISTINCT FROM phone::text
     THEN NULL ELSE phone END AS phone
```

Fields that haven't changed since last write are NULLed out. The ETL
only writes the changed fields. This reduces write amplification for
wide tables and for APIs with per-field update costs.

## State table design

```sql
CREATE TABLE _written_{mapping} (
    _cluster_id   text NOT NULL PRIMARY KEY,
    _written      jsonb NOT NULL,         -- field values as written
    _written_at   timestamptz NOT NULL DEFAULT now()
);
```

A row exists → the entity was previously synced (hard-delete detection).
The JSONB payload carries field values → enables noop-against-target,
conflict detection, and incremental delta. One table covers all five
use cases.

For element-level tracking within array targets:

```sql
CREATE TABLE _written_elements_{mapping} (
    _parent_id    text NOT NULL,
    _element_id   text NOT NULL,
    _written      jsonb NOT NULL,
    _written_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (_parent_id, _element_id)
);
```

Same principle: row existence = "was synced"; JSONB = field values.

### Identity-only as future optimization

The identity-only case (just the key, no JSONB) is a degenerate form of
the full-row table. If profiling shows the JSONB column causes measurable
storage or join overhead for mappings that only need hard-delete detection,
we can introduce a lightweight variant later:

```sql
-- Potential optimization — not implemented initially
CREATE TABLE _synced_entities_{mapping} (
    _cluster_id   text NOT NULL PRIMARY KEY
);
```

Until then, the engine always uses `_written_{mapping}` and the ETL always
stores field values. One table, one code path, one mental model.

## What the ETL writes

After each sync cycle:
- Insert: add row with written values as JSONB
- Update: replace JSONB with newly written values
- Delete: remove row
- Noop: no change

The ETL captures what it *actually wrote* — which may differ from what
the engine told it to write (due to target-side truncation, defaults,
normalization). For maximum accuracy, the ETL should store the
*acknowledged* values (the response from the target API), not the
*intended* values (from the delta view).

If the ETL can't capture acknowledged values (e.g., fire-and-forget
writes), it stores the intended values. This is still better than no state.

## What the engine reads

### For noop detection (opt-in via `written_noop`)

The `_written` noop is **not enabled by default**. It requires
`written_noop: true` on the mapping because it assumes the ETL is the
sole writer to the target. If an external actor modifies the target after
the ETL write, `_written` becomes stale and the engine incorrectly
classifies the row as noop.

When `written_noop: true` is set together with `written_state`, the delta
CASE adds a second noop branch after the `_base` fast path:

```sql
-- Without _written (current):
WHEN _base->>'name' IS NOT DISTINCT FROM name::text THEN 'noop'

-- With _written:
WHEN _written->>'name' IS NOT DISTINCT FROM name::text THEN 'noop'
```

This changes the noop question from "did the source change?" to "does the
target need to change?" — which is the question the ETL actually cares
about.

Note: `_base` comparison is still useful as an optimization. If `_base`
matches, the source hasn't changed, so the resolved value can't have
changed either — skip the `_written` comparison entirely. The engine could
use `_base` as a fast path and `_written` as the definitive check:

```sql
WHEN _base->>'name' IS NOT DISTINCT FROM name::text
 AND _base->>'phone' IS NOT DISTINCT FROM phone::text
THEN 'noop'                         -- fast path: source unchanged
WHEN _written->>'name' IS NOT DISTINCT FROM name::text
 AND _written->>'phone' IS NOT DISTINCT FROM phone::text
THEN 'noop'                         -- slow path: target matches
```

### For conflict detection

```sql
CASE
  WHEN _written->>'name' IS NOT DISTINCT FROM _base->>'name'
  THEN false                        -- target unchanged since last write
  ELSE true                         -- target was modified externally
END AS _conflict
```

### For incremental delta

```sql
CASE WHEN _written->>'name' IS NOT DISTINCT FROM name::text
     THEN NULL ELSE name END AS name
```

## Relationship between _base and _written

| | `_base` | `_written` |
|---|---|---|
| **What** | Raw source columns (pre-expression) | Values written to target by ETL |
| **Captures** | Current source state | Previous target state |
| **Built by** | Engine (forward view) | ETL (after write) |
| **Scope** | Per-mapping, current run | Per-mapping, previous run |
| **Question** | "Did the source change?" | "Does the target need to change?" |
| **Noop semantics** | Source-centric | Target-centric |
| **State requirement** | None (rebuilt each run) | Persistent table |

They're complementary, not replacements. `_base` is free (stateless).
`_written` is more accurate but requires ETL to maintain state.

## Mapping-level property: `written_state`

```yaml
mappings:
  - name: system_b
    source: system_b
    target: customer
    written_state: true
    written_noop: true            # opt-in: use _written for noop detection
    on_hard_delete: suppress      # from HARD-DELETE-PROPAGATION-PLAN
    fields: [...]
```

`written_state: true` declares the ETL-maintained table. It enables
hard-delete detection (row existence) and provides the data for conflict
detection. `written_noop: true` is a separate opt-in that adds the
target-centric noop branch — only appropriate when the ETL is the sole
writer to the target.

Like `cluster_members`, supports custom table/column names:

```yaml
written_state: true
# → table: _written_system_b
# → columns: _cluster_id, _written, _written_at

written_state:
  table: system_b_write_log
  cluster_id: entity_id
  written: last_payload
```

## Scope and phasing

### Phase 1: Written state + hard-delete + element deletion

- `written_state` / `_written_{mapping}` — full row state from ETL
- Hard-delete detection via row existence (subsumes `synced_entities`)
- Element deletion via `_written_elements_{mapping}` row existence
  (subsumes `synced_elements`)
- `on_hard_delete: suppress | delete | propagate` policy
- `_overrides_{mapping}` — operator overrides
- Noop against target (replaces `_base` comparison when declared)
- Conflict detection flag (`_conflict`)

### Phase 2: Incremental delta

- Per-field diffing against `_written` to NULL out unchanged fields
- Write-amplification reduction
- Requires Phase 1.

### Future: Identity-only optimization

If storage or join performance becomes an issue for mappings that only
need hard-delete detection, introduce lightweight `synced_entities` /
`synced_elements` tables (keys only, no JSONB). This is a performance
optimization, not a feature — the engine behavior is identical.

## Relationship to other plans

- **[HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md)** —
  uses `_written_{mapping}` row existence for hard-delete detection.
  `on_hard_delete` policy resolves in the delta CASE expression.
- **[ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md)** — uses
  `_written_elements_{mapping}` row existence for element removal.
- **[PRECISION-LOSS-PLAN](PRECISION-LOSS-PLAN.md)** — `normalize` handles
  the engine-side transformation; `_written` handles the target-side
  comparison. Compose for correct noop detection with lossy targets.
- **[PASSTHROUGH-PLAN](PASSTHROUGH-PLAN.md)** — orthogonal. Passthrough
  carries unmapped source columns to delta output. Written state carries
  target values back as input. Different directions.
