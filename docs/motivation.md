# Why This Project Exists

Two-way sync between systems sounds simple. You have customers in a CRM and
customers in an ERP. Keep them in sync. How hard can it be?

Very hard. Every team that builds this discovers the same cascading set of
problems, usually in the worst possible order — after the "simple" sync is
already in production.

This document explains the problems, why they compound, and why we believe a
declarative mapping approach is a better foundation than the custom code that
most integration projects devolve into.

## The Deceptively Simple Starting Point

Two systems. Same concept (customer). Different schemas:

```
CRM:  account_id, name, email, phone
ERP:  customer_id, customer_name, email_address
```

Step one is field mapping. `name` → `customer_name`, `email` → `email_address`.
A junior developer can write this in an afternoon. A senior developer starts
asking questions that will take months to answer.

## Problem 1: Which System Wins?

Both systems have a `name` field. CRM says "Alice Anderson". ERP says "Alice A."
Which one is correct?

The answer is almost never "always CRM" or "always ERP". The CRM is
authoritative for contact information because salespeople maintain it. The ERP
is authoritative for billing addresses because finance maintains those. Some
fields have no clear owner — the most recently updated value is the best guess.
Some fields need a manual review workflow.

This is **conflict resolution**, and every team reinvents it. Common approaches:

- Hardcoded `if/else` in sync scripts — works until the rules change
- "Last write wins" everywhere — silently loses data
- Source priority as a single number — too coarse; different fields need
  different priorities
- Custom resolution tables in a database — correct, but enormously painful
  to build and maintain

Our approach: resolution strategy is declared per target field. Priority,
timestamp-based, expression-based, and identity-based strategies can coexist
in the same entity. Change the strategy by editing one line, not rewriting
sync code.

## Problem 2: Identity Is Not Obvious

Before you can resolve conflicts, you need to know that CRM account `2000` and
ERP customer `CUST-001` represent the same real-world entity.

Systems don't share primary keys. They rarely share any single identifier.
Matching happens on domain-meaningful fields: email address, tax ID, or a
combination of name + date of birth. This is called **entity linking** or
**record matching**.

Entity linking has transitive consequences. If CRM record A matches ERP record
B (shared email), and ERP record B matches warehouse record C (shared tax ID),
then A, B, and C are all the same entity — even though A and C have nothing in
common. This is a connected-components problem, and getting it wrong means
merging unrelated records or failing to merge related ones.

Most sync scripts handle identity implicitly: a lookup by email, a JOIN on
customer number. These break the moment a second identifier is involved or
when transitive chains appear. Fixing it requires building a graph — identity
is not a table lookup.

Our approach: `identity` fields participate in a transitive closure algorithm.
Declare which fields identify an entity. The engine builds the graph. Adding a
new identifier means adding a field, not rewriting the matching logic.

## Problem 3: Foreign Keys Across Systems

Suppose you've solved conflict resolution and identity. Now consider that
customers have orders. CRM stores `company_id: 2000` on a contact record.
ERP stores `customer_id: CUST-001` on an invoice.

You've determined that CRM company 2000 and ERP customer CUST-001 are the same
entity. Now you need to sync a contact from CRM to ERP. What `customer_id`
should the ERP record get?

Not `2000` — that's a CRM ID. Not `CUST-001` — that was the ERP's original ID
for a different record. You need to find the ERP source row that is part of the
same resolved entity and extract its `customer_id`.

This is **cross-system FK resolution**, and it's one of the hardest problems in
integration. It requires:

1. Knowing which fields are foreign keys
2. Knowing which target entity they reference
3. Tracing through the identity graph to find the right source-local ID
4. Handling cases where no local ID exists (insert) or multiple exist (merge)
5. Doing this for every reference, in every direction, every time

Custom implementations typically build FK translation tables maintained by
triggers or batch jobs. These are fragile, expensive, and fail silently when
entities merge.

Our approach: declare `references: company` on the field mapping. The engine
traces through the identity graph automatically.

## Problem 4: The Same Entity Referenced Different Ways

Country codes. Status values. Category labels. Different systems store the
same concept using different representations.

ERP stores `country: "Norway"`. CRM stores `country_code: "NO"`. Both refer to
the same country entity. A vocabulary table maps between them: `{name: "Norway",
iso_code: "NO"}`.

This is a special form of FK resolution where the source FK column doesn't
even store the referenced entity's primary key — it stores a different field
entirely. CRM's `country_code` column stores `iso_code` values; ERP's `country`
column stores `name` values. The reverse mapping must return the correct
representation for each source.

Custom implementations handle this with CASE expressions or lookup JOINs
scattered through sync scripts. Every new vocabulary table requires a new
custom handler.

Our approach: the vocabulary is a regular target entity with identity fields.
`references_field` on the field mapping tells the engine which field of the
referenced entity to return during reverse mapping.

## Problem 5: Nested and Embedded Structures

APIs increasingly return denormalized data: an order with embedded line items,
a customer with nested addresses, a project with departments containing
employees.

Syncing these requires:

- **Extracting** sub-objects from a parent (forward: flatten the JSON)
- **Resolving** them independently (each line item has its own identity)
- **Writing them back** with the correct parent FK (reverse: the address
  must reference the right customer)
- **Reassembling** arrays when writing back to nesting-style sources
  (reverse: collect all line items back into the JSON array)

Each level of nesting multiplies the implementation complexity. Two levels of
nesting (order → line → sub-line) with two source systems means building
extraction, resolution, FK resolution, and reassembly for each level in each
direction.

Custom implementations typically handle this by hard-coding the JSON path,
building custom aggregation queries, and manually managing parent-child
relationships. Adding a new nesting level means rewriting the extraction and
reassembly pipelines.

Our approach: `source.path` declares the JSON path. `parent_fields` brings
ancestor data into scope. The engine handles JSONB extraction in the forward
direction and array reassembly in the delta output.

## Problem 6: There's No "Forward Only"

Every integration starts as one-directional: pull from CRM, push to ERP. Then
someone asks "can we update CRM when ERP changes?" Now it's bidirectional, and
every assumption breaks.

Bidirectional sync requires:

- **Change detection**: what changed since last sync? Not just "is the value
  different" but "is the difference meaningful or is it a round-trip echo?"
- **Noop suppression**: if CRM says name is "Alice" and the resolved value is
  also "Alice", don't generate an update — it creates noise, triggers
  webhooks, and can cause infinite sync loops
- **Insert detection**: entity exists in ERP but not CRM → create a CRM record
  with the resolved values
- **Conflict resolution in both directions**: CRM→target and target→CRM must
  both be well-defined

Custom implementations of bidirectional sync almost always start with "we'll
just diff the current state against the last synced state." This requires
maintaining a snapshot of the last-synced state, handling schema changes in
the snapshot, and deciding what to do when the snapshot is stale or corrupt.

Our approach: the forward→identity→resolution→reverse→delta pipeline is
always bidirectional. Every mapping produces changes in both directions. Noop
detection compares the original source values (captured in `_base` at forward
time) against the resolved values, so round-trip echoes are suppressed without
maintaining external state.

## Problem 7: Generated IDs on Insert

The delta detects an entity that exists in CRM but not ERP and produces an
insert. The ETL creates the ERP record — and the target system assigns a new
ID (`CUST-042`). On the next run the engine sees that new record but doesn't
know it's the same entity. Without feedback, it generates another insert. And
another. An infinite duplicate-creation loop.

Solving this requires linking the new record's ID back to the entity cluster
before the next run, handling the case where the target system modifies values
on write, and surviving concurrent inserts from multiple sources.

Custom implementations build "sync ID maps" — cross-system key-pair tables
maintained by triggers or batch jobs. These break when entities merge, when
systems reassign IDs, or when multiple sync processes run concurrently.

Our approach: `cluster_members` and `cluster_field` provide a standard
feedback path. After an insert, the ETL writes the new record's ID and the
entity's `_cluster_id` to a membership table. On the next run, the engine
includes those memberships in the identity graph automatically.

## Problem 8: Three-Way (and N-Way) Sync

Most teams start with two systems. Then a third arrives. Every pairwise
approach breaks:

- Adding system C to an existing A↔B sync means building A↔C and B↔C — three
  pipelines, each with its own identity matching and conflict resolution. Four
  systems need six. Ten need forty-five.
- Pairwise conflict resolution doesn't compose. If A beats B and B beats C,
  what happens between A and C? The answer depends on all three values
  simultaneously.
- Identity linking becomes transitive. A matches B, B matches C, but A and C
  share no field — yet they're the same entity. Pairwise matching misses this.

Our approach: every source maps to a shared target model, not to other sources.
Adding a system means adding one mapping, not rebuilding existing ones. Identity
linking and conflict resolution work across all sources simultaneously.

## Problem 9: These Problems Compound

None of these problems exists in isolation. A real integration faces all of them
simultaneously:

- Three systems, each with different schemas, different ID namespaces, and
  different FK representations
- Some fields need priority-based resolution, others need timestamp-based,
  others need custom SQL expressions
- Company and contact entities reference each other across systems
- One system uses nested JSON, another uses normalized tables
- Everything needs to sync both ways with correct change detection
- A vocabulary table needs to translate between different source representations

The interaction between these problems is where custom code truly breaks down.
FK resolution depends on entity linking. Entity linking depends on identity
fields. Identity fields depend on vocabulary normalization. Vocabulary
normalization depends on FK resolution to the vocabulary entity. Circular
dependencies in custom code produce circular bugs.

## Why Declarative

A declarative mapping file solves these problems by describing the relationships
rather than coding the transformations:

- **Resolution is stated, not coded.** Priority, timestamp, expression — one
  property per field. Changing strategy doesn't require code changes.
- **Identity is structural, not procedural.** Identity fields participate in a
  graph algorithm. Adding a new identifier doesn't change the algorithm.
- **References are declared, not computed.** FK translation is automatic once
  the reference relationship is stated.
- **Bidirectionality is inherent.** Every mapping implies both forward and
  reverse. The delta pipeline exists for all mappings by default.
- **Testing is embedded.** Input data + expected output in the same file. Run
  the full pipeline in a test container. No mocks, no stubs, no partial
  verification.

The mapping file is the single source of truth for how systems relate. The
engine is the implementation. Change the mapping, the engine adapts. No
deployment, no migration, no code review for a "simple field mapping change"
that actually touches seven files across three services.

## What This Isn't

This project is not:

- An ETL tool (we don't schedule or orchestrate)
- A CDC system (we don't detect source changes)
- A database migration tool (we don't alter schemas)
- A data quality platform (we don't profile or cleanse)

It is a **mapping and resolution specification** with a reference engine. It
describes _what_ should happen when systems disagree. How you trigger it
(Kafka, cron, API webhook, dbt), where you store intermediate state (Postgres,
Snowflake, DuckDB), and how you deliver changes (API calls, bulk loads, event
streams) are implementation choices made by the surrounding infrastructure.

The schema is the contract between "what the business wants" and "what the
integration does." Currently that contract is scattered across sync scripts,
transformation code, mapping tables, and tribal knowledge. We put it in one
file.
