# Vocabulary — Custom Codes

Demonstrates using a common vocabulary for enumeration values where multiple
systems map to standard codes.

## How it works

1. `status` target acts as a vocabulary/lookup table with `name`, `hr_code`, `crm_code`
2. HR and CRM each have their own status code systems
3. `employee.status` references the `status` vocabulary
4. HR maps via `hr_code`, CRM maps via `crm_code`
5. Adding a new system only requires mapping to the vocabulary — not to every other system
