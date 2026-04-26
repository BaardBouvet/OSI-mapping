# v2 Prototype Examples

**Status:** Draft

Three existing examples translated to the proposed v2 schema, to validate
that the simplifications proposed in [v2-spec-draft.md](v2-spec-draft.md)
actually feel better in practice. If they don't, the spec needs revision.

The originals are linked for side-by-side reading. Tests blocks are abbreviated
where unchanged.

## Example 1 — hello-world

Original: [examples/hello-world/mapping.yaml](../../examples/hello-world/mapping.yaml)

### v2

```yaml
version: "2.0"
description: >
  Two systems, one shared contact, synced by email.
  Simplest possible mapping showing identity matching, conflict resolution,
  and bidirectional sync.

sources:
  crm:
    primary_key: id
  erp:
    primary_key: id

targets:
  contact:
    identity:
      - email
    fields:
      email: { strategy: coalesce }
      name:  { strategy: coalesce }

mappings:
  - name: crm
    source: crm
    target: contact
    fields:
      - { source: email, target: email }
      - { source: name,  target: name, priority: 1 }

  - name: erp
    source: erp
    target: contact
    fields:
      - { source: contact_email, target: email }
      - { source: contact_name,  target: name, priority: 2 }

tests:
  - description: "Shared contact — CRM name wins (priority 1)"
    input:
      crm: [{ id: "1", email: "alice@example.com", name: "Alice" }]
      erp: [{ id: "100", contact_email: "alice@example.com", contact_name: "A. Smith" }]
    expected:
      erp:
        updates:
          - { id: "100", contact_email: "alice@example.com", contact_name: "Alice" }
```

### What changed

- Target gained an `identity:` block; `email` is no longer `strategy: identity`
  in `fields:`. Now `email: { strategy: coalesce }` — it is mapped and
  written like any other field.
- Strategy declarations use object form throughout (`{ strategy: coalesce }`
  instead of the v1 string shorthand `coalesce`). Verbosity cost paid for
  consistency.

### Verdict

Slight loss for the trivial case (3–4 extra characters per field, one extra
`identity:` block). Identity-as-target-block is more honest about *where*
identity lives, and the consistent object form scales better to mappings
where fields actually need additional properties — reader never has to
decide "is this a string or an object?"

---

## Example 2 — composite-keys

Original: [examples/composite-keys/mapping.yaml](../../examples/composite-keys/mapping.yaml)

### v2

```yaml
version: "2.0"
description: >
  Order lines identified by (order_ref, line_number) tuple.
  Orders merge via external reference.

sources:
  crm_line_items: { primary_key: line_item_id }
  crm_orders:     { primary_key: order_number }
  erp_order_lines: { primary_key: [order_id, line_no] }
  erp_orders:     { primary_key: order_id }

targets:
  purchase_order:
    identity:
      - external_order_ref
    fields:
      external_order_ref: { strategy: coalesce }
      order_date:         { strategy: coalesce }
      customer_name:      { strategy: coalesce }
      status:             { strategy: last_modified }

  order_line:
    identity:
      - [order_ref, line_number]
    fields:
      order_ref:    { strategy: coalesce, references: purchase_order }
      line_number:  { strategy: coalesce }
      product_name: { strategy: coalesce }
      quantity:     { strategy: coalesce, type: numeric }
      unit_price:   { strategy: coalesce, type: numeric }

mappings:
  - name: erp_orders
    source: erp_orders
    target: purchase_order
    fields:
      - { source: external_ref,   target: external_order_ref }
      - { source: order_date,     target: order_date }
      - { source: customer_name,  target: customer_name }
      - { source: status,         target: status, last_modified: updated_at }

  - name: erp_order_lines
    source: erp_order_lines
    target: order_line
    fields:
      - { source: order_id,       target: order_ref, lookup_source: erp_orders }
      - { source: line_no,        target: line_number }
      - { source: product_name,   target: product_name }
      - { source: quantity,       target: quantity }
      - { source: unit_price,     target: unit_price }

  - name: crm_orders
    source: crm_orders
    target: purchase_order
    fields:
      - { source: external_ref,   target: external_order_ref }
      - { source: order_date,     target: order_date }
      - { source: customer_name,  target: customer_name }
      - { source: status,         target: status, last_modified: last_updated }

  - name: crm_line_items
    source: crm_line_items
    target: order_line
    last_modified: modified_at
    fields:
      - { source: order_ref,    target: order_ref, lookup_source: crm_orders }
      - { source: product_name, target: product_name, last_modified: modified_at }
      - { source: quantity,     target: quantity }
      - { source: price,        target: unit_price }

# tests block identical to v1 — unchanged
```

### What changed

- `purchase_order` had `external_order_ref: { strategy: identity }`. Now
  `identity: [external_order_ref]` at the target.
- `order_line` had no `link_group` declared (the v1 example uses *implicit*
  identity via `references` chain). In v2 the composite identity becomes
  explicit: `identity: [[order_ref, line_number]]`. **This is a real
  improvement** — the identity story for `order_line` was previously
  ambiguous (was it identified by `order_ref` alone? by source PK? by both?).
- v1 source `primary_key: [order_id, line_no]` for `erp_order_lines` was
  doing double duty as composite identity. In v2 it remains the source PK
  (for change detection) but the *entity* identity is `(order_ref,
  line_number)` — the target-level concept, not source-shaped.

### Verdict

Identity becomes self-documenting. The reader does not have to chase
`references` and `primary_key` to figure out what makes an `order_line`
unique. This is the case the simplification was designed for.

---

## Example 3 — relationship-mapping

Original: [examples/relationship-mapping/mapping.yaml](../../examples/relationship-mapping/mapping.yaml)

This is the heaviest user of `link_group` in the repo. If v2 makes this one
nicer, the simplification pays for itself.

### v2

```yaml
version: "2.0"
description: >
  Maps many-to-many (CRM associations) to one-to-many (ERP foreign keys)
  through shared target entities.

sources:
  crm_associations: { primary_key: [company_id, contact_id, relation_type] }
  crm_companies:    { primary_key: company_id }
  crm_contacts:     { primary_key: contact_id }
  erp_companies:    { primary_key: id }
  erp_contacts:     { primary_key: id }

targets:
  company:
    identity: [email]
    fields:
      name:  { strategy: last_modified }
      email: { strategy: coalesce }

  person:
    identity: [email]
    fields:
      name:  { strategy: last_modified }
      email: { strategy: coalesce }

  company_person_association:
    identity:
      - [person_id, relation_type]
    fields:
      company_id:    { strategy: coalesce, references: company }
      person_id:     { strategy: coalesce, references: person }
      relation_type: { strategy: coalesce }

mappings:
  - name: crm_companies
    source: crm_companies
    target: company
    fields:
      - { source: company_name,  target: name, last_modified: updated_at }
      - { source: company_email, target: email }

  - name: crm_contacts
    source: crm_contacts
    target: person
    fields:
      - { source: contact_name,  target: name, last_modified: updated_at }
      - { source: contact_email, target: email }

  - name: crm_associations
    source: crm_associations
    target: company_person_association
    fields:
      - { source: company_id,    target: company_id, lookup_source: crm_companies }
      - { source: contact_id,    target: person_id,  lookup_source: crm_contacts  }
      - { source: relation_type, target: relation_type }

  - name: erp_companies
    source: erp_companies
    target: company
    fields:
      - { source: name,  target: name, last_modified: modified_at }
      - { source: email, target: email }

  - name: erp_contacts_person
    source: erp_contacts
    target: person
    fields:
      - { source: name,  target: name, last_modified: modified_at }
      - { source: email, target: email }
```

### What changed

The v1 association target was:

```yaml
# v1
company_person_association:
  fields:
    company_id:    { strategy: coalesce, references: company }
    person_id:     { strategy: identity, references: person, link_group: relationship }
    relation_type: { strategy: identity, link_group: relationship }
```

The reader had to understand:
- `link_group: relationship` groups `person_id` and `relation_type` into a
  composite identity tuple
- `strategy: identity` on each grouped field is required (not optional) for
  `link_group` to work
- `company_id` is *not* part of identity even though it is also a
  `references` field — easy to misread

The v2 form:

```yaml
# v2
company_person_association:
  identity:
    - [person_id, relation_type]
  fields:
    company_id:    { strategy: coalesce, references: company }
    person_id:     { strategy: coalesce, references: person }
    relation_type: { strategy: coalesce }
```

Now identity is a single short block at the top of the target. The fields
block expresses how to merge each value, independently. The reader never
mixes "what makes this entity unique" with "what to do when sources
disagree."

Mapping-level FK references are renamed `lookup_source:` (`references:` was
overloaded — the target-level one is type info, the mapping-level one is
namespace info; they are different concepts and now have different names).

### Verdict

This is the example where the simplification clearly wins. The original is
not *wrong*, but it conflates two concerns. The v2 form separates them. The
diff stat is roughly the same; the conceptual diff is much cleaner.

---

## Sanity-check summary

| Example | v1 readability | v2 readability | Net |
|---|---|---|---|
| hello-world | Already minimal | One extra block | Slight loss, acceptable |
| composite-keys | Identity scattered, implicit | Identity explicit and central | Clear win |
| relationship-mapping | `link_group` glue everywhere | Identity isolated, fields clean | Clear win |

The pattern: **the simpler the example, the smaller the v2 win.** That is the
right tradeoff. The complexity tax in v1 falls hardest on the composite/MDM
cases that are already the hardest to reason about. v2 takes weight off
exactly those.
