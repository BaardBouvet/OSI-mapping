# Passthrough fields

**Status:** Planned

Support carrying source columns through the pipeline to the delta view
without mapping them to target fields.

## Problem

The delta view outputs only columns that participate in the target model —
mapped source fields that went through forward → resolution → reverse. But
ETL consumers often need additional source columns in the delta output:

**1. API/write context.** The source system's update API requires fields
that aren't in the target model. E.g., CRM requires `record_type` on every
PUT call, but `record_type` is a source-internal concept with no target
representation.

**2. Self-contained change feeds.** When the delta view is consumed as a
CDC stream (Kafka, change data capture), the consumer doesn't have direct
access to the source tables. It needs enough context in each delta row to
construct a complete write operation.

**3. Audit / correlation IDs.** Source systems carry audit fields
(`created_by`, `last_sync_id`, `tenant_code`) that don't belong in the
golden record but must survive in delta output for traceability.

**4. Conditional ETL logic.** The ETL needs a source column to decide *how*
to apply the delta. E.g., `region_code` determines which API endpoint to
call, but region isn't a target field.

### What happens today

Unmapped source columns are excluded at the forward view — the first stage.
They never enter `_base`, never flow through identity or resolution, and
cannot appear in reverse or delta views. The only workaround is to add them
as target fields (with `direction: forward_only` or a dummy strategy),
which pollutes the target model with source-specific concerns.

## Design

### Syntax: `passthrough` list on mappings

```yaml
mappings:
  - name: crm_contacts
    source: { dataset: crm }
    target: contact
    passthrough:
      - record_type
      - region_code
      - tenant_id
    fields:
      - source: email
        target: email
      - source: name
        target: name
```

`passthrough` is a flat list of source column names. These columns:

| Property | Behavior |
|----------|----------|
| Included in forward view | Yes — raw, uncast, as-is |
| Included in `_base` | Yes — for round-trip preservation |
| Participate in identity | No |
| Participate in resolution | No |
| Included in reverse view | Yes — extracted from `_base` |
| Included in delta output | Yes — as regular columns |
| Affect noop detection | **No** — changes to passthrough fields alone don't trigger updates |

### Why passthrough fields don't affect noop

The noop check answers: "did anything change that the target model cares
about?" Passthrough fields are explicitly outside the target model. If only
a passthrough field changed in the source, the delta should still say
`noop` — the golden record hasn't changed, so there's nothing to sync.

The passthrough column's value in the delta row always reflects the
**current source value** (from `_base`), giving the ETL fresh context
regardless of the action.

### Why not `direction: passthrough`

Adding a `Direction` variant was considered, but passthrough fields don't
have a target — they're not field mappings at all. The existing `direction`
enum controls how a mapped field participates in forward/reverse flow.
A passthrough column has no target, no expression, no strategy. A
mapping-level list is a cleaner separation of concerns.

## Data flow

### Forward view

Today the forward view builds `_base` from all mapped fields:

```sql
jsonb_build_object('email', email, 'name', name) AS _base
```

With passthrough, the listed columns are added to `_base`:

```sql
jsonb_build_object(
  'email', email, 'name', name,
  'record_type', record_type,
  'region_code', region_code,
  'tenant_id', tenant_id
) AS _base
```

The passthrough columns are NOT added to the forward view's SELECT as
separate columns — they only live inside `_base`. This avoids polluting
the identity and resolution views with columns that have no strategy.

### Identity view

No change. `_base` flows through as part of `SELECT *`.

### Resolution view

No change. Passthrough fields don't participate in aggregation.

### Reverse view

The reverse view already accesses `_base` via the identity subquery:

```sql
FROM _resolved_{target} AS r
LEFT JOIN (SELECT _src_id, _mapping, _entity_id_resolved, _base, ...
           FROM _id_{target}) AS id
  ON id._entity_id_resolved = r._entity_id
  AND id._mapping = '{mapping}'
```

Passthrough columns are extracted from `_base`:

```sql
SELECT
  ...existing columns...,
  id._base->>'record_type' AS "record_type",
  id._base->>'region_code' AS "region_code",
  id._base->>'tenant_id' AS "tenant_id"
```

For insert rows (new rows where `id._src_id IS NULL`), passthrough columns
are `NULL` — which is correct since there's no existing source row to pull
context from. The ETL handles inserts differently anyway.

### Delta view

Passthrough columns appear in the delta output alongside mapped fields:

```sql
SELECT
  CASE ... END AS _action,
  _cluster_id,
  "email",           -- PK
  "name",            -- mapped field
  "record_type",     -- passthrough
  "region_code",     -- passthrough
  "tenant_id",       -- passthrough
  _base
FROM _rev_crm_contacts
```

They're included in `out_cols` but NOT in the noop comparison.

## Noop detection — passthrough excluded

Today's noop check:

```sql
WHEN _base->>'email' IS NOT DISTINCT FROM "email"::text
 AND _base->>'name'  IS NOT DISTINCT FROM "name"::text
THEN 'noop'
```

Passthrough fields are not added to the noop parts. If only `record_type`
changed in the source, the delta still says `noop`. This is correct — the
golden record didn't change; the passthrough value is contextual.

## Insert rows and passthrough

Insert rows have `_src_id IS NULL` — they're new records that the source
doesn't have yet. For these rows:

- Passthrough columns are `NULL` (no source row exists)
- The ETL must handle inserts differently (e.g., omit unknown fields, or
  use defaults)

This is the natural behavior and needs no special handling.

## Interaction with routing (multiple mappings per source)

When a source has multiple mappings (route pattern), each mapping may
declare different passthrough columns. The delta UNION ALL already handles
this — each branch contributes its own columns, with NULL for columns not
in that branch.

```sql
-- _delta_crm:
SELECT _action, _cluster_id, email, name, record_type, region_code, ...
FROM _rev_crm_persons   -- passthrough: [record_type, region_code]

UNION ALL

SELECT _action, _cluster_id, email, name, NULL, NULL, ...
FROM _rev_crm_companies -- passthrough: [] (none declared)
```

## Example

```yaml
sources:
  crm:
    table: crm_contacts
    primary_key: [email]

targets:
  contact:
    fields:
      email: identity
      name: coalesce

mappings:
  - name: crm_contacts
    source: { dataset: crm }
    target: contact
    passthrough:
      - record_type
      - region_code
    fields:
      - source: email
        target: email
      - source: name
        target: name

tests:
  - description: Passthrough fields appear in delta
    input:
      crm:
        - { email: "a@x.com", name: "Alice", record_type: "person", region_code: "EU" }
    expected:
      crm:
        updates:
          - { email: "a@x.com", name: "Alice", record_type: "person", region_code: "EU" }
```

## Scope of changes

### Model
- `model.rs`: Add `passthrough: Vec<String>` to `Mapping` struct (serde
  default empty vec).
- `mapping-schema.json`: Add `passthrough` array-of-strings property to
  mapping definition.

### Forward view
- `forward.rs`: When building `_base`, also include columns listed in
  `mapping.passthrough`. ~5 lines added to the `_base` builder loop.

### Reverse view
- `reverse.rs`: For each passthrough column, emit
  `id._base->>'col' AS "col"` in the reverse view SELECT. ~10 lines.

### Delta view
- `delta.rs`: Include passthrough columns in `out_cols` (union across
  mappings for the source). Exclude from noop detection. ~10 lines.

### Validation
- `validate.rs`: Warn if a passthrough column name conflicts with a
  mapped field's source name (ambiguity). Warn if passthrough is declared
  on a mapping with `direction: forward_only` on all fields (no delta
  generated, so passthrough is dead code).

### Test infrastructure
- Extend test expectation comparison to account for passthrough columns
  in delta output.

Total: ~40 lines of production code changes across 4 render files.

## Alternatives considered

### A. Passthrough on field mappings (`direction: passthrough`)

```yaml
fields:
  - source: record_type
    direction: passthrough
```

Rejected: field mappings are source→target connections. A passthrough
field has no target. Overloading `FieldMapping` adds complexity for a
concept that's fundamentally different.

### B. Source-level `include` list

```yaml
sources:
  crm:
    table: crm_contacts
    primary_key: [email]
    include: [record_type, region_code]
```

Rejected: passthrough is a per-mapping concern, not a per-source concern.
Different mappings from the same source may need different passthrough
columns (e.g., route patterns where only one branch needs `contact_type`).

### C. Field mapping with no target

```yaml
fields:
  - source: record_type
```

Appealing syntax, but today `source` without `target` is used for
`reverse_only` fields (reverse_expression computed values). Reusing it
for passthrough would create ambiguity. A dedicated `passthrough` property
is clearer.

## Open questions

1. **Should passthrough columns have types?** Currently they'd be text
   (from `_base` JSONB extraction). If the ETL needs typed output, we could
   support `passthrough: [{name: record_type, type: text}, ...]`. Proposal:
   start with plain strings (always text), add typed variant if needed.

2. **Nested passthrough.** Should `passthrough: ["metadata.tier"]` work for
   JSONB sub-fields? The `source_path` mechanism already handles this in
   field mappings. Proposal: support dotted paths in passthrough using
   the same `json_path_expr` helper.

3. **Passthrough for nested-path mappings.** When a mapping uses
   `source.path` (nested arrays), should passthrough refer to parent-level
   or item-level columns? Proposal: parent-level only (the natural scope).
   Item-level columns should be mapped fields if they matter.
