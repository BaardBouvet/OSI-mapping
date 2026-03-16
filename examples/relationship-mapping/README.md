# Relationship Mapping

Maps many-to-many relationships (CRM with separate association table) to one-to-many relationships (ERP with direct foreign keys) through a common target model.

## How it works

1. CRM has three tables: companies, contacts, and associations (junction table)
2. ERP has two tables: companies and contacts (with embedded company FK)
3. Both map to targets: `company`, `person`, and `company_person_association`
4. ERP contacts use `parent:` to emit both a person and an association from one row
5. `link_group` on person_id + relation_type ensures associations merge correctly across sources
6. `filter` on ERP association mapping restricts reverse output to employee relations only
