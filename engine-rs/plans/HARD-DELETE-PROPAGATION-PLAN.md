# Hard-delete propagation

**Status:** Design

When a source row disappears (hard delete), the engine's stateless views
cannot distinguish "this entity was never in this source" from "this entity
was intentionally removed from this source." Both produce `_src_id IS NULL`
in the reverse view, which the delta classifies as `'insert'`. This creates
a re-insertion loop: deleting an entity from one system causes the pipeline
to re-insert it from another system's contribution.

Complements [PROPAGATED-DELETE-PLAN](PROPAGATED-DELETE-PLAN.md), which handles
**soft** deletes (source keeps the row with a flag). This plan handles **hard**
deletes (source row is gone).

## Problem

### The re-insertion loop

Consider two systems synchronized through the engine:

**T0 — Entity originates in System A:**

```
A: { email: "alice@example.com", name: "Alice" }      ← origin
B: (nothing)
Delta for B: _action = 'insert'  (_src_id IS NULL)
```

**T1 — ETL inserts into System B with feedback:**

```
A: { email: "alice@example.com", name: "Alice" }
B: { email: "alice@example.com", name: "Alice" }      ← ETL-inserted
   cluster_members: (_cluster_id = X, _src_id = B.pk)
Delta for A: 'noop'
Delta for B: 'noop'
```

**T2 — Entity deleted from System A (hard delete):**

```
A: (row gone)
B: { email: "alice@example.com", name: "Alice" }      ← still there
Resolved entity: exists (from B's contribution alone)
Delta for A: _src_id IS NULL → 'insert'    ← WRONG: re-inserts into A
Delta for B: _src_id exists  → 'noop'
```

The pipeline re-inserts the entity into A — the system that just deleted it.
On the next cycle, A has the row again. If a user or process deletes it again,
the loop repeats indefinitely.

### Why this happens

The delta view's classification logic:

```sql
CASE
  WHEN _src_id IS NULL THEN 'insert'   -- no member from this source
  ...
END
```

This treats **absence** as **need-to-insert**. It has no concept of "this
entity used to have a member from this source that was intentionally removed."

### All the cases

| Scenario | Current behavior | Desired behavior |
|----------|-----------------|------------------|
| Entity in A only, never in B | Delta B = insert | insert (correct) |
| Entity in A+B, deleted from A (origin) | Delta A = insert (re-insert) | Delete from A + B |
| Entity in A+B, deleted from B (non-origin) | Delta B = insert (re-insert) | Depends on policy |
| Entity in A+B, deleted from both | Entity vanishes silently | Explicit delete from both |
| Entity soft-deleted in A (row kept) | Handled by propagated-delete | Already solved |

The interesting cases are 2, 3, and 4.

## Analysis

### Case 2: Origin deletes the entity

The entity exists in B only because the ETL put it there. B's copy is derived,
not primary. When the origin (A) removes the entity, there are two valid
responses:

- **Delete from B** — appropriate for compliance (GDPR). Use the soft-delete
  propagation pattern for auditable, per-system control.
- **Unlink B** — appropriate for operational safety. Sever the feedback link
  so B's copy becomes independent. Self-healing if the entity reappears in A.

In both cases, suppress re-insertion into A.

### Case 3: Non-origin system loses the entity

This is ambiguous. Why did B lose the entity?

- **Intentional deletion in B** — a user or process decided B shouldn't have
  this entity. Re-inserting it fights the user's intent.
- **Accidental/data loss in B** — the row was lost and re-insertion would be
  a recovery. Suppressing re-insertion loses the data.
- **System B doesn't persist ETL writes** — B is an ephemeral system and the
  row expired. Re-insertion is the correct steady-state behavior.

The right default depends on the operational context. But in practice, if the
ETL is the one managing B's data, the ETL knows whether it put the row there
or whether B had it natively.

**Correct behavior:** Configurable per mapping. Default: suppress re-insertion
(deletion wins), because the alternative (re-insertion loop) is observably
broken while a missed re-insertion is recoverable.

### Case 4: Entity deleted from all sources

The entity disappears from all forward views. No rows enter the identity view.
The resolved view has no entity. The reverse and delta views produce nothing.

**Current behavior:** The entity silently vanishes. No delta action is produced
for any mapping — no insert, no delete, just... gone.

**Desired behavior:** The delta should produce `'delete'` for every mapping
that previously had this entity. But the engine can't do this because it's
stateless — it doesn't know what "previously" looked like.

## Design options

### Option A: Soft-delete everything (no engine changes)

Mandate that sources use soft deletes instead of hard deletes. Map the
soft-delete marker to a shared target field via the existing propagated-delete
pattern:

```yaml
targets:
  customer:
    fields:
      is_deleted:
        strategy: bool_or

mappings:
  - name: system_a
    fields:
      - expression: "deleted_at IS NOT NULL"
        target: is_deleted
  - name: system_b
    reverse_filter: "is_deleted IS NOT TRUE"
    fields: [...]
```

This works perfectly for sources that support soft deletes. The entity row
persists in the forward view with `is_deleted = true`, flows through
resolution, and triggers `reverse_filter`-based deletes in other systems.

**Limitation:** Not all sources support soft deletes. Legacy systems, SaaS
APIs, and event-sourced systems may only support hard deletes. Requiring
every source to maintain tombstone records is a significant operational ask.

### Option B: ETL-layer provenance tracking (recommended)

The stateful ETL process already tracks insert feedback (`cluster_members`
or `cluster_field`). Extend this tracking to include **provenance** — whether
each entity-source relationship is native or ETL-inserted — and use it to
detect and propagate hard deletes.

#### Provenance state machine

For each (entity, mapping) pair, the ETL maintains a state:

```
┌──────────┐    ETL inserts     ┌──────────┐
│          │ ──────────────────→ │          │
│  absent  │                    │ inserted │
│          │ ←────────────────── │          │
└──────────┘    source deletes  └──────────┘
      │                               │
      │ source appears                │ source appears
      ▼                               ▼
┌──────────┐                    ┌──────────┐
│  native  │                    │  native  │
└──────────┘                    └──────────┘
      │
      │ source disappears
      ▼
┌──────────┐
│ vanished │ → ETL decides: delete from other systems?
└──────────┘
```

States:

- **absent** — entity has no member from this mapping (initial state, or
  after confirmed deletion)
- **native** — source natively contains this entity (appeared in forward view
  without ETL intervention)
- **inserted** — ETL inserted this entity into this source (insert feedback
  was written)
- **vanished** — entity was native but the source row disappeared (hard delete
  detected)

#### ETL tracking table

```sql
CREATE TABLE _etl_provenance (
    cluster_id    text NOT NULL,
    mapping       text NOT NULL,
    src_id        text,              -- source PK (NULL when absent)
    provenance    text NOT NULL,     -- 'native' | 'inserted' | 'vanished'
    updated_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (cluster_id, mapping)
);
```

#### ETL sync cycle

On each run, the ETL queries the delta view and compares against provenance:

```
For each entity E in delta:
  For each mapping M:
    current_action = delta._action for (E, M)
    prev_state = provenance(E, M)

    If current_action = 'insert' AND prev_state = 'absent':
      → Perform insert. Record provenance = 'inserted'.

    If current_action = 'insert' AND prev_state = 'inserted':
      → Source deleted the ETL-inserted row. Don't re-insert.
        Record provenance = 'absent'.
        Trigger deletion policy (see below).

    If current_action = 'insert' AND prev_state = 'native':
      → Source hard-deleted a native row. Don't re-insert.
        Record provenance = 'vanished'.
        Trigger deletion policy (see below).

    If current_action = 'noop'/'update':
      → Normal sync. Record provenance = 'native' (source has the row).

    If current_action = 'delete':
      → Reverse filter/required triggered. Perform delete.
        Record provenance = 'absent'.
```

#### Deletion policy

When a native or inserted entity is hard-deleted from one system, the ETL
must decide what to do with other systems' copies. Policy options:

**1. Origin-wins (recommended default)**

If the origin (first system to have the entity natively) deletes it, propagate
deletion to all systems where the entity was ETL-inserted. If a non-origin
system deletes it, suppress re-insertion but don't propagate.

```
A (native, origin) deletes → delete from B (inserted) + suppress re-insert to A
B (inserted) deletes       → suppress re-insert to B, A keeps its copy
```

This respects the authority of the originating system. The ETL tracks origin
as the mapping with the earliest `native` provenance.

**2. Any-delete-wins**

If any system deletes the entity, propagate deletion to all systems.

```
A deletes → delete from B + suppress re-insert to A
B deletes → delete from A + suppress re-insert to B
```

This is the user's "sensible default" — simple, predictable, no sync loops.
But it means a mistaken deletion in any system propagates everywhere.

**3. Quorum delete**

Only propagate deletion when a majority (or all) of the systems that had the
entity have deleted it. Prevents a single system's mistake from cascading.

**4. Per-mapping policy**

Let the mapping author declare the policy per mapping:

```yaml
mappings:
  - name: system_b
    deletion_policy: follow_origin   # or: independent, any_wins
```

This would be a new mapping property that the ETL interprets (not the engine).

#### Handling Case 4 (all sources delete)

When an entity disappears from all forward views, the resolved view has no
row. Neither the reverse nor delta views produce any output for it.

The ETL detects this by noticing that entities in its provenance table are no
longer present in any delta output:

```sql
-- Entities that were tracked but are no longer in any delta view
SELECT p.cluster_id, p.mapping
FROM _etl_provenance p
WHERE p.provenance IN ('native', 'inserted')
  AND NOT EXISTS (
    SELECT 1 FROM _delta_{source} d
    WHERE d._cluster_id = p.cluster_id
  );
```

For these entities, the ETL emits delete actions for every mapping that had
them and updates provenance to `'absent'`.

### Option C: Engine-emitted provenance hints (engine extension)

Add a `_provenance` column to the delta view that distinguishes first-insert
from re-insert:

```sql
CASE
  WHEN _src_id IS NULL AND _cm._src_id IS NOT NULL THEN 'was_present'
  WHEN _src_id IS NULL THEN 'insert'
  ...
END
```

If `cluster_members` has an entry for this mapping+entity but the forward
view doesn't emit the row, the entity *was* present but is now gone. The
delta could emit `'was_present'` instead of `'insert'`, giving the ETL a
hint that this is a re-insertion scenario.

**Limitation:** Only works when `cluster_members` is in use. Doesn't cover
`cluster_field` (because the source row is gone, so the field is gone too).
Adds complexity to the delta CASE expression. Does not handle Case 4 (all
sources delete — entity vanishes from views entirely).

### Option D: Reuse `written_state` table (recommended)

Same pattern as [ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md) Option E:
the engine reads a state table maintained by the ETL to compute the diff in
SQL. The engine doesn't write to the table — the ETL does, after each cycle.

The key insight: no new `synced_entities` table is needed. The existing
`_written_{mapping}` table from `written_state: true` already provides the
set of previously-synced entity IDs via its `_cluster_id` primary key. Row
existence in `_written` = "this entity was previously synced." This is the
same table used for noop detection (JSONB payload) and element-level deletion
(`derive_tombstones`). One table, three features.

```yaml
mappings:
  - name: system_b
    source: system_b
    target: customer
    cluster_members: true       # existing: insert feedback for cluster identity
    written_state: true         # existing: enables noop, tombstones, AND hard-delete detection
    tombstone_policy: suppress  # policy when source row disappears (default)
    fields:
      - source: email
        target: email
      - source: name
        target: name
```

#### Engine-generated delta enhancement

When `tombstone_policy` is set and `written_state` is declared, the
engine adds a CASE branch before the insert branch. The `_written` table
is already LEFT JOINed as `_ws` when `written_state` is present. The
engine can distinguish three states:

```sql
-- Inside the delta CASE expression:
CASE
  WHEN _src_id IS NULL
   AND _ws."_cluster_id" IS NOT NULL
  THEN NULL                            -- suppress (or 'delete' per policy)
  WHEN _src_id IS NULL
   AND {delete_conditions}
  THEN NULL                            -- existing: filter non-qualifying inserts
  WHEN _src_id IS NULL
  THEN 'insert'                        -- never synced, genuine new insert
  WHEN {delete_conditions}
  THEN 'delete'                        -- existing: reverse_filter/reverse_required
  WHEN {noop_conditions}
  THEN 'noop'
  ELSE 'update'
END AS _action
```

No additional LEFT JOIN needed — `_ws` is already present.

#### What each action means for the ETL

| `_action` | ETL response |
|-----------|-------------|
| `'insert'` | Insert into target, write to `_written` |
| `'update'` | Update in target, update `_written` |
| `'noop'` | Skip |
| `'delete'` | Delete from target, remove from `_written` |
| `NULL` | Row excluded — ETL never sees it |

The ETL is purely mechanical. It never interprets why an action was chosen.
The `on_hard_delete` policy is resolved inside the engine's CASE expression
— the ETL only sees the definitive result.

#### Handling Case 4 (all sources delete)

When an entity disappears from all forward views, the resolved view has no
row. The reverse and delta views produce nothing.

The engine can handle this too. When `on_hard_delete` is declared, the
delta view can include orphaned written entities:

```sql
-- Entities tracked in _written but absent from the resolved view
SELECT
    _ws."_cluster_id",
    NULL AS _src_id,
    'delete' AS _action
FROM _written_system_b AS _ws
LEFT JOIN _resolved_customer AS r
  ON r._entity_id_resolved = _ws."_cluster_id"
WHERE r._entity_id_resolved IS NULL
```

This is UNION ALL'd into the delta view, producing a `'delete'` action for
entities that no longer exist anywhere. The ETL then deletes from the
target and removes from `_written`.

#### Comparison: Option B (pure ETL) vs Option D (reuse written_state)

| | B: Pure ETL provenance | D: Reuse written_state |
|---|---|---|
| Diff logic | ETL code (bespoke) | SQL views (testable) |
| ETL responsibility | Provenance state machine + diff | Write to `_written` (already doing this) |
| Engine changes | None | Delta CASE branch |
| New tables | ETL provenance table | None (reuses `_written`) |
| Case 4 (all-gone) | ETL must scan for orphans | Engine emits `'delete'` |
| Testable in engine | No | Yes (SQL views) |
| Complexity | ETL: high, Engine: none | ETL: none (already writes `_written`), Engine: moderate |

### Option comparison (all)

| | A: Soft-delete | B: ETL provenance | C: Engine hints | D: Reuse written_state |
|---|---|---|---|---|
| Engine changes | None | None | Delta CASE | Delta CASE branch |
| ETL complexity | None | Provenance + diff | Interpret hint | None (already writes `_written`) |
| Source changes | Must soft-delete | None | None | None |
| New tables | None | ETL provenance | None | None (reuses `_written`) |
| Handles hard delete | No | Yes | Partial | Yes |
| Case 4 (all-gone) | Only if soft-deleted | ETL orphan scan | No | Engine emits `'delete'` |
| Per-system policy | reverse_filter | ETL policy | Limited | `tombstone_policy` in mapping |
| Diff is SQL | N/A | No | Partial | Yes |
| Testable in engine | Yes | No | Yes | Yes |

## Recommendation

**Unify under `written_state`** — no new `synced_entities` concept needed.

The `_written_{mapping}` table from `written_state: true` already provides
the set of previously-synced entity IDs via its `_cluster_id` primary key.
The engine can detect "entity in `_written` but absent from resolved view"
the same way `derive_tombstones` detects "element in `_written` JSONB but
absent from forward view." Both features read from `written_state` but
are independent — `derive_tombstones` handles element-level array
tombstones, `tombstone_policy` handles entity-level hard deletes.

**Option A (soft-delete)** remains complementary for sources that support it.

The key principle: the diff computation is a pure function of (current
resolved entities × previously synced entities). Row existence in
`_written_{mapping}` is sufficient — the JSONB payload is not needed for
hard-delete detection, but comes for free. The ETL already writes to
`_written` for noop detection and element deletion — hard-delete detection
is an automatic bonus.

### Entity and element detection are orthogonal

`derive_tombstones` detects element-level deletions: elements present in
the written JSONB array but absent from the current forward view are
excluded from all sources' reconstructed arrays.

`tombstone_policy` detects entity-level deletions: entities present in
`_written` (row existence) but absent from the source are either
suppressed or emitted as deletes.

Both read from `_written` but serve different levels of the data model.
You can use either independently:
- `tombstone_policy` alone: entity-level hard-delete handling without
  element-level array tombstones
- `derive_tombstones` alone: element-level array tombstones without
  entity-level hard-delete policy
- Both together: full coverage at both levels

### Policy belongs in the mapping, not the ETL

The ETL is a mechanical process: read delta, execute actions, write state.
It should not contain policy logic. If a hard-deleted entity means "don't
re-insert", that decision must be made by the engine based on a declaration
in the mapping YAML — not by ETL code interpreting hints.

This means the engine must emit **definitive actions** that the ETL
executes without judgment:

| `_action` | ETL does | ETL thinks |
|-----------|----------|------------|
| `insert` | Insert into target | "Engine says insert, I insert" |
| `update` | Update in target | "Engine says update, I update" |
| `noop` | Skip | "Nothing to do" |
| `delete` | Delete from target | "Engine says delete, I delete" |
| `NULL` | Skip row entirely | "Not applicable" |

The ETL never interprets *why* an action was chosen. It never decides
between re-inserting and suppressing. That logic is baked into the engine's
CASE expression, driven by the mapping author's configuration.

### Mapping-level property: `tombstone_policy`

Entity-level hard-delete policy. Declares what the engine should emit
when a previously-synced entity disappears from this source. Requires
`written_state` (the `_written` table provides the "previously synced"
set). Independent of `derive_tombstones` (which handles element-level
tombstones).

```yaml
mappings:
  - name: system_a
    source: system_a
    target: customer
    written_state: true
    tombstone_policy: suppress        # ← policy declaration (default)
    fields: [...]
```

Values:

| `tombstone_policy` | Delta output for this mapping | Effect |
|-------------------|-------------------------------|--------|
| `suppress` (default) | `NULL` (row excluded from delta) | No action. Entity stays as-is in target. No re-insert, no delete. |
| `delete` | `'delete'` | ETL deletes from this target system. |
| `propagate` | `'delete'` on ALL mappings for this entity | ETL deletes from every system. |

#### How each policy generates SQL

**`suppress`** — the simplest and safest default:

```sql
WHEN _src_id IS NULL
 AND _ws."_cluster_id" IS NOT NULL    -- was synced (row exists in _written), now gone
THEN NULL                              -- exclude from delta entirely
```

The row is excluded. The ETL never sees it. No action taken. The entity
remains in `_written_{mapping}` as a suppression marker — preventing future
`'insert'` actions for this entity.

This means: a hard delete in the source is silently absorbed. The target
system is unaffected. The resolved entity continues to exist if other
sources contribute to it. If all sources disappear, the `vanished` action
(see below) handles cleanup.

**`delete`** — explicit cleanup of this mapping only:

```sql
WHEN _src_id IS NULL
 AND _ws."_cluster_id" IS NOT NULL
THEN 'delete'                          -- delete from this target system
```

The ETL deletes from the target and removes the row from `_written_{mapping}`.

**`propagate`** — cross-system deletion. This is more complex: when source
A disappears and mapping A has `tombstone_policy: propagate`, the engine must
emit `'delete'` not just for A's delta but for every other mapping's delta
for this entity. This requires the engine to check the `_written` tables
of all other mappings for the same target.

This is the hardest to implement but handles the "origin system deletes,
all copies should go" case. For compliance (GDPR), the soft-delete pattern
remains the recommended approach because it's auditable and per-system.

#### Default: `suppress`

`suppress` is the right default because:

- **No data destruction** — target systems are unaffected
- **No loop** — the `_written` entry prevents re-insertion
- **Mechanical ETL** — the ETL doesn't see the row, so it does nothing
- **Recoverable** — if the source row reappears, the forward view picks
  it up again. The entity re-links via identity fields. The `_written`
  entry is already present, so the delta emits `'noop'` or `'update'`
  instead of `'insert'` — no duplicate.

The one-way-door concern: the `_written` entry acts as a permanent
suppression. If the entity is truly gone from all sources and should be
cleaned up, the `vanished` mechanism handles it (see below).

#### Vanished entities

When `tombstone_policy` is set with `written_state`, the delta view
UNION ALLs orphaned entities — present in `_written` but absent from the
resolved view:

```sql
SELECT _ws."_cluster_id", NULL AS _src_id, 'delete' AS _action
FROM _written_system_b AS _ws
LEFT JOIN _resolved_customer AS r
  ON r._entity_id_resolved = _ws."_cluster_id"
WHERE r._entity_id_resolved IS NULL
```

This produces a `'delete'` for entities that vanished from all sources.
The ETL deletes from the target and removes from `_written`. Clean,
deterministic, no policy decision needed — the entity is simply gone.

### Unlink as a policy option

The unlink approach (sever the `cluster_members` feedback link so B's
record becomes independent) is a valid policy, but it's complex because
it requires the ETL to write to `cluster_members` — which is a different
table with different semantics. It also creates ghost data (stale,
unmanaged records).

Rather than a first-class `tombstone_policy` value, unlink is better
expressed as an automation that an operator triggers: remove the
cluster_members entry manually (or via admin UI) when they want a
specific entity to become independent. It's an operational action, not
a steady-state policy.

### Full delta CASE with `written_state` and `tombstone_policy`

For `tombstone_policy: suppress` (default):

```sql
CASE
  WHEN _src_id IS NULL
   AND _ws."_cluster_id" IS NOT NULL
  THEN NULL                            -- suppress (exclude from delta)
  WHEN _src_id IS NULL
   AND {delete_conditions}
  THEN NULL                            -- existing: filter non-qualifying inserts
  WHEN _src_id IS NULL
  THEN 'insert'                        -- never synced, genuine new insert
  WHEN {delete_conditions}
  THEN 'delete'                        -- existing: reverse_filter/reverse_required
  WHEN {noop_conditions}
  THEN 'noop'
  ELSE 'update'
END AS _action
```

For `tombstone_policy: delete`:

```sql
  WHEN _src_id IS NULL
   AND _ws."_cluster_id" IS NOT NULL
  THEN 'delete'                        -- was synced, now gone → delete
```

The ETL sees only definitive actions. No interpretation needed.

### The one-way-door problem (generalised)

Deletion suppression creates a one-way door — once something is marked as
"don't re-create", there's no way to undo that through the normal pipeline.
The mapping YAML describes steady-state behavior, not one-time overrides.
This problem appears at both entity and element level, but in structurally
different ways.

#### Entity level: the re-insertion loop

When the ETL handles a suppressed entity E in mapping A:

- If it removes E from `_written` → next cycle sees `_src_id IS NULL`
  + no `_written` entry → `'insert'` → loop resumes
- If it keeps E in `_written` → engine suppresses forever
  → E can never come back through normal pipeline

The ETL must choose between a loop and a deadlock.

#### Element level: the grow-only wall

When one source removes an element but another still contributes it:

- The element stays in the resolved view (union of all sources)
- The reverse view still shows it → synced_elements sees it → noop
- **No deletion is detected.** The element can't be unilaterally removed.

The escape is Option A (tombstone field) — but with `bool_or`, once any
source sets `is_removed = true`, the removal is sticky. No source can undo
it by setting `is_removed = false`, because `bool_or` = true if ANY is true.

Using `last_modified` instead of `bool_or` lets sources un-remove (most
recent writer wins), but that's a strategy choice, not always appropriate.

#### Element level: the vanish case

When ALL sources drop an element, it vanishes from resolved. The
synced_elements anti-join detects this cleanly: `_element_action = 'delete'`.
After the ETL removes it from synced_elements, the element is simply gone.
**No loop** — unlike entities, there's no "phantom insert" because no source
contributes the element to the resolved view.

#### Where the problems map

| Scenario | Entity level | Element level |
|----------|-------------|---------------|
| One source deletes, others keep | `hard_deleted` + loop risk | Grow-only wall (no deletion detected) |
| All sources delete | `vanished` (clean) | `delete` (clean) |
| Sticky removal (bool_or tombstone) | N/A | One-way door (can't un-remove) |
| Override needed? | Yes — to break the loop | Yes — to force removal or un-removal |

The common thread: **any deletion mechanism needs an override path**. The
override is always an operator intent that can't be expressed in the
declarative mapping — it's a one-time imperative action.

### Unified override mechanism

Rather than separate override tables per concern, a single per-mapping
overrides table handles both entity and element lifecycle:

```sql
CREATE TABLE _overrides_{mapping} (
    _cluster_id     text NOT NULL,       -- entity identity
    _element_id     text,                -- NULL for entity-level, element identity for element-level
    _override       text NOT NULL,       -- 'insert' | 'delete' | 'clear'
    PRIMARY KEY (_cluster_id, _element_id)
);

-- Note: _element_id uses empty string '' for entity-level overrides
-- to avoid NULL in PK. Or use a composite approach:
--   _element_id = '' for entity overrides
--   _element_id = 'Sift flour' for element overrides
```

Override actions:

| `_override` | Entity effect | Element effect |
|-------------|--------------|----------------|
| `'insert'` | Force re-insert despite `hard_deleted` | Force inclusion in array despite tombstone |
| `'delete'` | Force deletion despite entity still existing | Force removal from array despite still resolved |
| `'clear'` | Clear suppression state — let normal pipeline decide | Clear tombstone — let resolution decide |

The engine reads the overrides table and factors it into the delta CASE:

```sql
-- Entity-level: override a hard_deleted to become insert
WHEN _src_id IS NULL
 AND _se._cluster_id IS NOT NULL
 AND _ov._override = 'insert'
THEN 'insert'

WHEN _src_id IS NULL
 AND _se._cluster_id IS NOT NULL
THEN 'hard_deleted'
```

For elements, the override table would be checked in the
`_element_delta_{mapping}` view similarly.

#### Override lifecycle

The override is a one-time imperative — it must be consumed after the ETL
acts on it, or it will fire again on the next cycle.

**Who writes:** An operator (via admin UI, script, or direct SQL).

**Who reads:** The engine (LEFT JOIN in delta/element-delta views).

**Who clears:** The ETL, after it has acted on the override.

Concrete sequence for a re-insertion override:

```
1. Operator:  INSERT INTO _overrides_system_b
              VALUES ('cluster-X', '', 'insert');

2. Engine:    Delta CASE sees _ov._override = 'insert'
              → emits _action = 'insert' (instead of 'hard_deleted')

3. ETL:       Reads _action = 'insert', performs the insert into system B.
              Then in a single transaction:
                INSERT INTO _synced_entities_system_b (_cluster_id) VALUES ('cluster-X');
                DELETE FROM _overrides_system_b
                 WHERE _cluster_id = 'cluster-X' AND _element_id = '';

4. Next cycle: _src_id IS NULL + _se present + no _ov → 'hard_deleted'
               (but now synced_entities matches reality — system B has the
               entity — so _src_id will NOT be NULL once the source reflects
               the insert. The 'hard_deleted' is transient for one cycle
               until the source table catches up.)
```

**What if the ETL forgets to clear the override?** The override fires again
→ ETL sees `'insert'` for an entity that already exists → should be
idempotent (upsert). Not harmful, but wasteful. The ETL should clear the
override in the same transaction as the insert to ensure atomicity.

**What if the ETL clears the override but fails the insert?** The override
is lost. The operator must re-create it. This argues for clearing the
override *after* confirming the insert succeeded, not before. The ETL
should:

1. Perform the insert
2. On success: update `_written` + delete override (in one transaction)
3. On failure: leave the override in place for retry on next cycle

**Who writes to the overrides table:**

- An admin UI / curation interface (most common)
- An automation script (e.g., "re-insert all entities deleted before date X")
- Direct SQL for operators comfortable with it

This is a second engine-readable table per mapping (alongside
`_written_{mapping}`). Same read-only pattern: engine reads, operator/ETL
writes.

### The admin UI question

The override table is the mechanical primitive. In practice, operators need
a UI to:

- **See** which entities/elements are suppressed / tombstoned state
- **Understand** why (which source deleted, when, what the resolved state
  looks like)
- **Act** by writing to the overrides table

Building this UI is outside the engine's scope, but the engine should
provide the tables and views that such a UI queries. The `_written`,
and `_overrides_{mapping}` tables, combined with the `_delta` and
`_element_delta` views, give a UI everything it needs.

## What needs to happen

### Engine

1. **Model** — add `tombstone_policy: Option<TombstonePolicy>` enum
   (`suppress` | `delete`, default `suppress`). Independent of
   `derive_tombstones` — only requires `written_state`. `propagate` is
   deferred — it requires cross-mapping coordination that adds significant
   complexity. No new `synced_entities` type — `written_state` provides
   the state table.

2. **Delta render** — when `tombstone_policy` is set (implies
   `written_state`), add a CASE branch before the insert branch:
   `WHEN _src_id IS NULL AND _ws._cluster_id IS NOT NULL THEN {action}`.
   The `_written` table is already LEFT JOINed when `written_state` is
   present. UNION ALL orphaned `_written` entities (present in `_written`
   but absent from resolved view) as `'delete'`.

3. **Validation** — `tombstone_policy` requires `written_state` (without
   the `_written` table, the engine can't detect hard deletes).

4. **Schema** — add `tombstone_policy` property to `mapping-schema.json`.

### Documentation

1. **ETL guidance** — document that `_written` serves double duty: noop
   detection (JSONB payload) and hard-delete detection (row existence).
   The ETL writes after each sync; removes after confirmed deletion.
2. **Update propagated-delete example** — explain the relationship between
   soft-delete (engine-native) and hard-delete (engine + written state).

### Open question: element-level deletion policy

The `derive_tombstones` feature currently has implicit deletion-wins
semantics — if any source removes an element, the removal propagates to
all sources' reconstructed arrays. There is no per-mapping policy control.

We should explore whether element-level deletions need the same policy
options as entity-level hard deletes:

| Policy | Entity level (`tombstone_policy`) | Element level (proposed `on_element_delete`) |
|--------|-------------------------------|---------------------------------------------|
| `suppress` | Don't re-insert the entity | Don't re-add the element (current behavior when one source removes) |
| `delete` | Emit `'delete'` for this mapping | Remove element from this mapping's delta array |
| `propagate` | Emit `'delete'` on all mappings | Remove element from ALL mappings' delta arrays (current `derive_tombstones` behavior) |

Currently `derive_tombstones` always behaves as `propagate` — an element
removed by one source is excluded from all sources' arrays. This is the
right default for synchronized data, but may be wrong when sources
contribute different elements (the "asymmetric contributions" footgun
documented in the derive-tombstones example).

A per-mapping `on_element_delete` policy could let the mapping author
choose:

```yaml
mappings:
  - name: blog_cms_recipes
    written_state: true
    derive_tombstones: true
    tombstone_policy: suppress       # entity-level: suppress re-insertion
    on_element_delete: propagate     # element-level: current default
```

This is a separate concern from entity-level hard deletes and should be
designed independently. The main risk is over-engineering — if all current
use cases work with deletion-wins, adding policy options premature. But
the structural parallel with `on_hard_delete` suggests the abstraction is
natural.

**Decision:** Defer `on_element_delete` until a concrete use case demands
non-propagate behavior. Document the parallel so it's easy to add later.

## Relationship to other plans

- **[PROPAGATED-DELETE-PLAN](PROPAGATED-DELETE-PLAN.md)** — handles soft deletes
  via engine views. This plan handles hard deletes via written state.
  Complementary.
- **[ORIGIN-PLAN](ORIGIN-PLAN.md)** — provides `_cluster_id` and insert
  feedback mechanisms (`cluster_members`, `cluster_field`). This plan follows
  the same read-ETL-state pattern via `_written`.
- **[ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md)** — same pattern at the
  array-element level. Uses the *same* `_written` table — element deletion
  extracts arrays from the JSONB payload, entity-level hard-delete detection
  uses row existence. One table, two levels of detection.
- **[SOURCE-REMOVAL-OPTIONS](SOURCE-REMOVAL-OPTIONS.md)** — handles removing
  an entire mapping from the configuration. This plan handles individual
  entities disappearing from a source while the mapping remains active.
- **[ETL-STATE-INPUT-PLAN](ETL-STATE-INPUT-PLAN.md)** — generalises the
  engine-reads-ETL-state pattern. `_written_{mapping}` provides hard-delete
  detection (row existence), target-centric noop detection (JSONB payload),
  and element-level deletion detection (array presence in JSONB). One table,
  three features.
