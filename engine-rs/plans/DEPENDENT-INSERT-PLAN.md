# Dependent Insert (Reference-Gated Inserts)

## Problem

Currently, when a resolved entity has no matching row in a source, the delta
view generates an **insert** for that source. This is full bidirectional sync:
every entity appears in every source that maps to the same target.

This is often undesirable. For example:
- CRM has a customer with an embedded address → ERP shouldn't get a standalone
  `erp_addresses` insert unless an ERP customer actually references it
- A vocabulary/lookup table row shouldn't be inserted into a source unless an
  entity in that source references the vocabulary entry
- A contact shouldn't be inserted into ERP unless an ERP company references it

### Current workaround: `reverse_required`

The `reverse_required: true` flag on a field suppresses inserts where that
field is NULL. This works for the embedded-objects case (CRM address has no
`address_id` so it's suppressed), but it's a per-field heuristic, not a
semantic "only insert if referenced" rule.

## Concept: Reference-Gated Inserts

A mapping-level or target-level declaration that says: "only generate an
insert for this source if another entity in this source references the
inserted entity."

### Semantics

Given mapping M (source S → target T):
- An insert row for S means: entity E exists in T but has no row in S
- **Reference-gated**: only emit the insert if ∃ another row in S (from a
  different mapping M') where M' has a `references: M` field pointing to E

### Example: embedded-objects

```yaml
mappings:
  - name: erp_addresses
    source: erp_addresses
    target: address
    insert_when: referenced  # ← new flag
    fields:
      - source: address_id
        target: id
      - source: street
        target: street
```

With `insert_when: referenced`, the engine checks: does any other mapping
that has `references: erp_addresses` have a row pointing to this entity?
If yes → insert. If no → suppress.

## Design Options

### Option A: `insert_when: referenced` on mapping

Mapping-level flag. The engine:
1. Finds all mappings that have `references: <this_mapping>` fields
2. In the delta view, adds a WHERE EXISTS subquery checking if the referencing
   mapping's reverse view has a row pointing to this entity

**Pros**: Simple declaration, clear semantics
**Cons**: Only works when the reference relationship is explicit in the schema

### Option B: `insert_when: referenced_by: [mapping_names]` on mapping

Explicit list of which mappings must reference the entity.

**Pros**: More control, works for complex reference graphs
**Cons**: Verbose, couples mapping names

### Option C: Target-level `insert_requires_reference: true`

All mappings to this target only get inserts when another mapping references
the target.

**Pros**: DRY — one declaration covers all mappings to the target
**Cons**: May be too broad; some mappings might want unrestricted inserts

### Recommendation: Option A

`insert_when: referenced` is the simplest and most intuitive. It leverages the
existing `references:` graph to determine dependencies automatically.

## Implementation

### SQL Generation

For a mapping with `insert_when: referenced`, the delta view's insert detection
changes from:

```sql
WHEN src._pk IS NULL THEN 'insert'
```

to:

```sql
WHEN src._pk IS NULL AND EXISTS (
  SELECT 1 FROM _reverse_<referencing_mapping> ref
  WHERE ref.<fk_field> = rev.<identity_field>
) THEN 'insert'
```

Where:
- `_reverse_<referencing_mapping>` is the reverse view of a mapping that has
  `references: <this_mapping>`
- `ref.<fk_field>` is the FK field in the referencing mapping
- `rev.<identity_field>` is the identity field in this mapping's reverse view

### Steps

1. **Parse**: Add `insert_when: Option<InsertWhen>` to Mapping model
   (`InsertWhen::Referenced`)
2. **Validation**: Ensure at least one other mapping has `references: <this>`
   when `insert_when: referenced` is set
3. **Render (delta.rs)**: In `action_case()`, when `insert_when == Referenced`,
   add EXISTS subquery check against referencing reverse views
4. **Schema**: Add `insert_when` to `mapping-schema.json`

### Files to Modify

| File | Change |
|------|--------|
| `src/model.rs` | `InsertWhen` enum, field on Mapping |
| `src/parser.rs` | Parse `insert_when` from YAML |
| `src/render/delta.rs` | `action_case()`: EXISTS subquery for referenced check |
| `spec/mapping-schema.json` | Add `insert_when` property |

## Relationship to `reverse_required`

- `reverse_required: true` on a field = "don't insert if THIS field is NULL"
  (necessary condition on field value)
- `insert_when: referenced` on a mapping = "don't insert unless ANOTHER entity
  references this one" (structural dependency)

Both can coexist. `reverse_required` is a quick per-field filter;
`insert_when: referenced` is a structural graph-aware gate.

## Test Plan

- **embedded-objects**: Replace `reverse_required: true` on `address_id` with
  `insert_when: referenced` on the `erp_addresses` mapping. Same result: CRM
  address not inserted into ERP because no ERP customer references it.
- **vocabulary-custom**: Add `insert_when: referenced` to `crm_status_codes`
  mapping → "on-leave" status not inserted because no CRM entity uses it
- **references**: `erp_customer` with `insert_when: referenced` → "wrong.com"
  company not inserted if no ERP contact references it
- New example demonstrating the feature explicitly

## Open Questions

1. Should `insert_when: referenced` check references transitively? (A refs B
   refs C → does C count as referenced?) Probably not in v1.
2. Should this also gate **updates**? Probably not — if a row already exists,
   it should be updated regardless.
3. The `insert_when` name could also support future values like `always`
   (default), `never` (suppress all inserts), or `referenced`.
