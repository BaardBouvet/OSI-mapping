# Asymmetric field direction

ERP expands company info on reads but only accepts the FK on writes. Writes require a tag field not present on reads.

## Scenario

An ERP and a CRM sync contact data. The ERP returns an expanded `company_name` alongside `company_id` when reading — the server resolves the name from the foreign key. On writes, only `company_id` is accepted; sending `company_name` back would be rejected. Additionally, every write to the ERP must include a `sync_source` tag that identifies the integration, but this field is never returned on reads.

## Key features

- **`direction: forward_only`** — `company_name` enriches the golden record on read but is excluded from reverse output
- **`direction: reverse_only`** — `sync_source` is injected into every write via `reverse_expression` but never read
- **`direction: bidirectional`** (default) — `email`, `name`, `company_id` flow in both directions

## How it works

1. The ERP mapping declares `company_name` as `direction: forward_only` — it contributes to the target on read but is stripped from the reverse view
2. The ERP mapping declares `sync_source` as `direction: reverse_only` with `reverse_expression: "'integration'"` — the engine injects this constant into every reverse output row
3. When CRM updates a contact's name, the ERP reverse view shows the new name, the original `company_id`, the injected `sync_source`, but no `company_name`
4. When a CRM-only contact is inserted into the ERP, the insert includes `sync_source` but not `company_name`

## When to use

- A source API expands sub-objects on read but only accepts IDs on write (common in Nordic ERPs like Tripletex, PowerOffice, Visma)
- A source requires a discriminator, audit tag, or routing field on writes that isn't returned on reads
- A field is computed server-side and should contribute to the golden record but never be written back
