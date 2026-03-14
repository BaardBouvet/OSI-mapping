# Design Rationale

This document explains the key design decisions behind the Integration Mapping Schema — why the schema takes the shape it does, what alternatives were considered, and what trade-offs were made.

## Single Unified File

**Decision:** Combine target definitions, field mappings, resolution rules, and test cases in one YAML file.

**Why:** Earlier iterations split these into separate files (a mapping schema and a resolution schema). In practice, resolution rules are tightly coupled to the mappings they govern — changing an identity field always means updating both. A single file eliminates cross-file references, makes each mapping self-contained, and simplifies validation. AI agents benefit too: one file gives full context without needing to resolve imports.

**Trade-off:** Larger files for complex integrations. Mitigated by clear sectioning (targets → mappings → tests) and the ability to split across multiple files when needed.

## Declarative, Not Procedural

**Decision:** Mapping files declare _what_ maps where and _how_ conflicts resolve. They never describe _when_ or _in what order_ processing happens.

**Why:** The schema describes the steady-state relationship between systems. Execution details (scheduling, batching, CDC, API calls) belong to the runtime that interprets the mapping. This keeps the spec portable across engines and prevents coupling to specific infrastructure.

## Resolution Strategy on Target Fields

**Decision:** Resolution strategies (`identity`, `coalesce`, `last_modified`, `expression`, `collect`) are declared on target fields, not on mappings or field mappings.

**Why:** Resolution is a property of the target model — "how do we decide what the canonical name is?" is a question about the shared entity, not about any particular source. Placing it on the target makes the resolution rule visible in one place regardless of how many sources contribute.

## Identity-Based Record Matching

**Decision:** Records from different sources are linked by matching `identity` field values (transitive closure), not by requiring pre-assigned shared IDs.

**Why:** In real integrations, systems rarely share a common primary key. Email addresses, tax IDs, or composite keys (name + date of birth) are the natural match keys. The schema supports this directly via `identity` strategy and `link_group` for composite matching.

**Link groups:** When multiple fields together form the identity (first_name + last_name + dob), a `link_group` ensures they are matched as a tuple rather than individually. Without link groups, matching on first_name alone would produce false positives.

## Coalesce with Explicit Priority

**Decision:** Priority for `coalesce` resolution is an integer on individual field mappings, not a source-level ranking.

**Why:** Different fields from the same source may have different trustworthiness. CRM might be authoritative for customer names but not for addresses. Per-field priority captures this. Lower numbers win, making the priority intuitive to read.

## Flat Nesting (source.path + parent_fields)

**Decision:** Nested array mapping uses flat `source.path` and `source.parent_fields` on the mapping, rather than recursive nested blocks inside field definitions.

**Alternatives considered:**
- Recursive `nested` block inside FieldMapping — caused schema recursion, broke `oneOf` constraints, made SQL expressions context-dependent
- Separate mapping entries with implicit parent context — ambiguous about which fields were available

**Why:** Flat nesting keeps every mapping as a regular top-level entry. SQL expressions work the same at any depth. Features like filters, routing, and embedding apply uniformly. No schema recursion means simpler validation and tooling.

## Expressions as Plain SQL Strings

**Decision:** All expressions are ANSI SQL strings. No template language, no placeholders.

**Why:** SQL is universally understood by engineers, database tooling, and AI agents. Using `${field}` or `{{ field }}` style templates would require a custom parser and create ambiguity about escaping and evaluation context. SQL expressions reference field names directly as column identifiers.

**Dialect support:** Currently single-dialect (ANSI SQL). Multi-dialect support is deferred — when needed, it would follow a convention-level dialects dictionary rather than per-expression objects.

## Embedded Entities

**Decision:** Sub-entities extracted from the same source row are marked with `embedded: true` on the mapping.

**Why:** Many APIs return denormalized data (an order with an embedded shipping address). The schema needs to express "these fields come from the same row but belong to a different target entity." The `embedded` flag is simpler than nested mapping definitions and reuses the same mapping structure.

## Bidirectional by Default

**Decision:** Field mappings are bidirectional by default — values flow forward (source → target) during resolution and backward (target → source) during reverse mapping.

**Why:** Most fields should propagate resolved values back to all sources. Unidirectional cases (computed fields, constants) are the exception and opt out via `direction: forward_only` or `direction: reverse_only`.

## Explicit Test Format

**Decision:** Test expected values always use the explicit `{ updates: [], inserts: [], deletes: [] }` structure, never bare arrays.

**Alternatives considered:**
- Bare array shorthand where all items are treated as updates — concise but ambiguous
- Polymorphic (bare array OR object) — AI agents frequently guessed wrong about which form to use

**Why:** Explicit structure eliminates ambiguity. Every test case clearly communicates whether rows are modifications, new records, or removals. This is critical for AI agents that generate test data — a single unambiguous format prevents systematic errors.

## Filters and Reverse Filters

**Decision:** `filter` and `reverse_filter` are separate SQL WHERE conditions on mappings.

**Why:** Forward and reverse filtering serve different purposes:
- `filter` — "which source rows qualify for this target?" (forward routing)
- `reverse_filter` — "which resolved rows should be written back to this source?" (reverse routing)

These are often asymmetric. A CRM mapping might accept all rows coming in (`filter` not set) but only send back rows where `type LIKE '%customer%'` (`reverse_filter` set). Separate properties make this explicit rather than requiring complex bidirectional expressions.

## Groups for Atomic Resolution

**Decision:** The `group` property on target fields ensures all grouped fields resolve from the same winning source.

**Why:** Address fields (street, city, zip, country) must come from the same source — mixing street from CRM with city from ERP produces invalid addresses. The `group` resolves atomically: the source with the most recent timestamp across any field in the group wins for all fields in that group.

## References (Foreign Keys)

**Decision:** Foreign-key relationships between target entities use a `references` property on the target field definition.

**Why:** When a target entity has a field that references another entity (e.g., `company_id` referencing `company`), the resolution engine needs to know this for FK resolution during entity linking. Declaring it on the target field keeps it close to the identity/resolution logic it interacts with.

**The hard problem this solves:** In multi-system integration, each system has its own ID namespace. CRM might reference company `2000` while ERP references the same real company as `CUST-001`. When entity linking merges these via a shared identity (e.g., domain), all foreign keys pointing to those companies need translating back to each source's local namespace during reverse mapping. Building this translation by hand requires maintaining cross-system ID maps, handling transitive merges, and dealing with race conditions — it's one of the most error-prone parts of integration. The `references` declaration makes it declarative: state the relationship once, and the engine handles ID translation automatically.

**Reference preservation:** When duplicate entities merge within the same source (e.g., two company records with domain `acme.com`), contacts that pointed to either original company keep their original `company_id` on reverse — because both IDs are still valid locally. The engine traces source identity through the merge rather than arbitrarily picking one winning ID.

## Vocabulary as Entity Mapping

**Decision:** Vocabulary normalization (e.g., country codes, status values) is handled by mapping lookup tables as regular target entities, not via special expression syntax.

**Why:** Vocabulary tables follow the same merge pattern as any other entity. Mapping `"Norge"` → `"NO"` is the same problem as mapping `"Alice A."` → `"Alice Anderson"` — two sources providing different values for the same identity. This avoids introducing a vocabulary-specific sublanguage while making bidirectional conversion automatic through reverse mapping.

## Schema Validation (7 Passes)

The validator performs structural and semantic checks in order:

1. **JSON Schema** — catches structural errors immediately
2. **Unique names** — prevents ambiguous references
3. **Target references** — ensures mappings point to real entities and fields
4. **Strategy consistency** — validates that required companion properties exist (priority for coalesce, timestamp for last_modified, expression for expression strategy)
5. **Field coverage** — warns about unmapped target fields
6. **Test datasets** — ensures test data matches declared sources
7. **SQL syntax** — optional parse check via sqlglot

The order matters: each pass assumes previous passes succeeded, so errors are reported at the most meaningful level.

## Sources as Shared Metadata

**Decision:** Primary keys live in a top-level `sources:` section, not on individual mappings.

**Why:** A source dataset (e.g., `crm`) is referenced by multiple mappings — one per target entity it feeds. Declaring `primary_key: id` once on the source avoids repeating it on every mapping that reads from that dataset. It also makes cross-cutting features possible: `links` declarations reference sources by name and need to know their primary key to join. Putting the PK on the source keeps structural metadata (table name, key columns) together, separate from the per-mapping transformation logic (field mappings, strategies, filters).

**Trade-off:** The mapping author must declare every source up front, even when there's only one mapping per source. This is a slight overhead for trivial cases but pays off immediately when a second mapping appears.

## Single Delta View per Mapping

**Decision:** Each mapping produces one `_delta_{mapping}` view with an `_action` column (`insert`, `update`, `delete`) rather than three separate views.

**Why:** The delta view is a single SELECT from the reverse view with a CASE expression classifying all three action types. For most mappings (no `reverse_required` or `reverse_filter`), the CASE is just insert-or-update. When filtering conditions exist, the delta adds a `WHEN {condition fails} THEN 'delete'` branch. One view, one SELECT, no joins.

**Alternatives considered:** Separate `_ins_`, `_upd_`, `_del_` views. Rejected because most consumers need to process all three action types in one pass anyway (e.g., a CDC pipeline), and the DAG visualization becomes cluttered with three views per mapping instead of one.

## No Diamond Dependencies in the DAG

**Decision:** Every view in the pipeline has exactly one path from any upstream view. The DAG is a strict funnel: `source → forward → identity → resolved → reverse → delta`. No view reads from two different layers.

**Why:** Diamond dependencies (two paths from the same upstream view to a downstream view) break Incremental View Maintenance (IVM). When a source row changes, the database must propagate through both paths and reconcile them — often resulting in intermediate inconsistencies or requiring expensive multi-path refresh. A diamond also complicates manual REFRESH MATERIALIZED VIEW scripting: the operator must understand the graph to determine the correct refresh order.

**How:** The reverse view passes ALL rows through (no filtering). Classification logic (`reverse_required`, `reverse_filter`) is evaluated in the delta view via a CASE expression. This means the delta depends only on the reverse view, not on both reverse and identity. The provenance view is the only other consumer of the identity view, and it has no downstream dependents.

**Previous approach:** The delta view used a UNION ALL: updates/inserts from the reverse view, deletes from the identity view via `NOT EXISTS`. This created a diamond: delta depended on both reverse and identity, which share the identity view as a common ancestor.

## Delta View Classification

**Decision:** The delta view is a single SELECT from the reverse view. It does not join back to the source table or the identity view.

**Why:** The reverse view already emits every entity: those with a source row (`_src_id IS NOT NULL`) and those without (`_src_id IS NULL` → insert). For deletes, the reverse view also carries all needed data: when a source row exists but `reverse_required` fields are NULL or `reverse_filter` fails, the delta classifies it as a delete via CASE.

**Benefits:** (a) The delta is trivially cheap — no joins, no UNION, just column alias and CASE. (b) Source rows excluded by forward `filter` (intentionally outside sync scope) never appear as deletes — they never enter the pipeline at all. (c) With IVM, there is no consistency concern because delete detection doesn't reference a second view.

## Links and Cluster Identity

**Decision:** External identity edges are declared as `links` on mappings, not as references on sources.

**Why:** A link is an instruction about entity resolution for a specific target. Placing it on the mapping scopes it correctly — if the same source maps to multiple targets, each mapping declares its own links. A mapping with `links` and no `fields` is a "linkage-only" mapping that contributes identity edges without business data.

**Two modes:**
- `links` **without `link_key`** (batch-safe): the engine generates pairwise edge SQL fed into the identity view's connected-components algorithm alongside identity-field edges. This is the standard path for record linkage tools and manual curation.
- `links` **with `link_key`** (IVM-safe): the `link_key` column provides a pre-computed cluster ID, pushed into the forward view via LEFT JOIN. Source row and cluster membership arrive atomically, avoiding the race where IVM sees a source row before its link.

## Cluster Members and Cluster Field

**Decision:** ETL feedback for insert tracking uses two mechanisms: `cluster_members` (separate table) and `cluster_field` (source column).

**Why:** When the delta view produces an insert, the ETL writes the new row to the target system and gets back a generated ID. To prevent duplicate inserts on the next run, the ETL links that generated ID to the entity's `_cluster_id`. Two patterns exist:

- **`cluster_members: true`** — ETL writes `(_cluster_id, _src_id)` to a per-mapping table. The forward view LEFT JOINs this table. Works with any target system.
- **`cluster_field: column_name`** — ETL writes `_cluster_id` as a field on the target record. The source table carries the cluster identity directly — no separate table needed. Simpler when the target system supports custom fields.

Both produce the same result: a `_cluster_id` column in the forward view. Rows sharing the same `_cluster_id` are linked by the identity algorithm, so pre-populated cluster memberships are respected alongside identity-field matching.

**Per-mapping tables:** `cluster_members` uses one table per mapping (default name: `_cluster_members_{mapping}`). Source PKs differ in type across mappings, so a shared table would require casting. Per-mapping tables also align with security boundaries.

## What's Intentionally Left Out

- **Execution semantics** — No scheduling, triggers, or processing order. That's runtime.
- **Authentication/connection** — No credentials or connection strings. That's infrastructure.
- **Schema evolution** — No migration directives. Versioning is by schema version (`1.0`).
- **Multi-dialect expressions** — Deferred until there's a clear convention to follow.
- **Computed aliases** — Reusable named expression aliases (e.g., `full_name`) deferred until real-world repetition justifies the complexity.
