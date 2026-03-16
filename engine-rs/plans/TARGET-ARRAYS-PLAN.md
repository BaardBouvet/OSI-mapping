# Target arrays

**Status:** Planned

Support array-typed fields directly on a target, so that a one-to-many
relationship can live on the parent target instead of requiring a separate
child target.

## Problem

Today every "list of values" requires a separate child target:

```yaml
targets:
  contact:
    fields:
      email: { strategy: identity }
      name:  { strategy: coalesce }

  phone_entry:                         # separate target just for a list
    fields:
      contact_ref: { strategy: coalesce, references: contact }
      phone:       { strategy: identity }
```

This works, but has consequences:

1. **Target model bloat.** Simple lists (phone numbers, tags, email addresses)
   each need their own target with a FK reference and identity strategy. A
   contact with phones, emails, and tags = 4 targets.

2. **Scalar + list duplication.** When a system only deals with one phone, the
   [MULTI-VALUE-PLAN](MULTI-VALUE-PLAN.md) adds a `primary_phone` coalesce
   field alongside the `phone_entry` child target. With array fields, a single
   `phones` field covers both: scalar consumers map one element, list consumers
   map all of them.

3. **No way to coalesce lists.** The `collect` strategy produces an array of
   all distinct contributions, but it operates on a single scalar column — it
   can't merge multiple arrays from different sources into one deduplicated
   list while preserving per-element identity.

4. **Child target overhead.** Each child target generates a full pipeline
   (forward → identity → resolution → reverse → delta). For simple value
   lists, this is heavy machinery.

## Proposed syntax

```yaml
targets:
  contact:
    fields:
      email:
        strategy: identity
      name:
        strategy: coalesce
      phones:
        type: text[]
        element_identity: [value]          # what makes two phone entries "the same"
        strategy: collect                  # merge arrays from all sources
```

### New field properties

| Property | Type | Description |
|----------|------|-------------|
| `type: text[]` (or `X[]`) | string | Marks the field as an array. The base type (`text`, `integer`, etc.) defines element type. |
| `element_identity` | string[] | Which sub-properties identify a unique element. For scalar arrays, use `[value]` (the element itself). For object arrays (future), list the key fields. |

### How `collect` works on arrays

Today `collect` on a scalar field generates:
```sql
array_agg(DISTINCT "field") FILTER (WHERE "field" IS NOT NULL)
```

On an array field, `collect` should:
1. Unnest each source's contributed array
2. Deduplicate by `element_identity`
3. Re-aggregate into a single array

```sql
-- Resolution for phones (array field, collect strategy)
(SELECT array_agg(DISTINCT val ORDER BY val)
 FROM unnest_contributions AS val
 WHERE val IS NOT NULL) AS "phones"
```

The "unnest_contributions" part comes from gathering all contributing
mappings' forward-view arrays into one pool.

## Forward view: array contribution

A mapping contributing to an array field can come from two shapes:

### Scalar source → array target

```yaml
- name: crm_contacts
  source: { dataset: crm }
  target: contact
  fields:
    - source: phone
      target: phones
```

The forward view wraps the scalar in `ARRAY[...]`:
```sql
ARRAY["phone"] FILTER (WHERE "phone" IS NOT NULL) AS "phones"
```

### Array source → array target

```yaml
- name: cc_contacts
  source: { dataset: contact_center }
  target: contact
  fields:
    - source: phone_numbers    # JSONB array in source
      target: phones
```

The forward view converts the JSONB array:
```sql
(SELECT array_agg(el::text) FROM jsonb_array_elements_text("phone_numbers") el) AS "phones"
```

### Nested source → array target

```yaml
- name: cc_contacts
  source: { dataset: contact_center }
  target: contact
  fields:
    - source: phones[].number       # path into nested array
      target: phones
```

The forward view extracts and collects:
```sql
(SELECT array_agg(el->>'number') FROM jsonb_array_elements("phones") el) AS "phones"
```

## Resolution view: merging arrays

The resolution view needs to merge arrays from multiple forward views.
With `strategy: collect` on an array field:

```sql
-- Inner: each forward view contributes its phones array
-- Resolution CTE unnests and re-aggregates:
(SELECT array_agg(DISTINCT phone ORDER BY phone)
 FROM (
   SELECT unnest("phones") AS phone
   FROM _contributions
   WHERE "phones" IS NOT NULL
 ) sub
) AS "phones"
```

For `strategy: coalesce` on an array field, the highest-priority non-null
array wins (no merging — same as scalar coalesce but the value is an array).

For `strategy: identity` on an array field, all contributions must be
identical (same as scalar identity but compared as sorted arrays).

## Reverse view: array → source reconstruction

The reverse view needs to turn the resolved array back into the source's
expected shape.

### Array target → scalar source

CRM has a single `phone` column. The reverse view picks one element:

```yaml
- source: phone
  target: phones
  reverse_expression: "phones[1]"    # first element
```

Or the engine could auto-generate this when it detects array → scalar
mapping (take element at index 1, or `NULL` if empty).

### Array target → array source

The JSONB array reconstruction already exists for nested arrays in
`delta.rs` (`JsonNode` tree). Extend it to handle array-typed target fields:

```sql
-- Reverse: resolved phones[] → source JSONB array
(SELECT jsonb_agg(el) FROM unnest("phones") el) AS "phone_numbers"
```

### Array target → nested source

For sources with nested object arrays (e.g., `phones: [{number: "..."}]`),
the reverse wraps each element:

```sql
(SELECT jsonb_agg(jsonb_build_object('number', el))
 FROM unnest("phones") el) AS "phones"
```

## Delta view: noop comparison for arrays

The noop check needs to compare the `_base` snapshot against the resolved
array. Since `_base` stores the raw source value as text:

**Scalar source:**
```sql
_base->>'phone' IS NOT DISTINCT FROM "phones"[1]::text
```

**Array source:** Compare sorted, like `_osi_text_norm` does for nested JSONB:
```sql
_osi_array_norm(_base->'phone_numbers') IS NOT DISTINCT FROM _osi_array_norm(to_jsonb("phones"))
```

A new `_osi_array_norm` helper (or reuse `_osi_text_norm` extended for arrays)
sorts array elements and normalizes types for stable comparison.

## Multi-value revisited

With array fields, the [MULTI-VALUE-PLAN](MULTI-VALUE-PLAN.md) simplifies:

```yaml
targets:
  contact:
    fields:
      email: { strategy: identity }
      name:  { strategy: coalesce }
      phones:
        type: text[]
        element_identity: [value]
        strategy: collect

mappings:
  - name: crm_contacts
    source: { dataset: crm }
    target: contact
    fields:
      - source: email
        target: email
      - source: name
        target: name
      - source: phone
        target: phones              # scalar → array, auto-wrapped

  - name: cc_contacts
    source: { dataset: contact_center }
    target: contact
    fields:
      - source: email
        target: email
      - source: full_name
        target: name
      - source: phone_numbers
        target: phones              # JSONB array → array, auto-converted
```

No `primary_phone`. No `phone_entry` child target. No dual mappings. CRM
contributes its single phone to the array; CC contributes its full list.
The `collect` strategy merges them. CRM's reverse gets `phones[1]` back.

## Scope of changes

### Model
- `model.rs`: Add array detection to `FieldType` (parse `text[]` → base
  type `text`, is_array = true). Add `element_identity` to `TargetFieldDef`.
- `mapping-schema.json`: Allow `type: "text[]"` pattern, add
  `element_identity` property.

### Forward view
- `forward.rs`: When source is scalar and target is array, wrap in
  `ARRAY[...]`. When source is JSONB array, convert via
  `jsonb_array_elements_text`.

### Resolution view
- `resolution.rs`: For `collect` on array fields, generate
  unnest-dedup-reaggregate SQL instead of simple `array_agg(DISTINCT ...)`.
  For `coalesce`/`identity` on array fields, compare/select as arrays.

### Reverse view
- `reverse.rs`: When reversing array → scalar, generate `field[1]` (or
  custom `reverse_expression`). When reversing array → JSONB array,
  generate `to_jsonb(field)`.

### Delta view
- `delta.rs`: Array noop comparison using sorted normalization.
- Possibly a new `_osi_array_norm` SQL helper function.

### Validation
- `validate.rs`: Validate `element_identity` fields exist or are `[value]`.
  Validate `strategy: identity` on array fields requires sorted comparison.

## Open questions

1. **Object arrays.** Should `element_identity` support compound keys for
   future object-typed array elements? e.g.:
   ```yaml
   addresses:
     type: jsonb[]
     element_identity: [type, zip]
   ```
   This would enable address lists where (type + zip) identifies unique
   entries. Proposal: support it in the schema now, implement scalar-only
   (`[value]`) first.

2. **Ordering.** Should the resolved array preserve insertion order, or
   always sort? Sorting is simpler for noop comparison. If ordering matters,
   we'd need an `order_by` property or positional identity.

3. **Array of objects vs. array of scalars.** Scalar arrays (`text[]`) are
   straightforward. Object arrays (`jsonb[]`) would need per-element field
   mapping — essentially what child targets do today. Proposal: start with
   scalar arrays only.

4. **Coexistence with child targets.** Array fields don't replace child
   targets — child targets remain the right choice for complex entities with
   their own identity, multiple fields, and independent resolution strategies.
   Array fields are for simple value lists.

## Phasing

### Phase 1 — Scalar array fields
- `type: text[]` / `integer[]` etc.
- `element_identity: [value]`
- `strategy: collect` (unnest + dedup + reaggregate)
- `strategy: coalesce` (pick highest-priority array)
- Scalar ↔ array forward/reverse wrapping
- Array noop comparison

### Phase 2 — Object array fields (future)
- `type: jsonb[]`
- `element_identity: [field1, field2]`
- Per-element field decomposition in forward/reverse
- Essentially inlines child target behavior into the parent

## Interaction with other plans

- **MULTI-VALUE-PLAN**: Array fields eliminate the need for `primary_phone` +
  separate `phone_entry` child target.
- **EXPRESSION-SAFETY-PLAN**: Array field expressions are column-level
  snippets — no change to validation rules.
- **PRECISION-LOSS-PLAN**: `normalize` applies per-element when comparing
  arrays.
