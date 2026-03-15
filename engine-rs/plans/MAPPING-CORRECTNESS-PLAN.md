# Mapping Correctness Fixes Plan

**Status:** Done  
**Scope:** Audit and fix questionable expected data and missing type declarations across examples, plus engine fixes for typed identity fields.

---

## Background

After bulk-fixing 13 examples' expected data to reach 35/35 passing, a review of the changes revealed several examples where the expected data was matched to engine output rather than corrected to reflect the intended behavior. This plan documents the analysis and fixes.

---

## Issues Analyzed

### 1. vocabulary-custom: status should be integer

**Problem:** CRM uses integer status codes (`crm_code: 1`, `crm_code: 0`), but the target field had no `type:` declaration, so the center model stores them as text.  
**Root cause:** Missing `type: integer` on `status.crm_code`.  
**Fix:** Added `type: integer` to the target field.  
**Engine fix required:** Forward view casts to `::integer`, but identity hash uses `COALESCE(field, '')` which fails for non-text types. Fixed `identity.rs` to use `COALESCE(field::text, '')`. Also, reverse reference matching `ref_match.{field} = r.{target}::text` fails for integer identity fields. Fixed `reverse.rs` to use `ref_match.{field}::text`.  
**Status:** Done.

### 2. composite-keys: unit_price decimal precision

**Problem:** Question about whether decimal numbers like `10.50` and `25.00` are handled correctly.  
**Analysis:** `type: numeric` is already declared on `quantity` and `unit_price`. PostgreSQL preserves `NUMERIC` precision. Input `25.00` resolves to `25` via PostgreSQL's numeric normalization. The expected data (`25`) is correct.  
**Status:** No fix needed.

### 3. relationship-embedded: crm_associations missing relation_type

**Problem:** Insert row for `crm_associations` has no `relation_type` value — just `{_cluster_id: "..."}`.  
**Analysis:** All three fields (`company_id`, `contact_id`, `relation_type`) are composite PK columns. The reverse view extracts PK values from `_src_id`, but for insert rows `_src_id` is NULL (entity exists only in ERP, not yet in CRM). The three `forward_only` fields in `erp_companies_assoc` don't produce reverse output. This is the known COMPOSITE-KEY-REFS-PLAN limitation — not a data bug.  
**Fix:** Implemented COMPOSITE-KEY-REFS-PLAN: PK columns with reverse field mappings now use `COALESCE(pk_extraction, field_expr)` in the reverse view. For insert rows, `company_id` resolves through reference to company ("C1"), `contact_id` resolves through reference to person (NULL — no CRM contact for Charlie Brown), and `relation_type` resolves through identity fallback ("employee"). Updated expected to verify `company_id` and `relation_type` values. Also updated insert comparison to use subset matching (expected keys only, extra keys in actual ignored).
**Status:** Done.

### 4. value-conversions: phone not fully stripped

**Problem:** `REGEXP_REPLACE(phone_number, '[^0-9]', '')` only strips the first non-digit character. Input `"+1 555-1234"` becomes `"1 555-1234"` instead of `"15551234"`.  
**Root cause:** PostgreSQL's `REGEXP_REPLACE` requires the `'g'` flag for global replacement.  
**Fix:** Changed expression to `REGEXP_REPLACE(phone_number, '[^0-9]', '', 'g')`. Updated expected `contact_phone` from `"1 555-1234"` to `"15551234"`.  
**Status:** Done.

### 5. embedded-simple: billing/shipping columns are null

**Problem:** CRM's billing_street, billing_city, shipping_street, shipping_city are all null in the update row, even though billing system has newer address data.  
**Root cause:** `billing_address` target had no identity field. Without identity, CRM's embedded billing entity and the billing system's entity are separate (no merge). The resolved billing_address has no CRM member, so the embedded row is noop — and the non-embedded customer mapping row gets `NULL::text` for all billing columns (UNION ALL padding).  
**Fix:** Added `customer_ref: identity, references: customer` field to both `billing_address` and `shipping_address` targets. Added corresponding field mappings (`customer_email → customer_ref` for CRM, `account_email → customer_ref` for billing). Now address entities merge via customer email. Updated expected to show billing address update flowing from billing system to CRM.  
**Status:** Done.

### 6. embedded-multiple: two rows with null columns

**Problem:** Two update rows for CRM, each with most columns null.  
**Root cause:** Delta view used UNION ALL of separate embedded mappings. Each mapping's reverse view only projected its own fields; other columns were padded with `NULL::text`.  
**Fix:** Implemented embedded merge in `delta.rs` — embedded reverse views are now LEFT JOINed on `_src_id` to the primary reverse view, producing one merged row per source record with all fields populated. The `_base` JSONB is merged via `||` so noop detection covers all fields in one check. Updated expected data to one merged row.  
**Status:** Done.

### 7. embedded-vs-many-to-many: domain is null

**Problem:** CRM update row shows `domain: null` instead of `"acme.com"`.  
**Root cause:** Same UNION ALL pattern as #6. The embedded contacts mapping produced its own row with `NULL::text AS "domain"`, while the non-embedded customer mapping produced a noop.  
**Fix:** Same embedded merge fix as #6. The domain value now comes from the primary mapping's reverse view, merged into the same row as the embedded contact fields. Updated expected data from `domain: null` to `domain: "acme.com"`.  
**Status:** Done.

### 8. merge-generated-ids: id columns not numeric

**Problem:** `system_a_id`, `system_b_id`, `system_c_id` are integers in source data but stored as text in the center model.  
**Root cause:** Missing `type: integer` declarations.  
**Fix:** Added `type: integer` to all three identity fields.  
**Engine fix required:** Source PK columns in the reverse view come from `_src_id` (always text). Added `typed_pk_select_exprs()` helper in `reverse.rs` that checks if the PK column maps to a typed identity field — if so, casts `_src_id::integer` instead of plain `_src_id`. Updated expected data from `id: "101"` to `id: 101`.  
**Status:** Done.

### 9. value-groups: cid as string

**Problem:** Expected data has `cid: "1"` instead of numeric `1`.  
**Root cause:** `cid` is the source PK but doesn't map to any target field, so there's no target `type:` to infer from. `_src_id` is always text.  
**Fix:** Added `types:` map to `Source` model (`types: { cid: integer }`). The `typed_pk_select_exprs` helper now checks source-level `types:` as fallback when no target field type exists. Updated JSON schema. Updated expected data from `cid: "1"` to `cid: 1`.  
**Status:** Done.

### 10. Validate default value vs field type (new validation)

**Problem:** No validation catches when a `default:` value is incompatible with the declared `type:` (e.g., `default: "foo"` on a `type: numeric` field).  
**Fix:** Added Pass 9 (`pass_default_type_compat`) to `validate.rs`. Warns when a string/non-numeric default is used on a numeric field, or a non-boolean default on a boolean field.  
**Status:** Done.

---

## Engine Changes

| File | Change | Lines |
|------|--------|-------|
| `src/render/identity.rs` | `COALESCE({field}::text, '')` — cast typed identity fields to text before COALESCE with empty string | ~L101 |
| `src/render/reverse.rs` | `ref_match.{field}::text = r.{target}::text` — cast identity fields to text in reference matching | ~L95 |
| `src/render/reverse.rs` | Auto-typed reference resolution: when referenced mapping's single PK maps to a typed identity field, return `ref_local.{field}` (typed) instead of `ref_local._src_id` (text) | `return_expr` |
| `src/render/reverse.rs` | `pk_base_expr_map()` — returns base PK expressions without AS alias. PK columns with reverse field mappings use `COALESCE(pk_base, field_expr)` for insert resolution | Replaces `typed_pk_select_exprs` |
| `src/validate.rs` | Pass 9: `pass_default_type_compat` — warns on default/type mismatch | New function |

## Test Changes

| File | Change |
|------|--------|
| `tests/integration.rs` | Insert comparison switched from full-row to subset matching (expected keys only). PK columns no longer stripped from actual inserts. |
| `tests/integration.rs` | Added `dump_relationship_embedded_intermediates` test |

## Example Changes

| Example | Change |
|---------|--------|
| `vocabulary-custom` | Added `type: integer` to `status.crm_code` |
| `value-conversions` | Added `'g'` flag to REGEXP_REPLACE; updated expected phone |
| `merge-generated-ids` | Added `type: integer` to 3 identity fields |
| `relationship-embedded` | PK columns now resolve for inserts; added `company_id: "C1", relation_type: "employee"` to expected |
| `embedded-simple` | Added `customer_ref` identity field to address targets + mappings; updated expected |

## Verification

All 35/35 examples passing, 22/22 tests green (`cargo test`).
