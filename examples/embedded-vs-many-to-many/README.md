# Embedded vs Many-to-Many

CRM has embedded contacts within customer rows. ERP has normalized tables with a junction table for customer-contact relationships.

## How it works

1. CRM: one row = customer + embedded primary contact + embedded association
2. ERP: separate customers, contacts, and customer_contacts tables
3. Both map to `customer`, `contact`, and `customer_contact` targets
4. `reverse_filter` on CRM association only writes back primary-type relations
