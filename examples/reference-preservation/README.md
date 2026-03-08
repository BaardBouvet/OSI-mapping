# Reference Preservation

When entities from the same source merge, contacts preserve their original reference IDs.

## Scenario

CRM companies 100 and 200 merge (same `domain="acme.com"`):
- Contact C1 references `company_id="100"`
- Contact C2 references `company_id="200"`

After merge and reverse transformation:
- C1 → `company_id="100"` (preserved)
- C2 → `company_id="200"` (preserved)

Without preservation, both would get the same arbitrary ID.
