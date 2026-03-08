# Relationship Embedded

Maps one-to-many relationships (ERP companies with embedded contact fields) to
many-to-many relationships (CRM with separate association table) through a
common target model.

## How it works

1. CRM has normalized tables: `companies`, `contacts`, `associations`
2. ERP has denormalized: company rows with embedded `contact_name`/`contact_email`
3. ERP embedded mappings contribute to `person` and `company_person_association` targets
4. `reverse_filter` on the association mapping ensures only `relation_type = 'employee'`
   records flow back to ERP
5. `link_group: relationship` ties `person_id` + `relation_type` as composite identity
