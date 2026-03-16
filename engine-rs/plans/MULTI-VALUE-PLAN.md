# Multi-value cardinality

**Status:** Pattern

Handle cardinality mismatch: one system stores a single value while another
stores multiple values for the same semantic field (e.g., one phone number vs.
a list of phone numbers).

## Problem

System A (CRM) has a single `phone` column. System B (contact center) has a
`phones` JSONB array with multiple numbers. Both represent "phone numbers for
this contact" — but at different cardinalities.

CRM's phone should:
1. **Contribute** to the shared multi-value list
2. **Receive** a phone from the list if CRM doesn't have one

This is bidirectional but asymmetric: forward is scalar → list, reverse is
list → scalar.

## Design: `primary_phone` on the target + phone list as child target

The target models both a scalar `primary_phone` field (for systems that only
deal with one phone) and a separate `phone_entry` child target (for the full
list). This avoids cross-target subqueries — each mapping maps to exactly one
target using standard field mappings.

> **Future direction:** [TARGET-ARRAYS-PLAN](TARGET-ARRAYS-PLAN.md) proposes
> supporting array-typed fields directly on the target model, which would let
> the phone list live on `contact.phones[]` instead of requiring a separate
> `phone_entry` target. That would eliminate the need for `primary_phone`
> entirely — a single array field serves both scalar and multi-value consumers.

### Target model

```yaml
targets:
  contact:
    fields:
      email:
        strategy: identity
      name:
        strategy: coalesce
      primary_phone:
        strategy: coalesce

  phone_entry:
    fields:
      contact_ref:
        strategy: coalesce
        references: contact
      phone:
        strategy: identity
```

`primary_phone` is a regular coalesce field — the highest-priority system's
phone wins. No subqueries, no cross-target access.

### Mappings

```yaml
mappings:
  # ── CRM ──────────────────────────────────────────────

  - name: crm_contacts
    source: { dataset: crm }
    target: contact
    priority: 10
    fields:
      - source: email
        target: email
      - source: name
        target: name
      - source: phone
        target: primary_phone

  # CRM contributes its phone to the shared list too
  - name: crm_phones
    source: { dataset: crm }
    target: phone_entry
    direction: forward_only
    fields:
      - source: email
        target: contact_ref
        references: crm_contacts
      - source: phone
        target: phone

  # ── Contact Center ───────────────────────────────────

  - name: cc_contacts
    source: { dataset: contact_center }
    target: contact
    priority: 20
    fields:
      - source: email
        target: email
      - source: full_name
        target: name
      # CC picks one representative phone for the scalar field
      - source: primary_phone
        target: primary_phone

  # Contact center contributes its full phone list
  - name: cc_phones
    source:
      dataset: contact_center
      path: phones
      parent_fields:
        parent_email: email
    target: phone_entry
    fields:
      - source: parent_email
        target: contact_ref
        references: cc_contacts
      - source: number
        target: phone
```

### How it works

**Forward direction:**
- CRM maps `phone` → `primary_phone` (priority 10, wins coalesce)
- CC maps `primary_phone` → `primary_phone` (priority 20)
- Coalesce picks the highest-priority non-null value
- Both contribute to `phone_entry` independently via their `*_phones` mappings

**Reverse direction:**
- CRM receives `primary_phone` back as `phone` — standard field mapping
- If CRM had no phone, it receives whatever the coalesce resolved
- `phone_entry` rows flow back to CC via nested array reverse (existing)

**CRM has a phone, CC has a list + primary:**
- CRM `"555-1234"` → `primary_phone` at priority 10 (wins coalesce)
- Reverse: CRM gets `"555-1234"` back → noop

**CRM has no phone, CC has a list + primary:**
- CC `"555-1234"` → `primary_phone` at priority 20 (only contributor)
- Reverse: CRM gets `"555-1234"` → update (CRM receives a phone)

**CRM has a phone, CC has no primary:**
- CRM `"555-1234"` → `primary_phone` at priority 10 (only contributor)
- Reverse: CRM gets its own phone back → noop

### Why `primary_phone` on the target is acceptable

`primary_phone` is a genuine concept: "the one phone number that represents
this contact." It's not target model pollution — it's a real field that
systems consuming a single phone need. The phone *list* is a different concern
handled by the `phone_entry` child target.

An earlier version of this plan avoided `primary_phone`, using
`reverse_expression` subqueries against internal view names
(`_resolved_phone_entry`, `r."email"`) instead. That leaked engine internals
into the mapping YAML — see [EXPRESSION-SAFETY-PLAN](EXPRESSION-SAFETY-PLAN.md).

The tradeoff:
- **Pro:** No cross-target subqueries, no internal name leaks, standard
  coalesce resolution, works with existing engine features
- **Con:** Target model has both `primary_phone` and a `phone_entry` list —
  slight redundancy. Eliminated once array fields are supported
  (see [TARGET-ARRAYS-PLAN](TARGET-ARRAYS-PLAN.md))

## No engine changes needed

This pattern uses existing features:
- `priority:` + `strategy: coalesce` — already supported
- `direction: forward_only` — already supported
- `source.path` + `parent_fields` — already supported for nested arrays
- Dual mappings from the same source — already supported

## When to use which

| Scenario | Pattern |
|----------|---------|
| Scalar + list of same concept | `primary_phone` coalesce field + child target for list |
| Scalar only contributes | Single `forward_only` mapping to child target |
| Both systems have lists | Both use `source.path` to same child target — standard nested arrays |
| List has primary concept | Source provides `primary_phone` directly (or via `expression`) |
| Want array on target directly | See [TARGET-ARRAYS-PLAN](TARGET-ARRAYS-PLAN.md) |

## Risk

**None.** This is a mapping pattern using existing engine features. No code
changes required.
