# Precision loss

**Status:** Planned

Handle lossy transformations between systems: numeric precision reduction,
string truncation, case folding, date format degradation, and other situations
where one system fundamentally cannot represent the full fidelity of a value.

## Problem

The delta view detects changes by comparing `_base->>'source_col'` (raw source
snapshot) against the reverse-projected resolved value cast to `::text`. When
the round-trip is lossless, equal values produce `'noop'`. But when a system
has lower precision, the resolved "golden" value pushed back doesn't match
what the system can actually store — producing false updates on every sync.

### Concrete scenarios

**1. Numeric precision** — ERP stores price as `numeric(10,2)`, warehouse
stores it as `integer`. Golden record resolves `12.50` from ERP. Pushed to
warehouse reverse: `12.50` ≠ `_base->>'price'` = `12` → false update every
sync. Warehouse can't store `12.50` — it'll write `13` (or `12`), next sync
that doesn't match `12.50` → false update again. Infinite loop.

**2. String truncation** — CRM allows 200-char names, legacy system has a
40-char `VARCHAR(40)`. Golden name is "International Business Machines
Corporation" (51 chars). Pushed to legacy, stored truncated as
"International Business Machines Corporat". Next sync: `_base` = truncated ≠
resolved = full → false update. Infinite loop.

**3. Case folding** — One system stores uppercase-only (`ALICE SMITH`),
another stores mixed case (`Alice Smith`). Without case normalization, the
reverse comparison always sees a difference.

**4. Date format loss** — Source stores `DD/MM/YY` (2-digit year). Forward
expression converts via `TO_DATE`. But `TO_DATE('15/06/85', 'DD/MM/YY')`
is ambiguous (1985 or 2085?). Even if correct, `TO_CHAR(date, 'DD/MM/YY')`
round-trips fine — but if the target type forces a higher-precision date
format and the original can't be recovered exactly, false updates occur.

**5. Whitespace / special characters** — One system trims trailing whitespace,
another preserves it. The values are semantically identical but textually
different.

**6. Encoding/charset** — One system strips diacritics ("Müller" → "Muller").
Round-trip can never match.

## Current mechanics

Today's noop check is:
```sql
_base->>'source_col' IS NOT DISTINCT FROM "source_col"::text
```

Where `"source_col"` in the reverse view is either:
- A direct target field reference (e.g., `r."name"`)
- A `reverse_expression` result (e.g., `TO_CHAR(date_of_birth, 'DD/MM/YY')`)

This is a **textual equality** check on both sides. It works perfectly when the
`expression` / `reverse_expression` pair is lossless. It fails when the system
can't store the full-fidelity value.

## Key insight

The problem isn't in the resolved value — it's in the **comparison**. The delta
asks "did anything change?" but should be asking "did anything change **within
the resolution this system can represent**?"

A system that stores integers can't tell the difference between `12.49` and
`12.51` — both are `12` (or `13`). The noop check should compare at the
system's resolution, not at full fidelity.

## Design: `normalize` on field mappings

A new optional property on field mappings that defines how to normalize both
sides of the noop comparison:

```yaml
- source: price
  target: price
  normalize: "round(%s, 0)::integer::text"
```

The `%s` placeholder is replaced with the value being compared. The same
normalization is applied to **both sides** of the delta comparison:

```sql
-- Instead of:
_base->>'price' IS NOT DISTINCT FROM "price"::text

-- With normalize:
round((_base->>'price')::numeric, 0)::integer::text
IS NOT DISTINCT FROM
round("price"::numeric, 0)::integer::text
```

Both sides are reduced to the system's resolution before comparison. If the
system stores `12` and the resolved value is `12.50`, both normalize to `13`
(or `12` depending on rounding) — but as long as both sides use the same
normalization, the comparison is consistent.

### Why `normalize` and not `compare_as`

A declarative `compare_as: integer` would cover 80% of cases but can't handle
truncation with a specific length, case folding, or custom normalization.
A SQL expression is maximally flexible while remaining a single property.

### Why not on the target field

The normalization is specific to a **source system's limitation**, not to the
target's golden record. Different systems mapping to the same target field
may have different precision limitations. CRM has `numeric(10,2)`, ERP has
`numeric(10,4)`, legacy has `integer` — each mapping needs its own normalize.

## Examples

### Numeric precision

System A has `numeric(10,2)`, System B has `integer`:

```yaml
mappings:
  - name: system_a
    source: { dataset: system_a }
    target: product
    fields:
      - source: price
        target: price
        priority: 1

  - name: system_b
    source: { dataset: system_b }
    target: product
    fields:
      - source: price
        target: price
        normalize: "round(%s::numeric, 0)::integer::text"
```

System B's noop check: `round(_base->>'price'::numeric, 0)::integer::text IS
NOT DISTINCT FROM round("price"::numeric, 0)::integer::text`. Both sides
truncated to integer precision — no false updates.

System A has no `normalize` — its full-precision comparison is unchanged.

### String truncation

```yaml
  - name: legacy
    source: { dataset: legacy }
    target: customer
    fields:
      - source: name
        target: name
        normalize: "left(%s, 40)"
```

Noop check: `left(_base->>'name', 40) IS NOT DISTINCT FROM left("name"::text, 40)`.
If the golden record name is longer than 40 chars, both sides are truncated to
40 before comparison — no false update. The legacy system will write the
truncated version, and that's recognized as "expected" loss.

### Case folding

```yaml
  - name: uppercase_system
    source: { dataset: legacy_erp }
    target: contact
    fields:
      - source: name
        target: name
        normalize: "upper(%s)"
```

Noop check: `upper(_base->>'name') IS NOT DISTINCT FROM upper("name"::text)`.
"ALICE SMITH" vs "Alice Smith" → both normalize to "ALICE SMITH" → noop.

### Combined normalization

```yaml
  - source: product_name
    target: name
    normalize: "upper(left(%s, 50))"
```

Uppercase AND truncated to 50 chars. Normalizations compose naturally
because `%s` is just substituted into a SQL expression.

## Implementation

### Phase 1 — Model

Add to `FieldMapping`:
```rust
pub normalize: Option<String>,  // SQL expr with %s placeholder
```

### Phase 2 — Delta noop generation (`delta.rs`)

In `action_case()`, when building `noop_parts`, apply normalization:

```rust
let lhs = format!("_base->>'{}'", sql_escape(src));
let rhs = format!("{}::text", qi(src));

let (lhs_norm, rhs_norm) = if let Some(ref norm) = fm.normalize {
    (
        norm.replace("%s", &lhs),
        norm.replace("%s", &rhs),
    )
} else {
    (lhs, rhs)
};

Some(format!("{lhs_norm} IS NOT DISTINCT FROM {rhs_norm}"))
```

### Phase 3 — Schema + validation

- Add `normalize` to field mapping schema (type: string).
- Validate: if present, must contain `%s` placeholder.
- Warning: `normalize` without `reverse_expression` — the mapping author likely
  also needs to consider forward/reverse expression interplay.

### Phase 4 — Example

Create `examples/precision-loss/` with:
- System A: `numeric(10,2)` price, full-length name
- System B: `integer` price, `VARCHAR(40)` name
- Tests showing that without `normalize`, false updates occur (or rather,
  showing that WITH `normalize`, the system correctly detects noop)

## Interaction with existing mechanics

### `expression` / `reverse_expression`

`normalize` is independent of `expression`/`reverse_expression`. The expression
pair handles format conversion (date format, phone formatting). `normalize`
handles the precision check.

They can coexist:
```yaml
- source: dob
  target: date_of_birth
  expression: "TO_DATE(dob, 'DD/MM/YY')"
  reverse_expression: "TO_CHAR(date_of_birth, 'DD/MM/YY')"
  normalize: "upper(%s)"  # hypothetical: normalize case of text representation
```

### `_osi_text_norm`

The existing `_osi_text_norm` function handles type normalization for JSONB
nested arrays (integer `2` vs string `"2"`). `normalize` handles semantic
normalization at the field level. They don't overlap — `_osi_text_norm` only
applies to nested JSONB comparisons, `normalize` applies to flat field
comparisons in the delta's `_base` check.

### `direction: forward_only`

Fields with `forward_only` direction don't participate in noop detection
(no reverse comparison). `normalize` is meaningless on forward_only fields.
Validation should warn if both are set.

## Alternatives considered

### A. `compare_type` / `compare_as` — declarative type-based normalization

```yaml
- source: price
  target: price
  compare_as: integer
```

Generates `%s::integer::text` automatically. Covers numeric precision and
basic type coercion. Doesn't cover string truncation, case folding, or
custom normalization. Could be added as **sugar** later, desugaring to
`normalize: "%s::integer::text"`.

### B. `tolerance` — numeric range comparison

```yaml
- source: price
  target: price
  tolerance: 0.5
```

Generates `abs((_base->>'price')::numeric - "price"::numeric) <= 0.5`.
Only works for numeric fields. Doesn't compose with other normalizations.
And it changes the comparison semantics (range vs equality) which affects
what constitutes a change. Too narrow.

### C. Normalization on target field — applied to all systems

```yaml
targets:
  product:
    fields:
      price:
        strategy: coalesce
        normalize: "round(%s, 0)"
```

Problem: normalization is a property of the system's limitations, not the
target's golden record. Different systems have different precision. Per-system
control (on the field mapping) is the right granularity.

### D. Do nothing — let `reverse_expression` handle it

The mapping author could write:
```yaml
- source: price
  target: price
  expression: "price"                    # identity
  reverse_expression: "round(price, 0)"  # truncate on way back
```

This changes the reverse-projected value, which means the **resolved** value
would be pushed as truncated. But that defeats the purpose — the golden record
should have full precision. The system should push full precision, **accept**
that the system can't store it, and recognize the difference as "expected loss"
rather than "change."

Also: `reverse_expression` affects what the reverse view emits as the
**desired value** for the target system. Using it for normalization would
make the delta say "write 12" instead of "write 12.50" — losing information.
`normalize` keeps the reverse value at full precision while accepting the
comparison at reduced precision.

## Edge cases

**What if the normalized values match but raw values don't?**
That's the intended behavior. `normalize` says "this difference is within
the expected loss — don't flag it as a change."

**What if normalize is asymmetric?**
It shouldn't be — the same expression is applied to both `_base` and the
reverse value. If the normalize expression produces different results for
the same semantic value on each side, it's a bug in the normalize expression.

**What about nested array fields?**
Nested array noop detection uses `_osi_text_norm` on the full JSONB object.
Per-field `normalize` doesn't apply there. If nested fields have precision
issues, the mapping author would need to handle it via `reverse_expression`
on the nested field mapping, or a future enhancement to `_osi_text_norm`
that supports per-field normalization.

**What about NULL handling?**
`normalize` wraps both sides, but if `_base->>'col'` is NULL, the expression
must handle it. SQL functions like `upper(NULL)`, `left(NULL, 40)`,
`round(NULL::numeric, 0)` all return NULL, so `IS NOT DISTINCT FROM` still
works correctly. No special handling needed.

## Forward-direction implications

`normalize` only affects the delta noop check — it does NOT change:
- What the forward view emits (full fidelity value)
- What the resolution view resolves (golden record at full precision)
- What the reverse view pushes back (full fidelity desired value)

The system still **tries** to write the full value. The normalize only says
"when checking if it worked, compare at this resolution."

This is philosophically correct: the mapping declares "here is the value you
should have" (reverse view), and separately "here is how to check if you
stored it acceptably" (normalize). The system may store a degraded version, and
that's acknowledged as acceptable rather than flagged as a change.

## The echo problem: `normalize` alone isn't enough

`normalize` solves half the problem — it prevents false delta updates. But it
doesn't prevent a low-precision source from **winning resolution** and
overwriting the high-precision original.

### The scenario

1. System A (decimal, `last_modified`): price = `1.6`, updated March 14
2. Sync → System B (integer) receives `1.6`, stores `2` (rounded)
3. `normalize` on B's mapping → noop in delta ✓ (correct so far)
4. User re-saves record in B → timestamp updates to March 15, value still `2`
5. Next sync: B is newer → resolution picks `2` as golden record
6. System A gets pushed `2` → its original `1.6` is lost

The rounded echo from B propagated back through resolution and destroyed
the precision in A. `normalize` can't help because the problem is in the
**resolution** stage, not the delta stage.

### Idea: echo-aware resolution

A low-precision source should only win resolution if its value represents a
**meaningfully different assertion**, not just a rounded echo. The heuristic:

> If `normalize(B_value) == normalize(A_value)`, B's value is just an echo of
> A's value at B's precision → B shouldn't win, even if newer.

Using the `normalize` expression already declared on the mapping:

| A value | B value | `normalize` = `round(%s, 0)` | B wins? |
|---------|---------|------------------------------|---------|
| 1.6     | 2       | round(1.6)=2, round(2)=2     | No — echo |
| 1.2     | 2       | round(1.2)=1, round(2)=2     | Yes — real change |
| 49.99   | 50      | round(49.99)=50, round(50)=50 | No — echo |
| 10.00   | 15      | round(10)=10, round(15)=15   | Yes — real change |

This elegantly reuses `normalize` for both purposes:
1. Delta: "are these values the same at my precision?" (noop detection)
2. Resolution: "is this value meaningfully different from the competitor?"

### How it would work in resolution

The current `last_modified` resolution generates:
```sql
(array_agg("price" ORDER BY timestamp DESC NULLS LAST)
 FILTER (WHERE "price" IS NOT NULL))[1]
```

With echo-aware resolution, the low-precision source's value is excluded when
it's indistinguishable from a higher-precision source's value at the low
source's resolution. Conceptually:

```sql
-- Don't let B's value participate if it's just a rounded echo of A's
(array_agg("price" ORDER BY timestamp DESC NULLS LAST)
 FILTER (WHERE "price" IS NOT NULL
   AND NOT (is_echo_of_higher_precision_value))
)[1]
```

### Why this is hard

The resolution view aggregates across ALL contributing sources with a single
`array_agg`. It doesn't know which source has which `normalize` expression.
The `normalize` is on a **field mapping** (source-specific), but the resolution
view operates on identity-merged rows from ALL sources.

To implement echo detection, the resolution would need to:
1. Know each contributing row's mapping (available via `_mapping` column)
2. Know each mapping's `normalize` expression for this field
3. Compare each row's value against all higher-precision candidates
4. Exclude echoes from the ordering

This requires either:
- A multi-pass CTE (compare values pairwise before aggregation)
- A custom aggregate function
- Window functions to detect echo relationships

All are significantly more complex than the current single-pass `array_agg`.

### Simpler alternative: use `coalesce` + priority when precision differs

If System A has higher precision, give it higher priority:

```yaml
  - name: system_a
    source: { dataset: system_a }
    target: product
    fields:
      - source: price
        target: price
        priority: 1           # always wins

  - name: system_b
    source: { dataset: system_b }
    target: product
    fields:
      - source: price
        target: price
        priority: 2           # only wins when A has no value
        normalize: "round(%s::numeric, 0)::integer::text"
```

Result: A always wins resolution → golden has full precision → B gets the
precision value, stores rounded, `normalize` detects noop → stable.

If someone changes the value in B (from 2 to 5), next sync:
- `normalize` sees `round(5,0) ≠ round(2,0)` → B gets `_action = update`
  with the golden value (which is still A's since A has priority)... but wait,
  B's change doesn't propagate to A because A has priority.

Hmm — that's a problem too. If B can never win, there's no way to correct a
value from B. The user wants: "B can win, but only when it's a real change."

### Practical recommendation

**Phase 1** (implement now): `normalize` for delta noop detection. This
solves the most common symptom (false updates) and is simple to implement.

**Phase 2** (implement if needed): echo-aware resolution for `last_modified`.
This solves the deeper problem but is significantly more complex. The approach:

Add `_normalize_{field}` as a computed column in the forward view when
`normalize` is declared. This pre-computes the normalized value so the
resolution view can use it:

```sql
-- Forward view for System B
round("price"::numeric, 0)::integer::text AS "_normalize_price"
```

Forward views without `normalize` emit the raw value:
```sql
-- Forward view for System A
"price"::text AS "_normalize_price"
```

Resolution then uses a two-pass approach:

```sql
-- CTE 1: rank values, deduplicating by normalized form
_dedup AS (
  SELECT *,
    ROW_NUMBER() OVER (
      PARTITION BY _entity_id_resolved, "_normalize_price"
      ORDER BY _last_modified DESC NULLS LAST
    ) AS _echo_rank
  FROM "_id_product"
  WHERE "price" IS NOT NULL
)
-- CTE 2: from non-echo values, pick the newest
SELECT
  (array_agg("price" ORDER BY _last_modified DESC NULLS LAST)
   FILTER (WHERE _echo_rank = 1))[1] AS "price"
FROM _dedup
```

When two rows have the same `_normalize_price`, only the first (newest) survives
the dedup. If B's `2` normalizes to `2` and A's `1.6` also normalizes to `2`,
they share a partition. Only the newest row survives — but they'd produce the
same normalized value anyway, so we can pick A's `1.6` (the one with finer
precision). Actually, the ROW_NUMBER tiebreak should prefer **higher precision**
(lower `_normalize_price` length? or the one WITHOUT a `normalize` expression).

This needs more thought on the exact tiebreaker. Deferring to Phase 2.

### Summary of the two-problem model

| Problem | Where | Solution | Phase |
|---------|-------|----------|-------|
| False delta updates | Delta view | `normalize` on noop comparison | 1 |
| Echo wins resolution | Resolution view | Echo-aware dedup in resolution | 2 |

Phase 1 is standalone useful — it prevents the infinite false-update loop.
Phase 2 prevents precision degradation of the golden record. Both use the same
`normalize` expression declared on the field mapping.

## Risk

Low. Single new optional property on FieldMapping. Only affects the
`action_case()` function in delta.rs. All existing examples have no
`normalize` and behave identically. No breakage possible.
