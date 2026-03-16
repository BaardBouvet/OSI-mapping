# FK references

**Status:** Done

**Replaces:** LCP heuristic in `find_same_system_mapping()` (removed)

## Problem

When a target field has `references: other_target`, the reverse view must
translate the resolved entity-level reference back to a source-level foreign
key.  The current code uses a longest-common-prefix (LCP) heuristic on
source dataset names to guess which mapping to the referenced target
belongs to the same system.  This is fragile and opaque.

The mapping file should explicitly say **what type a source foreign key
field points to**.

## Decision

Add a `references:` property on individual **field mappings** that names
the mapping whose source identity table should be used for reverse
resolution.  This replaces the LCP heuristic entirely.

## Schema Change

```yaml
mappings:
  - name: crm_contact
    source: { dataset: crm_contacts, primary_key: contact_id }
    target: person
    fields:
      - source: email
        target: email_address
      - source: company_id          # FK in source system
        target: primary_contact     # entity reference on target
        references: crm_company     # ← which mapping to resolve through
```

The value of `references` is the **name** of another mapping definition.
It tells the reverse view: "to translate the entity reference in
`primary_contact` back to a source FK value, use the identity view row
where `_mapping = 'crm_company'`."

### Field mapping schema addition

```json
{
  "references": {
    "type": "string",
    "description": "Name of the mapping whose source identities should be used when translating this entity reference back to a source FK value in the reverse view."
  }
}
```

## How `references` Replaces the Heuristic

### Before (current LCP approach, in `reverse.rs`)

```rust
let same_sys = find_same_system_mapping(
    &mapping.source.dataset,
    ref_target_name,  // from target field definition
    all_mappings,
);
```

The code looks at all mappings whose target is `ref_target_name`, then
picks the one whose `source.dataset` has the longest common prefix with
the current mapping's `source.dataset`.

### After (explicit field-level `references`)

```rust
// fm.references is the mapping name supplied in YAML
if let Some(ref ref_mapping_name) = fm.references {
    let ref_target_name = field_def.and_then(|f| f.references())
        .unwrap_or_else(|| &mapping.target.name()); // fallback
    let id_ref = format!("_id_{ref_target_name}");
    format!(
        "(SELECT ref_local._src_id \
         FROM {id_ref} ref_match \
         JOIN {id_ref} ref_local \
           ON ref_local._entity_id_resolved = ref_match._entity_id_resolved \
         WHERE ref_match._src_id = r.{tgt}::text \
         AND ref_local._mapping = '{ref_mapping_name}' \
         LIMIT 1)"
    )
}
```

The reference target type (`_id_company`) still comes from the target
schema's `references:` on the target field.  The **new** field-level
`references:` on the source field mapping specifies which mapping name to
filter by in the reverse lookup.

## Key Distinction

There are two different `references:` in the system:

| Location | Purpose | Example |
|---|---|---|
| **Target field** (`targets.*.fields.*.references`) | Declares that this target field is an entity reference to another target type | `primary_contact: { references: company }` |
| **Field mapping** (`mappings.*.fields.*.references`) | Tells the reverse view which mapping to use for translating the reference back to a source FK | `references: crm_company` |

The target-level one says *what* the reference points to.
The field-mapping one says *how* to reverse-resolve it for this particular source.

## Implementation Steps

1. **Model**: Add `references: Option<String>` to `FieldMapping` in `model.rs`
2. **Schema**: Add `references` property to field mapping in `mapping-schema.json`
3. **Reverse renderer**: In `render_reverse_view()`, use `fm.references` instead
   of calling `find_same_system_mapping()` when present
4. **Validation**: Warn or error if `fm.references` names a mapping that doesn't
   exist or doesn't map to the expected target type
5. **Remove LCP**: Once all examples use explicit `references`, remove
   `find_same_system_mapping()` and `common_prefix_len()` entirely
6. **Update examples**: Add `references: crm_company` etc. to the
   `reference-preservation` and `references` examples

## Backward Compatibility

During migration, keep the LCP fallback:
- If `fm.references` is set → use it directly
- If `fm.references` is not set and the target field has `references:` →
  fall back to LCP with a validation warning
- Eventually remove the LCP fallback

## Examples

### Single-system (no ambiguity, but explicit is still better)

```yaml
mappings:
  - name: crm_contact
    source: { dataset: crm_contacts, primary_key: contact_id }
    target: person
    fields:
      - source: company_id
        target: primary_contact
        references: crm_company
```

### Multi-system (where LCP would be fragile)

```yaml
mappings:
  - name: sales_contact
    source: { dataset: sales_leads, primary_key: lead_id }
    target: person
    fields:
      - source: account_id
        target: primary_contact
        references: sales_company    # explicit — no naming heuristic needed

  - name: support_contact
    source: { dataset: support_tickets, primary_key: ticket_id }
    target: person
    fields:
      - source: org_id
        target: primary_contact
        references: support_org      # different system, different mapping
```
