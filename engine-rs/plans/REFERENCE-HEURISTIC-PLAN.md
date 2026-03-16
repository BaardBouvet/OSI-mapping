# Reference heuristic (LCP)

**Status:** Superseded

## The Problem

When a target field has `references: other_target`, the reverse view must
translate the resolved entity-level reference back to a source-level foreign
key value.  Each system uses its own ID namespace, so the reverse view needs
to know *which mapping to the referenced target* belongs to the *same system*
as the current mapping.

Example: `crm_contact → person` has `primary_contact: { references: company }`.
The reverse view for `crm_contact` needs to look up the company's identity in
the same CRM namespace — i.e. via `crm_company`, not `erp_customer`.

## Current Heuristic: Longest Common Prefix (LCP)

`find_same_system_mapping()` picks the mapping to the referenced target whose
`source.dataset` shares the longest common prefix with the current mapping's
`source.dataset`.

```
current: crm_contact    candidates: crm_company (prefix 4: "crm_"), erp_customer (prefix 0)
→ picks crm_company ✓

current: erp_contact    candidates: crm_company (prefix 0), erp_customer (prefix 4: "erp_")
→ picks erp_customer ✓
```

### Why It Works (Barely)

Most real-world configurations follow a naming convention:
- `crm_contacts`, `crm_companies`, `crm_orders` — all CRM
- `erp_employees`, `erp_customers`, `erp_invoices` — all ERP

The common prefix `crm_` or `erp_` correctly groups them.

### Why It Smells

1. **Fragile naming dependency.** If someone names datasets `sales_crm` and
   `support_crm`, the prefix `s` groups `sales_crm` with `source_a` rather
   than `support_crm`.

2. **Ambiguous edge cases.** `crm_contacts` vs `crm_clients` — both have
   the same prefix length against `crm_companies`.  The `max_by_key` tiebreaker
   is arbitrary (first in iterator order).

3. **Fails for mixed naming.** Dataset names like `contacts` and `companies`
   have prefix `co` — not meaningful at all.

4. **Not explicit.** The user has to "hope" their naming convention is
   consistent.  There's no way to override or verify.

## Proposed Fix: Explicit System Tag

Add an optional `system:` property to source or mapping definitions:

```yaml
sources:
  crm_contacts:
    primary_key: contact_id
    system: crm           # ← explicit
  crm_companies:
    primary_key: company_id
    system: crm
  erp_employees:
    primary_key: employee_id
    system: erp
```

Or at mapping level:

```yaml
mappings:
  - name: crm_contact
    source: { dataset: crm_contacts }
    system: crm           # ← explicit
    target: person
```

Resolution algorithm change:
1. If both mappings have the same `system:` tag → match.
2. If only one side has `system:` → fall back to LCP (or error).
3. If neither side has `system:` → fall back to LCP.

This makes the grouping explicit when needed while remaining backward-compatible.

## Alternative: Convention-Based System Detection

Strip a common suffix/prefix pattern to extract the system name automatically:
- Split dataset name on `_` → first token is the system.
- `crm_contacts` → system `crm`, `erp_employees` → system `erp`.

This is slightly less fragile than raw LCP but still relies on naming conventions.
It could be the default when `system:` is not specified.

## Recommendation

1. Add `system: Option<String>` to `SourceMeta` (in model.rs).
2. In `find_same_system_mapping()`, prefer exact `system` match over LCP.
3. Keep LCP as fallback for backward compatibility.
4. Add a validator warning when reference resolution relies on LCP (no `system:` tags).
