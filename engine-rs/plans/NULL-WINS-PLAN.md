# Null wins

**Status:** Maybe

Allow NULL values from authoritative sources to win resolution and propagate
as the golden record value — overriding non-NULL contributions from other
sources.

**Status:** May not implement — the sentinel pattern using existing
`expression` / `reverse_expression` covers the core use case without engine
changes. See [examples/null-propagation](../../examples/null-propagation/)
for the working pattern. This plan remains as a cleaner alternative if the
sentinel approach proves too fragile in practice.

## Problem

Every resolution strategy currently filters or skips NULL values:

| Strategy | NULL handling | SQL mechanism |
|----------|-------------|---------------|
| `identity` | Skipped | `min(field)` ignores NULLs |
| `collect` | Excluded | `FILTER (WHERE field IS NOT NULL)` |
| `coalesce` | Bypassed | `FILTER (WHERE field IS NOT NULL)`; next priority wins |
| `last_modified` | Bypassed | `FILTER (WHERE field IS NOT NULL)`; next timestamp wins |
| `bool_or` | Treated as false | Three-valued logic |
| `expression` | Depends | Custom SQL — can propagate if designed to |

This means: if the highest-priority source deliberately sets a field to NULL
(e.g., "this person's phone number has been removed"), the resolution
ignores that NULL and falls through to a lower-priority source that still
has a stale value. The golden record shows a phone number that the
authoritative source explicitly removed.

### Use cases where NULL should win

**1. GDPR right to erasure.** CRM (priority 1) deletes a customer's phone
number. ERP (priority 2) still has it cached. The golden record should
reflect NULL — the phone is gone. Today: ERP's stale phone wins because
coalesce skips CRM's NULL.

**2. Data correction / cleansing.** A data steward clears an incorrect
date of birth in the master system. Another system still has the bad value.
The cleared field should propagate as NULL, not be backfilled from the
system with bad data.

**3. Opt-out fields.** A customer opts out of marketing, and the source
system clears the `marketing_email` field. Other systems still have the
email. NULL should win — the customer explicitly removed consent.

**4. Decommissioned attributes.** A field is deprecated in the
authoritative system (set to NULL for all records). Lower-priority systems
still populate it. The golden record should transition to NULL.

**5. Sparse sources with authority.** System A is authoritative for
address data. When it says "no address" (NULL), that should override
System B's possibly-outdated address. Today: System B's old address always
wins because it's non-NULL.

### The fundamental tension

NULLs have two semantic meanings in integration:

1. **"I don't know"** — the source doesn't have this data. The correct
   behavior is to skip it and look at other sources. This is what the
   engine does today.

2. **"I know it's empty"** — the authoritative source says this field has
   no value. The correct behavior is to propagate NULL as the resolved
   value.

The engine can't distinguish between these without explicit configuration.

## Existing workaround: sentinel pattern

Before considering engine changes, the sentinel pattern works today:

```yaml
# Authoritative source — convert NULL to sentinel
- source: phone
  target: phone
  priority: 1
  expression: "COALESCE(phone, '__CLEARED__')"
  reverse_expression: "NULLIF(phone, '__CLEARED__')"

# Every other source — strip sentinel on reverse
- source: phone
  target: phone
  priority: 2
  reverse_expression: "NULLIF(phone, '__CLEARED__')"
```

The sentinel (`'__CLEARED__'`) survives the `FILTER (WHERE field IS NOT NULL)`
in coalesce, wins by priority, then gets converted back to NULL in every
reverse view.

### Sentinel drawbacks

1. **Coordination burden.** Every mapping that reads the field needs
   `reverse_expression: "NULLIF(phone, '__CLEARED__')"`. Forgetting one
   leaks the sentinel into a source system.

2. **Analytics pollution.** The resolved view and analytics view show
   `'__CLEARED__'` instead of NULL. Requires `default_expression` on the
   target field to clean it up.

3. **Collision risk.** If real data could contain the sentinel string, the
   pattern breaks. The sentinel must be chosen carefully.

4. **No conditional logic.** The sentinel is unconditional — every NULL from
   the authoritative source becomes `__CLEARED__`, even "don't know" NULLs.

If these drawbacks are acceptable, no engine changes are needed. The
[null-propagation example](../../examples/null-propagation/) demonstrates
this working end-to-end.

## Design: `null_wins` expression on field mappings

If the sentinel pattern proves too fragile, a proper engine feature
eliminates all its drawbacks.

`null_wins` is an optional **expression** (not a boolean) on field mappings
that determines when this source's NULL should be treated as authoritative:

```yaml
fields:
  - source: phone
    target: phone
    priority: 10
    null_wins: "true"           # All NULLs from this source are intentional
```

Or with a condition:

```yaml
  - source: phone
    target: phone
    priority: 10
    null_wins: "updated_at > phone_last_set_at"  # NULL is intentional only when freshly touched
```

When the expression evaluates to true AND the source value is NULL, the
NULL participates in resolution as an authoritative value rather than being
filtered out.

## Implementation: sentinel approach

### The problem with `FILTER (WHERE field IS NOT NULL)`

The current coalesce and last_modified expressions use
`FILTER (WHERE field IS NOT NULL)` to skip NULLs before ordering:

```sql
(array_agg(phone ORDER BY priority ASC)
 FILTER (WHERE phone IS NOT NULL))[1]
```

This unconditionally removes all NULLs. We need NULLs from `null_wins`
sources to survive the filter.

### Solution: sentinel column

Add a boolean column `_nw_{field}` to the forward view that marks
whether this row's NULL should be treated as authoritative:

```sql
-- Forward view for crm_contacts (null_wins: "true" on phone)
SELECT
  ...
  phone::text AS "phone",
  CASE WHEN phone IS NULL AND (true) THEN true ELSE false END AS "_nw_phone",
  ...
```

With a conditional expression:

```sql
-- Forward view for crm_contacts (null_wins: "updated_at > phone_last_set_at")
SELECT
  ...
  phone::text AS "phone",
  CASE WHEN phone IS NULL AND (updated_at > phone_last_set_at) THEN true ELSE false END AS "_nw_phone",
  ...
```

For mappings without `null_wins`, the column is always false:

```sql
-- Forward view for erp_contacts (no null_wins)
SELECT
  ...
  phone::text AS "phone",
  false AS "_nw_phone",
  ...
```

### Resolution: conditional NULL filtering

The resolution aggregation changes from unconditionally filtering NULLs to
keeping NULLs that have `_null_wins = true`:

**Coalesce — today:**
```sql
(array_agg(phone ORDER BY COALESCE(_priority_phone, _priority, 999) ASC)
 FILTER (WHERE phone IS NOT NULL))[1]
```

**Coalesce — with null_wins:**
```sql
(array_agg(phone ORDER BY COALESCE(_priority_phone, _priority, 999) ASC)
 FILTER (WHERE phone IS NOT NULL OR _nw_phone))[1]
```

This keeps NULL rows in the array when their `_nw_phone` flag is true.
If the highest-priority source has `null_wins: true` and the value is NULL,
the NULL is the first element of the array → resolved value is NULL.

**Last_modified — same pattern:**
```sql
(array_agg(phone ORDER BY COALESCE(_ts_phone, _last_modified) DESC)
 FILTER (WHERE phone IS NOT NULL OR _nw_phone))[1]
```

**Identity — no change needed.** Identity fields are match keys — NULL
identity fields already mean "not a match candidate" and should remain
filtered.

**Collect — no change.** Collecting NULLs into an array is meaningless.

**Bool_or — no change.** NULL in boolean OR is already well-defined.

### Fields that need changes

Only `coalesce` and `last_modified` strategies need the sentinel approach.
These are the two priority/timestamp-ordered strategies where "which source
wins?" is the question.

## Data flow

### Forward view

For each field mapping with `null_wins: true`, emit an additional boolean
column:

```sql
CASE WHEN "phone" IS NULL THEN true ELSE false END AS "_nw_phone"
```

For field mappings without `null_wins`, emit `false`:

```sql
false AS "_nw_phone"
```

The column is only emitted when ANY mapping to this target field has
`null_wins: true`.

### Identity view

Pass-through — `SELECT *` from forward views.

### Resolution view

Modified aggregation for coalesce/last_modified (see above). The `_nw_*`
columns are consumed here and NOT passed to the outer SELECT — they're
internal to the aggregation.

### Reverse view

No change. The resolved value is already NULL or non-NULL — the reverse
view uses it directly.

### Delta view

No change. Noop comparison with `IS NOT DISTINCT FROM` already handles
NULL correctly:

```sql
_base->>'phone' IS NOT DISTINCT FROM "phone"::text
-- NULL IS NOT DISTINCT FROM NULL → true (noop)
-- 'old_value' IS NOT DISTINCT FROM NULL → false (update)
```

When the golden record resolves to NULL (because null_wins), and the
source's `_base` still has the old phone → update. This is correct:
the source should be told to delete the phone.

When the source already has NULL and the golden record is NULL → noop.
Also correct.

## Example

```yaml
mappings:
  - name: crm_contacts
    source: { dataset: crm }
    target: contact
    priority: 10
    fields:
      - source: email
        target: email
      - source: phone
        target: phone
        null_wins: "true"           # CRM is authoritative — all NULLs are intentional

  - name: erp_contacts
    source: { dataset: erp }
    target: contact
    priority: 20
    fields:
      - source: email
        target: email
      - source: phone
        target: phone
        # No null_wins — ERP's NULLs mean "don't know"
```

With a conditional:

```yaml
      - source: phone
        target: phone
        null_wins: "updated_at > '2024-01-01'"  # Only recent NULLs are intentional
```

## Interaction with existing features

### `default` / `default_expression`

If a target field has a `default`, the COALESCE wraps the aggregation:

```sql
COALESCE((array_agg(...) FILTER (WHERE phone IS NOT NULL OR _nw_phone))[1], 'N/A')
```

If null_wins produces NULL *and* the field has a default → default wins.
This is correct: the target model says "this field always has a value."
If the author wants NULL to truly propagate even past defaults, don't set
a default.

### `reverse_required`

If a field has both `null_wins: true` (on a mapping) and
`reverse_required: true` (on another mapping), the resolved NULL triggers
a delete for the mapping with `reverse_required`. This is the correct
GDPR pattern: authoritative source clears the field → other systems get
a delete.

### Groups

If a field with `null_wins` is in an atomic group, the NULL applies to the
entire group's DISTINCT ON evaluation. The group resolves atomically — if
the winning row has NULL for the null_wins field, all fields in the group
come from that row (including the NULL).

## Scope of changes

### Model
- `model.rs`: Add `null_wins: Option<String>` to `FieldMapping` (serde
  default None). The value is a SQL expression evaluated in the forward
  view's scope. ~2 lines.
- `mapping-schema.json`: Add `null_wins` string property to field mapping.

### Forward view
- `forward.rs`: For each target field where any contributing mapping has
  `null_wins`, emit `_nw_{field}` boolean column. For mappings with
  `null_wins`, emit
  `CASE WHEN field IS NULL AND ({null_wins_expr}) THEN true ELSE false END`.
  For others, emit `false`. ~15 lines.

### Resolution view
- `resolution.rs`: When `_nw_{field}` columns exist, modify the FILTER
  clause for coalesce and last_modified from
  `WHERE field IS NOT NULL` to `WHERE field IS NOT NULL OR _nw_{field}`.
  ~10 lines.

### Validation
- `validate.rs`: Warn if `null_wins` is set on identity strategy (NULLs in
  identity fields break matching). Error if `null_wins` is on a
  `direction: forward_only` field (no reverse impact, so pointless).
  ~5 lines.

### No changes needed
- Identity view (pass-through)
- Reverse view (uses resolved value as-is)
- Delta view (`IS NOT DISTINCT FROM` handles NULL)
- Analytics view (exposes resolved)

Total: ~30 lines of production code.

## Alternatives considered

### A. Sentinel pattern (no engine changes)

Use `expression: "COALESCE(phone, '__CLEARED__')"` on the authoritative
source and `reverse_expression: "NULLIF(phone, '__CLEARED__')"` on all
mappings. See [examples/null-propagation](../../examples/null-propagation/).

Works today, but: sentinel must be on every reverse mapping for the field,
pollutes analytics view, collision risk with real data, no conditional logic.
This is the **recommended approach** unless these drawbacks become a real
problem.

### C. Mapping-level `null_wins: true`

All fields from this mapping propagate NULLs:

```yaml
mappings:
  - name: crm_contacts
    null_wins: true
```

Rejected: too coarse. A source might be authoritative for phone but not for
name. Per-field control is essential.

### D. Sentinel value in forward view instead of boolean column

Replace NULL with a sentinel string (e.g., `__NULL__`) in forward view so
it survives FILTER. Reverse it back to NULL in resolution.

Rejected: fragile (sentinel might collide with real data), adds complexity
to the round-trip, and sentinel values leak into `_base` comparisons.

### E. Target-level `nullable: true`

Mark a target field as nullable to allow NULL resolution:

```yaml
fields:
  phone:
    strategy: coalesce
    nullable: true
```

Rejected: doesn't distinguish "CRM's NULL is authoritative" from "ERP's
NULL is ignorable." The semantics depend on the source, not the target.

### F. Two-phase resolution (filter then don't-filter)

Run resolution twice: once without NULLs (current), once with NULLs. If
the non-NULL winner differs from the with-NULL winner, pick the with-NULL
result.

Rejected: over-engineered. The sentinel column approach is simpler and
composes with existing GROUP BY without double-pass.

## Open questions

1. **Should `null_wins` interact with `normalize`?** The PRECISION-LOSS-PLAN
   applies normalization to both sides of the noop check. If the resolved
   value is NULL (from null_wins), normalization is a no-op. No interaction
   needed.

2. **Expression validation.** The `null_wins` expression runs in the forward
   view's source scope. It should be validated as a safe snippet per
   [EXPRESSION-SAFETY-PLAN](EXPRESSION-SAFETY-PLAN.md).

3. **Is the sentinel pattern good enough?** The
   [null-propagation example](../../examples/null-propagation/) works today
   with zero engine changes. If sentinel coordination across mappings proves
   manageable, this plan may never need implementation.

4. **Can the ETL state table pattern help here?** No. The
   `synced_entities` / `synced_elements` pattern from
   [ELEMENT-DELETION-PLAN](ELEMENT-DELETION-PLAN.md) and
   [HARD-DELETE-PROPAGATION-PLAN](HARD-DELETE-PROPAGATION-PLAN.md) solves
   lifecycle detection: "was this present before?" That's a temporal
   question. Null-wins is a semantic question: "is this source's NULL
   authoritative?" A field that has been NULL since day one (batch import
   with no phone data) and a field that was intentionally cleared both
   show the same NULL — the state table can't distinguish them. The
   answer depends on the source's authority, not on history. Per-field
   `null_wins` on the mapping is the correct level of abstraction.
