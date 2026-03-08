# Cross-Entity References

Demonstrates cross-entity references where person references company across
different source systems. Each system has its own ID space, but references
resolve correctly through the target model.

## How it works

1. `company` and `person` are separate targets
2. `person.primary_contact` references `company`; `company.account_manager` references `person`
3. Each mapping declares which source field + dataset the reference resolves through
4. References are namespace-safe: ID "100" in ERP ≠ ID 100 in CRM
