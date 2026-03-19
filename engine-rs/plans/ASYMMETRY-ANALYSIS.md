# Read/write asymmetry: mapping concern or ETL concern?

**Status:** Design

Analysis of where read/write asymmetries belong in the mapping-vs-ETL boundary.
"Asymmetry" means data has a different shape when reading from a system than when writing to it.

---

## The question

Many real-world APIs and databases present different shapes for reads and writes:

- **REST APIs:** `GET /contacts` returns `{ address: { street, city } }` but
  `POST /contacts` expects `{ street: "...", city: "..." }` (flat).
- **Soft deletes:** Read returns `{ deleted_at: "2025-01-01T..." }` (timestamp),
  but write expects `DELETE /contacts/123` (an HTTP verb, not a field).
- **Computed fields:** Read returns `{ total: 150.00 }` but write ignores `total`
  (it is recalculated server-side).
- **Envelope differences:** Read returns `{ data: [...], meta: {...} }` but
  write expects just the bare array.
- **Embedded vs normalized:** Read returns inline objects; write expects IDs pointing
  to separately-created resources.
- **Default injection:** Omitted fields on write get server-side defaults;
  reads always include them.

The question: should the mapping language model these asymmetries, or should the
ETL layer normalize them before the mapping engine ever sees the data?

---

## Design principle: ETL is mechanics, mapping is semantics

The project draws a clear line:

| Layer | Responsibility | Examples |
|-------|---------------|----------|
| **ETL** | Transport, scheduling, serialization, pagination, auth, retry | Kafka consumers, cron, REST polling, webhook handlers |
| **Mapping** | Field correspondence, conflict resolution, identity, cardinality, directionality | `expression`, `reverse_expression`, `strategy`, `filter` |

ETL should be **mechanically faithful**: it moves data in and out, converting between wire
formats (JSON, XML, CSV) and the staging tables the engine reads from.
It should not make semantic decisions about which fields matter or how conflicts resolve.

---

## What the mapping language already handles

The current schema has composable primitives that cover most read/write asymmetries:

### 1. Transformation asymmetry (`expression` / `reverse_expression`)

```yaml
- source: phone_number
  target: phone
  expression: "REGEXP_REPLACE(phone_number, '[^0-9]', '', 'g')"
  reverse_expression: "'+' || SUBSTRING(phone, 1, 1) || ' (' || SUBSTRING(phone, 2, 3) || ') ' || SUBSTRING(phone, 5, 3) || '-' || SUBSTRING(phone, 8, 4)"
```

Forward strips formatting; reverse restores it. The two expressions are independently
authored — they are not required to be mathematical inverses.

### 2. Direction asymmetry (`direction`)

```yaml
- source: contact_type
  direction: reverse_only
  reverse_expression: "'person'"
```

A field that only exists in one direction. Reading never touches it; writing generates
a hardcoded discriminator. Covers computed markers, type flags, and write-only defaults.

### 3. Filter asymmetry (`filter` / `reverse_filter`)

```yaml
mappings:
  - name: crm_customers
    filter: "status != 'deleted'"
    reverse_filter: "is_deleted IS NOT TRUE"
```

Forward and reverse filters operate independently. A source might contribute all data
but only accept back a subset (or vice versa).

### 4. Existence asymmetry (`reverse_required`)

```yaml
- source: customer_id
  target: customer_id
  reverse_required: true
```

If the resolved value is NULL, the entire row is excluded from reverse output
(classified as a delete). Enables lifecycle propagation without explicit delete signals.

### 5. Cardinality asymmetry (parent/child mappings)

```yaml
# CRM: single phone column → scalar target field
# Contact center: phones JSONB array → child entities
```

The multi-value pattern handles scalar↔list mismatches through separate parent and
child target entities with different resolution strategies.

### 6. Structural depth asymmetry (nested hierarchies)

System A has 2 levels (Product → Features), System B has 3 levels (Product → Modules → Features).
Both normalize to the same target entities; the reverse excludes data that can't be
represented at the source's nesting depth via `reverse_filter`.

### 7. Precision asymmetry (`normalize`)

```yaml
- source: price
  target: price
  normalize: "round(%s::numeric, 0)::integer::text"
```

Different systems store values at different precision. Normalization prevents infinite
false-update loops by comparing at the target system's fidelity level.

### 8. Expanded-on-read, ID-on-write asymmetry (Nordic ERP pattern)

Many ERPs (common in Nordic systems like Tripletex, PowerOffice, Visma, 24SevenOffice)
expand sub-objects when reading but expect only an ID reference when writing:

```
GET /invoices/42 →
{
  "customer": { "id": 1001, "name": "Acme Corp", "orgNumber": "912345678" },
  "department": { "id": 5, "name": "Sales", "code": "SALES" },
  "lines": [
    { "product": { "id": 99, "name": "Widget", "sku": "W-100" }, "quantity": 3 }
  ]
}

PUT /invoices/42 →
{
  "customer": { "id": 1001 },
  "department": { "id": 5 },
  "lines": [
    { "product": { "id": 99 }, "quantity": 3 }
  ]
}
```

On read, `customer` is an expanded object with name, org number, etc. On write, the
API expects `{ "id": 1001 }` — just the foreign key. The expanded fields are read-only
projections; sending them back is either ignored or rejected.

**This splits cleanly across ETL and mapping:**

**ETL responsibility:** Flatten the expanded objects into staging columns. A read
from the API produces a staging row like:

```
invoice_id | customer_id | customer_name | customer_org | department_id | department_name
42         | 1001        | Acme Corp     | 912345678    | 5             | Sales
```

When writing back, ETL reads the delta view (which contains `customer_id`, `department_id`)
and constructs the `{ "id": ... }` wrapper objects. This is serialization — the same
mechanical concern as building any JSON payload from flat columns.

**Mapping responsibility:** The expanded fields (`customer_name`, `customer_org`,
`department_name`) are either:

1. **Mapped as `direction: forward_only`** — they contribute to the golden record
   but are never written back to this source (the ERP recalculates them from the ID):

```yaml
mappings:
  - name: erp_invoices
    source:
      table: erp_invoices
    fields:
      # The FK — bidirectional, resolved via references
      - source: customer_id
        target: customer_id
        references: erp_customers

      # Expanded read-only fields — enrich the golden record
      - source: customer_name
        target: customer_name
        direction: forward_only

      - source: customer_org
        target: customer_org_number
        direction: forward_only

      # Department FK — bidirectional
      - source: department_id
        target: department_id
        references: erp_departments

      # Department name — read-only enrichment
      - source: department_name
        target: department_name
        direction: forward_only
```

2. **Already mapped via a separate entity mapping** — if `customer` is its own target
   entity (as it normally would be in a multi-system sync), then the expanded fields
   are redundant with the customer mapping and can be ignored entirely. The `references:`
   property on `customer_id` handles FK translation in the reverse view.

**Why this is not a new concern:** The ERP's expand-on-read pattern decomposes into
(a) staging table design (ETL) and (b) `direction: forward_only` + `references:`
(existing mapping primitives). No new schema feature is required.

### 9. Association asymmetry (HubSpot pattern)

CRM platforms like HubSpot model relationships between objects (Companies, Contacts,
Deals) as first-class "associations" with a distinctly asymmetric API:

```
GET /crm/v3/objects/companies/100?associations=contacts →
{
  "id": "100",
  "properties": { "name": "Acme Corp", "domain": "acme.com" },
  "associations": {
    "contacts": {
      "results": [
        { "id": "201", "type": "company_to_contact" },
        { "id": "202", "type": "company_to_contact" }
      ]
    }
  }
}
```

Reading returns associations **embedded in the parent object**. But writing
associations is a completely separate API:

```
PUT /crm/v3/objects/companies/100
  Body: { "properties": { "name": "Acme Corp" } }
  ← Only updates properties. Cannot create/remove associations here.

PUT /crm/v4/associations/companies/contacts/batch/associate
  Body: { "inputs": [{ "from": { "id": "100" }, "to": { "id": "203" }, "types": [...] }] }
  ← Separate endpoint to manage the association itself.
```

The asymmetries compound:

1. **Read shape ≠ write shape:** Associations are embedded on read, separate API on write.
2. **Many-to-many with metadata:** A Contact can belong to multiple Companies; each
   link has a `type` (primary, secondary) and an `associationCategory`.
3. **Multi-labeled:** The same pair (Company 100 ↔ Contact 201) can have multiple
   association types simultaneously (e.g., both "company_to_contact" and "employer").
4. **Unidirectional creation:** You create the link from one side, but it appears
   on both sides when reading.

**This splits across ETL and mapping as follows:**

**ETL responsibility:**

- **On read:** Extract the embedded `associations` block into a separate staging
  table — a classic junction/bridge table:

  ```
  hs_company_contact_assoc
  ────────────────────────────────────
  company_id | contact_id | assoc_type
  100        | 201        | company_to_contact
  100        | 202        | company_to_contact
  ```

  The parent object's properties go into their own staging table (`hs_companies`).
  This is structural flattening — the same ETL concern as envelope stripping.

- **On write:** Read the delta view for the association mapping and translate
  inserts/deletes into calls to the batch association API. This is transport
  serialization — building the right HTTP request from flat delta rows.

**Mapping responsibility:**

The junction table is a regular entity in the mapping, modeled as a relationship
mapping with its own target:

```yaml
targets:
  - name: company
    fields: [company_id, name, domain]
  - name: contact
    fields: [contact_id, first_name, last_name, email]
  - name: company_contact_link
    fields: [company_id, contact_id, assoc_type]

mappings:
  # HubSpot company properties
  - name: hs_companies
    source:
      table: hs_companies
    target: company
    fields:
      - source: id
        target: company_id
        strategy: identity
      - source: name
        target: name
      - source: domain
        target: domain

  # HubSpot contact properties
  - name: hs_contacts
    source:
      table: hs_contacts
    target: contact
    fields:
      - source: id
        target: contact_id
        strategy: identity
      - source: firstname
        target: first_name
      - source: lastname
        target: last_name
      - source: email
        target: email

  # HubSpot association → junction target
  - name: hs_company_contacts
    source:
      table: hs_company_contact_assoc
    target: company_contact_link
    fields:
      - source: company_id
        target: company_id
        strategy: identity
        references: hs_companies
      - source: contact_id
        target: contact_id
        strategy: identity
        references: hs_contacts
      - source: assoc_type
        target: assoc_type
        strategy: coalesce
        priority: 1
```

If another system (e.g., an ERP) also has company–contact relationships, it gets its
own mapping to the same `company_contact_link` target. The identity resolution merges
links across systems, and `references:` handles FK translation in both directions.

The delta view for `hs_company_contacts` produces insert/update/delete rows. ETL
reads those and calls the appropriate HubSpot association batch API — creating
new associations for inserts, removing them for deletes.

**Why this works with existing primitives:**

- **Junction table as entity:** The association is just another target with composite
  identity (`company_id` + `contact_id`). No special "association" concept needed.
- **Multi-labeled associations:** If the same pair has multiple types, each type is a
  separate row in the staging table (different `assoc_type`), which means a separate
  identity row — or `assoc_type` can be part of the composite identity if the label
  itself matters.
- **FK translation via `references:`** handles the ID mapping between systems.
- **Insert/delete propagation:** The delta view naturally computes which associations
  need to be created or removed.

**The general principle:** When an API buries relationships inside parent objects on
read but manages them through a separate endpoint on write, ETL normalizes the read
into a junction staging table and translates delta output into the write API. The
mapping treats the junction as a regular entity. This is the same decomposition as
embedded-vs-normalized — a pattern the schema already handles.

---

## What asymmetries should NOT be in the mapping

Some asymmetries are purely mechanical and belong in ETL:

### Wire format differences

- **JSON ↔ XML ↔ CSV serialization:** The ETL layer deserializes into staging tables.
  The mapping engine never sees wire format.
- **Envelope stripping:** `{ data: [...], meta: {...} }` → ETL extracts the `data` array
  into a staging table. The mapping just sees rows.
- **Pagination:** ETL handles `?page=2&limit=100`; mapping sees the full dataset.

### Authentication and transport

- **OAuth token refresh, API key headers, mTLS:** Pure ETL/infrastructure.
- **Rate limiting and retry:** ETL orchestration.

### Temporal mechanics

- **Polling intervals, CDC log tailing, webhook registration:** ETL scheduling.
- **Eventually consistent reads** (the "2-second delay" problem): ETL must handle
  read-after-write visibility windows. The mapping engine assumes it sees a consistent
  snapshot.

### Staging table shape normalization

When an API's read shape is so different from its write shape that they are effectively
different schemas, ETL should present them as **two separate staging tables** (or two
column sets within one table). Examples:

- Read endpoint returns nested `{ address: { street, city } }` → ETL flattens into
  columns `address_street`, `address_city` in the staging table.
- Write endpoint expects `{ updates: [{ op: "set", path: "/name", value: "Alice" }] }`
  (JSON Patch) → ETL constructs patches from the delta view output. This is pure
  serialization, not mapping.

The key test: **does the asymmetry involve semantic meaning (which field wins, what it means)?**
If yes → mapping. **Does it involve serialization or transport (how bytes move)?** If yes → ETL.

---

## The grey zone: asymmetries that could go either way

Some cases are genuinely ambiguous:

### A. Soft-delete representation differences

- Source A: `deleted_at` timestamp (NULL = active)
- Source B: `is_active` boolean (true = active)
- Source C: Row disappears entirely (hard delete)

**Current approach:** Mapping handles A and B via `expression` (converting to a target
boolean). ETL handles C via `_synced_entities` state tracking.

**Verdict: correct split.** The timestamp→boolean conversion is semantic (the mapping
decides what "deleted" means). The hard-delete detection is mechanical (ETL must track
what rows previously existed).

### B. Write-side resource creation order

Some APIs require creating parent resources before children (e.g., create an Account
before creating its Contacts). The mapping knows the entity relationships, but the
execution order is an ETL concern.

**Verdict: ETL.** The mapping's DAG defines data dependencies. ETL interprets that DAG
to sequence API calls. The mapping should not model "create X before Y" — that is
a consequence of the entity graph that ETL can derive from the delta view output order.

### C. Idempotency keys and write tokens

Some APIs require an `idempotency_key` header on POST. The value is not part of the
data model — it is a transport concern.

**Verdict: ETL.** The mapping has no business knowing about idempotency keys.

### D. Conditional writes (ETags, If-Match)

Read returns `ETag: "abc123"`; write requires `If-Match: "abc123"`. This is
optimistic concurrency at the transport level.

**Verdict: ETL.** The mapping provides the resolved values. ETL handles conditional
write mechanics.

### E. Field availability asymmetry (read-only / write-only API fields)

An API returns 30 fields on GET but only accepts 10 on PUT.

**Verdict: mapping.** The mapping already handles this with `direction: forward_only`
for read-only fields and `direction: reverse_only` for write-only fields.
No new feature needed.

### F. Different update granularity

- Read: full resource (all fields)
- Write: partial update (only changed fields, PATCH semantics)

**Verdict: ETL.** The delta view already computes which fields changed. ETL interprets
the delta to construct PATCH payloads with only the changed fields. The mapping
does not need to know whether the target API supports PATCH vs PUT.

---

## Conclusion

**Asymmetry is overwhelmingly a mapping concern**, and the current schema already
handles it well through composition of small, orthogonal primitives:

| Asymmetry type | Responsibility | Mechanism |
|---------------|---------------|-----------|
| Value transformation | **Mapping** | `expression` / `reverse_expression` |
| Field existence | **Mapping** | `direction: forward_only \| reverse_only` |
| Row qualification | **Mapping** | `filter` / `reverse_filter` |
| Lifecycle propagation | **Mapping** | `reverse_required` |
| Cardinality mismatch | **Mapping** | Parent/child entity pattern |
| Structural depth | **Mapping** | Nested mappings + `reverse_filter` |
| Precision differences | **Mapping** | `normalize` |
| Expanded-on-read / ID-on-write | **Both** | ETL flattens + reconstructs; mapping uses `direction: forward_only` + `references:` |
| Associations embedded on read, separate API on write | **Both** | ETL normalizes to junction table + translates deltas; mapping treats junction as regular entity |
| Wire format / serialization | **ETL** | Staging table normalization |
| Transport (auth, retry, pagination) | **ETL** | Orchestration infrastructure |
| Write execution order | **ETL** | DAG-derived sequencing |
| Conditional writes / idempotency | **ETL** | Transport-level concerns |
| Hard-delete detection | **ETL** | `_synced_entities` state tracking |

### The guiding rule

> **If the asymmetry affects what the data means → mapping.**
> **If the asymmetry affects how the data moves → ETL.**

### What this means for the project

No new schema features are needed for asymmetry handling. The existing primitives
(`expression`/`reverse_expression`, `direction`, `filter`/`reverse_filter`,
`reverse_required`, `normalize`, parent/child patterns) compose to cover all
semantic asymmetries. ETL handles the remaining mechanical asymmetries outside
the mapping boundary.

The one area to watch is **hard-delete propagation**, which straddles the boundary:
ETL detects the absence (mechanical), but the mapping decides what it means
(semantic). The `_synced_entities` approach in the ETL-STATE-INPUT-PLAN handles
this correctly by keeping the two responsibilities separated across layers.
