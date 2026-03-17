# Multi-value cardinality

Scalar-vs-list cardinality mismatch — CRM stores one phone number, the contact center stores only a JSONB array.

## Scenario

A CRM system keeps a single `phone` column per contact. A contact center system has no scalar phone field at all — only a `phones` JSONB array with multiple numbers (work, mobile, fax). Both represent "phone numbers for this contact" but at fundamentally different cardinalities.

The resolved target bridges this with two levels:
- **`contact.primary_phone`** — a scalar coalesce field for systems that deal with one phone
- **`phone_entry`** — a child target holding the full merged list from both sources

## Key features

- **`expression: "phones->0->>'number'"`** — derives a scalar from CC's JSONB array so it can contribute to `primary_phone` via coalesce
- **`strategy: coalesce` with priorities** — CRM's scalar phone (priority 10) wins when present; CC's derived phone (priority 20) fills in when CRM has none
- **`parent:` + `array:` on `cc_phones`** — extracts JSONB array elements from the contact center source into `phone_entry`
- **`direction: forward_only` on `crm_phones`** — CRM's phone contributes to the shared list without generating reverse/delta rows
- **Bidirectional merge** — CRM's scalar phone appears in CC's phone list (test 1), and CC's first array element fills CRM's empty phone (test 2)

## How it works

1. CRM maps `phone → primary_phone` at priority 10. CC derives `phones->0->>'number' → primary_phone` at priority 20. Coalesce picks CRM when non-null, falls back to CC's first array element.
2. CRM's single phone also contributes to the `phone_entry` list via a separate forward-only mapping. CC's full JSONB array contributes via a nested-array child mapping.
3. Identity resolution links CRM and CC contacts by email, merging them into one entity.
4. In the reverse direction:
   - **CRM** receives the resolved `primary_phone` back. If CRM had no phone, it gets CC's first number as an update.
   - **CC** receives the full merged phone list. CRM's phone appears as a new entry in the array, triggering an update.

## When to use

- One system stores a scalar value, another stores only a list of the same concept (no scalar equivalent)
- The scalar system should receive a fallback value from the list when its own value is missing
- The list system should receive the scalar system's value as a new list entry
- An expression bridges the cardinality gap by deriving a scalar from the array
