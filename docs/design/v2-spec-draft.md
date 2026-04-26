# v2 Spec Draft

**Status:** Draft

A backend-neutral simplification of the OSI mapping schema, informed by lessons
from [opensync](https://github.com/BaardBouvet/opensync) and the goal of
supporting an RDF/SPARQL renderer alongside the existing PostgreSQL view
renderer.

This is a forward-looking proposal. Nothing here is implemented yet. See
[v2-migration-rfc.md](v2-migration-rfc.md) for a clause-by-clause migration
mapping and [v2-prototype-examples.md](v2-prototype-examples.md) for full
example translations.

## Design principles

1. **Two backends, one schema.** v2 targets exactly two renderers: SQL views
   (the v1 implementation) and SPARQL/RDF (the new triplestore renderer).
   The schema describes what the integration *means*; each renderer
   compiles it to its runtime. Additional backends (Datalog, RML, others)
   are not part of v2 and will be added later if real demand appears.
2. **One identity model.** Field-value matching with transitive closure
   (union-find). All other identity primitives collapse into this.
3. **Typed escape hatch for expressions.** All expressions — transforms,
   aggregations, filters — use a backend-keyed object with exactly two keys:
   `sql:` (consumed by the PG renderer) and `sparql:` (consumed by the
   triplestore renderer). The schema never carries a free string that one
   backend can interpret and another cannot. Curated expression primitives
   will be added after real usage patterns are established.
4. **Multi-file by folder, not by structure.** A mapping is either a single
   `mapping.yaml` or a folder of `.yaml` files. Teams choose the layout.
5. **Tests are the cross-backend conformance suite.** Every renderer must
   reproduce the same `updates`/`inserts`/`deletes` from the same input. The
   test format is the spec's teeth.
6. **Pre-1.0: no compatibility shims.** Renames and restructures land freely.
   v1 YAML does not load on v2.

## Document loading

A mapping is loaded from one of:

- A single file `mapping.yaml`
- A directory `mapping/` containing any number of `.yaml` files at any depth

When loaded from a directory, files are read in lexicographic path order and
merged by union at the top level. The merge rules are strict:

| Top-level key | Merge rule |
|---|---|
| `version` | Must appear in exactly one file; must be identical if repeated |
| `description` | Concatenated with newline separators |
| `sources` | Union by source name; duplicate names are an error |
| `targets` | Union by target name; duplicate names are an error |
| `mappings` | Concatenated; duplicate `name:` is an error |
| `tests` | Concatenated |

There are no required filenames and no required directory structure inside the
folder. Teams pick `by-entity/`, `by-source/`, `by-concern/`, or one big file.

## Document root

```yaml
version: "2.0"
description: optional human-readable summary
sources:    { ... }    # source dataset metadata
targets:    { ... }    # target entity definitions
mappings:   [ ... ]    # source-to-target field mappings
tests:      [ ... ]    # backend-neutral conformance tests
```

## Sources

```yaml
sources:
  crm:
    primary_key: id                       # change-detection key only
  erp_lines:
    primary_key: [order_id, line_no]      # composite PK supported
```

`primary_key` is **only** used for change detection and source-row
identification in renderer output. It is not the entity identity — that lives
on the target. This decoupling is the main change from v1.

**PK value types.** PK fields may be any scalar type the source supports
(string, integer, UUID, etc.). Values are compared natively for change
detection — the engine does not coerce types. A PK component that is
`null` on any row is a **runtime per-row error**; PKs must be non-null on
every row.

**Composite PK encoding.** When the engine needs to serialize a PK — for
use in IRIs, test-format tokens, `id_feedback` rows, or cross-source
references — it applies a **minimal escape** to each component and joins
composite components with `/`. The escape protects only the separator
itself: replace `%` with `%25`, then replace `/` with `%2F`. Two
`replace()` calls in either backend; no extension required, no shipped
function. The encoding is deterministic and reversible.

```
primary_key: id                  →  "42"          (integer 42, no escaping)
primary_key: id                  →  "abc-123"     (string, no escaping)
primary_key: [order_id, line_no] →  "42/A.1"      (no special chars; "." is safe)
primary_key: [order_id, line_no] →  "42/A%2F1"    (line_no = "A/1")
primary_key: [order_id, line_no] →  "42/A%2525"   (line_no = "A%25")
```

The SPARQL renderer wraps the final encoded PK in `ENCODE_FOR_URI` when
minting source IRIs (`<base>/source/<mapping_name>/<encoded_pk>`), so the
result is fully URI-safe by construction. The SQL renderer uses the
encoded string directly as a `text` value in `id_feedback` and any
cross-mapping reference column. The test format uses it as the value of
`_canonical_id` and any source-PK reference. Choosing minimal escape
over full URL percent-encoding keeps Postgres free of `urlencode`
extensions or hand-rolled regex.

> **Naming note (pre-implementation):** `primary_key` is a relational term.
> It will be renamed to `id_field` before the v2 spec is finalised — the new
> name is backend-neutral and makes the purpose clearer: "which field(s)
> identify a source row for change detection." Semantics and requirements are
> unchanged. All v2 examples use `primary_key` temporarily.

**Operational configuration is not in the schema.** Connection metadata
(driver, URL, credentials, pool settings), the SQL schema/view prefix used
by the SQL renderer, and the base IRI used by the SPARQL renderer are all
deployment concerns handled by the runtime, not by the mapping file.

Mapping authors write `sql:` and `sparql:` expressions against **logical
names**, never against deployment-specific identifiers:

- `sql:` blocks reference targets by their bare canonical name
  (`global_contact`, `global_order`). The SQL renderer rewrites these to
  fully qualified view names (`osi.v_global_contact` or whatever the
  deployment configures) at compile time.
- `sparql:` blocks reference targets and properties using reserved curies
  (`canonical:order`, `prop:person_ref`). The SPARQL renderer expands these
  to full IRIs using the deployment's base IRI at compile time. Reserved
  prefixes: `canonical:` (target IRIs), `prop:` (property IRIs),
  `source:` (source-graph IRIs).

A mapping file is fully portable: the same mapping deployed against two
different databases or two different RDF stores produces correctly named
views or correctly based IRIs without any change to the mapping itself.

> **Future direction (post-v2):** later versions may introduce optional
> schema-level annotations to customize generated artifacts — e.g. a per-
> target hint to override the generated view name, or a per-field
> annotation to bind a property to a specific external IRI like
> `schema:email`. These would live alongside the existing schema, not in
> the operational sidecar. They are deliberately out of scope for v2 to
> keep the initial schema small and the portability contract crisp.

## Targets

```yaml
targets:
  contact:
    description: optional
    identity:
      - email                              # OR-group, single field
      - [first_name, last_name, dob]       # OR-group, AND-tuple
    fields:
      email:      { strategy: coalesce }
      first_name: { strategy: coalesce }
      last_name:  { strategy: coalesce }
      dob:        { strategy: coalesce }
      phone:
        strategy: coalesce
        normalize: strip_non_digits
```

### Identity

Identity is declared at the target level. It is a list of **OR-groups**; each
group is either a single field name or a list of field names that must all
match together (AND-tuple).

Two source records contribute to the same canonical entity when any OR-group
produces a non-null shared value (single field) or shared tuple (AND-tuple).
Matching is transitive: if A↔B match on email and B↔C match on `(first, last,
dob)`, then A, B, C are the same entity.

**AND-tuples and null.** A record participates in an AND-tuple group only
when **every** field in the tuple is non-null. If any field is null on a
given record, that tuple is skipped for that record (the record may still
match on other OR-groups). This avoids spurious matches between records
that happen to share `(first_name="Alice", last_name=null)`.

Identity fields are **also** regular fields — they still appear in `fields:`
with their own resolution strategy. A field can be both an identity match key
and a coalesce target. (In v1, declaring a field as `strategy: identity`
implicitly removed it from normal resolution, which surprised users.)

### Resolution strategies

| Strategy | Purpose |
|---|---|
| `coalesce` | Best non-null by priority |
| `last_modified` | Most recently changed wins |
| `multi_value` | Multi-valued scalar attribute (tags, codes, labels) |
| `any_true` | Boolean fields: true if any source is true |
| `all_true` | Boolean fields: true if every contributing source is true |
| `expression` | Custom cross-source aggregation (see [Aggregation](#aggregation)) |

The `identity` strategy is **gone** — identity is declared at the target level.
Field-level `link_group` is **gone** — replaced by AND-tuples in the target
identity list.
The v1 `collect` strategy is **gone** — replaced by `multi_value` (for scalar
multi-valued data) or by a child target (for structured/identity-bearing
elements). See [Multi-valued fields](#multi-valued-fields).

All target field declarations use the **object form** `{ strategy: <name> }`.
The v1 string-shorthand form (`email: coalesce`) is removed in service of
"one canonical way to express each concept." Verbosity cost is a few extra
characters per field; consistency benefit is meaningful for AI-generated YAML.

### Multi-valued fields

v2 distinguishes two kinds of multi-valued data and forces a clean choice:

**Scalar multi-value** — elements are bare scalars with no identity, no own
attributes. Tags, ISO codes, labels, ISBN lists. Use `strategy: multi_value`:

```yaml
targets:
  contact:
    fields:
      tags:
        strategy: multi_value
        element_type: string         # required: string | numeric | integer | boolean
```

Semantics: the resolved value is the **deduplicated union** of every value
contributed by every source for this field. **Element order is undefined**
— renderers may emit elements in any order, and consumers that need a
defined order must sort. No duplicates, no per-source priority. This is
the v1 `collect` strategy under a clearer name.

In the PG backend `multi_value` materializes as `text[]` / `numeric[]`. In
the triplestore backend it materializes as multi-valued predicates (a bag of
triples).

If you need ordering, duplicates, or per-source winner-take-all, the
elements aren't really scalars — use a child target.

**Structured multi-value** — elements are objects with their own attributes,
identity, or references. Order line items, addresses, contacts. Use a
**child target** with `parent:` and `array:`. See
[Nested data](#nested-data-parent--array).

The choice is mechanical: scalar elements → `multi_value`; structured elements
→ child target. There is no third option.

### Field properties

```yaml
fields:
  primary_contact:
    strategy: coalesce
    references: company                    # type info: this field references a company entity
    default: { value: null }               # static literal
    # OR
    default: { sql: "first_name || ' ' || last_name' " }   # computed
    group: addr                            # atomic resolution group
    type: numeric                          # type hint for aggregation
    description: optional
```

Unchanged from v1 in semantics: `references`, `group`, `type`. 

**Changed:**

- `default` and `default_expression` are **collapsed into a single `default:`
  property** that always takes an object. `{ value: <literal> }` for static
  defaults; `{ sql: ... }` / `{ sparql: ... }` for computed.
  Literal-scalar shorthand (`default: true`) is removed. `value:` accepts
  YAML scalars only (string, number, boolean, null); for complex defaults use
  the expression form.
- `references_field` is **gone from target field properties**. It was
  misplaced in v1 — it describes how a *source* represents a reference, not
  what the *target* references. It moves to the mapping-level `lookup_field`
  (see [Mappings](#mappings)).

#### `type:`

Small portable type vocabulary. Renderers map these to native types.

| Value | Meaning |
|---|---|
| `string` (default) | Text |
| `numeric` | Decimal number (precision-preserving) |
| `integer` | Integer |
| `boolean` | Boolean |
| `date` | Calendar date, no time component |
| `datetime` | Timestamp |
| `reference` | IRI / FK to another target (used implicitly when `references:` is set) |

No backend-specific datatypes (XSD, PG-specific) in the core type. Renderers
that need finer types (`xsd:gYear`, PG `interval`) use the per-backend escape
hatch in `transform:` to cast.

#### `group:`

Atomic resolution group. All fields sharing the same `group:` value resolve
from the **same winning source** — the source with the most recent timestamp
(under `last_modified`) or highest priority (under `coalesce`) across any
field in the group wins for *all* fields in that group.

Use case: address fields where `street`, `city`, `zip`, `country` must come
from one source, never mixed.

```yaml
fields:
  street: { strategy: last_modified, group: addr }
  city:   { strategy: last_modified, group: addr }
  zip:    { strategy: last_modified, group: addr }
```

`group:` is incompatible with `strategy: multi_value`: atomic group
resolution picks one winning source for the whole group, while
multi-valued fields aggregate across all sources. Combining the two is a
load-time error.

All fields in a `group:` must share `direction: bidirectional` (the
default). Mixing `forward_only` or `reverse_only` fields into a group is
a load-time error — partial-direction members would leave the group's
reverse pass underspecified.

#### `direction:`

Controls whether a field participates in forward (source→target) and reverse
(target→source) mapping.

| Value | Meaning |
|---|---|
| `bidirectional` (default) | Read from sources, written back to sources |
| `forward_only` | Read from sources, never written back (computed targets, derived values) |
| `reverse_only` | Never read from sources, only written (constants, defaults, system-generated) |

Declared on field mappings (mapping-level), not on target fields. The
direction is a property of *that source's relationship* to the field, not of
the target itself.

## Mappings

```yaml
mappings:
  - name: crm
    source: crm
    target: contact
    priority: 1                            # mapping-level coalesce priority
    last_modified: updated_at              # mapping-level timestamp
    filter:                                # forward filter
      sql: "status = 'active'"
    reverse_filter:                        # reverse filter
      sql: "type LIKE '%customer%'"
    fields:
      - source: email
        target: email
      - source: first_name
        target: first_name
      - source: phone
        target: phone
        transform:
          sql: "regexp_replace(phone, '[^0-9]', '', 'g')"
      - source: company_id
        target: primary_contact
        lookup_source: crm_company         # this ID belongs to crm_company's namespace
        lookup_field: id                   # optional: which field of looked-up source to return
```

Field mapping properties (`source`, `target`, `priority`, `last_modified`,
`default`, `direction`) are unchanged from v1 in semantics.

**Renamed from v1:**

| v1 (mapping-level) | v2 (mapping-level) | Why |
|---|---|---|
| `references` | `lookup_source` | The mapping-level `references` is namespace info ("this ID lives in source X"), not a reference declaration. Reusing the word collided with the target-level `references` (type info). |
| `references_field` | `lookup_field` | Parallels `lookup_source`. |

The target-level `references:` keeps its name — it *is* a reference declaration.
The mapping-level pair (`lookup_source` / `lookup_field`) describes how to
reverse-translate a canonical ID back to the source's local representation.

When `lookup_field:` is omitted, it defaults to the looked-up source's
`primary_key` (or `id_field` after the planned rename) — the natural choice
for most FK relationships.

#### Deferred references

On reverse pass, a `lookup_source:` translation can fail when the
target entity exists in canonical state but has not yet been written to
the looked-up source (no row in `id_feedback` carries its source-local
ID). In that case the renderer **defers the FK**: the row is emitted
with the FK field omitted (or null, depending on the source schema's
tolerance), and the engine retries the FK on the next sync cycle once
the referenced row has been written and reported its local ID.

This behavior matches opensync's pending-edge resolution and prevents
ordering bugs from blocking the entire sync. Strict-FK targets that
cannot accept a deferred null must declare the column NOT NULL on the
source side; the renderer then surfaces a per-row write error and skips
the row until resolvable. Deferred resolution applies only to FK
fields produced by `lookup_source:`; ordinary fields are unaffected.

### Source paths

`source:` is a single key. For flat sources it's the bare field name; for
nested sources (JSON documents, RDF, hierarchical APIs) it's a path. The
same key handles both — the parser decides at load time whether the value
is a single token or a path expression.

The path grammar is intentionally tiny and aligned with JSONPath / jq /
JS bracket notation, but is **not JSONPath**. Tokens, applied left-to-right:

| Token | Example | Meaning |
|---|---|---|
| key | `address` | `obj[key]` (object key lookup) |
| integer index | `[0]` | `arr[0]` (zero-based array index) |
| quoted string key | `['foo.bar']` or `["foo.bar"]` | literal key lookup; required when the key contains `.`, `[`, or `]`, or is the empty string `['']` |

```yaml
fields:
  - source: customer_id          # flat: bare key
    target: customer_id
  - source: address.street       # nested: dotted path
    target: street
  - source: lines[0].product_id  # array index then key
    target: first_product_id
  - source: "['address.type'].code"   # bracketed escape for a key with a dot
    target: address_type_code
  - source: "['my.field']"       # flat-mode equivalent for a column literally named "my.field"
    target: my_field
```

A bare single-token value (`source: customer_id`) is **flat-mode**:
identical to a top-level column lookup, with no nested-reverse machinery.
A value containing `.` (outside a bracket) or `[` triggers **path-mode**:
the engine walks the path forward and reconstructs the nested structure
on reverse.

Rules:

- Missing intermediates resolve to null and fall through to `default:`
  if present.
- Path-mode is fully reversible for object keys (bare and quoted): the
  engine auto-reconstructs the nested object on reverse, merging sibling
  paths under the same root
  (`address.street` + `address.city` → `{ address: { street, city } }`).
- **Array-index reverse is rejected at load time.** Any `[N]` segment in
  a `source:` path means the field cannot be written back — array
  positions are ambiguous on write. Use a child mapping with `array:`
  for reversible array handling, or declare `direction: forward_only` to
  acknowledge the field is read-only.
- The same grammar is used by `array:` on a child mapping for the path
  to the array column or nested array (e.g. `array: data.lines[0].items`).
- No wildcards, filters, or recursive descent. Anything that wants
  JSONPath belongs in the escape hatch.

This grammar also handles **JSON-LD-shaped source data** — sources that
expose per-field metadata as nested objects (e.g.
`{ name: { value: "Alice", lastModified: "..." } }`) — with no special
schema feature. `source: name.value` reads the value, `source: name.lastModified`
reads the timestamp; both round-trip through the same nested object on
reverse.

### Nested data: parent / array

`parent:` declares an embedded sub-entity (flat columns from the same row) or
array expansion (`array:`) with `parent_fields:` carrying ancestor data into
scope. Semantics unchanged from v1.

**Changed:** v1 had two properties — `array:` for a single column name and
`array_path:` for a dotted path into a nested object. They are collapsed in v2
into a single `array:` that uses the [Source paths](#source-paths) grammar
above. A bare segment (`array: lines`) and a multi-segment path
(`array: data.lines[0].items`) use identical syntax. Bracket-quoted keys
handle column names that literally contain `.` or `[` (`array: "['data.v2']"`).

#### Embedded sub-entity (flat columns)

```yaml
mappings:
  - name: order_header
    source: orders
    target: order
    fields:
      - { source: order_id,    target: order_id }
      - { source: total,       target: total }

  - name: order_address
    parent: order_header                       # inherits source: orders
    target: shipping_address                   # different target entity
    fields:
      - { source: ship_street, target: street }
      - { source: ship_city,   target: city }
```

#### Array expansion (one row per element)

```yaml
  - name: order_lines
    parent: order_header
    array: lines                               # column or path holding the array
    parent_fields:
      parent_order_id: order_id                # bring parent's order_id into scope
    target: order_line
    fields:
      - { source: parent_order_id, target: order_ref, lookup_source: order_header }
      - { source: line_num,        target: line_number }
      - { source: item,            target: product }
```

#### Deep nesting (chain of parents)

```yaml
  - name: lines, parent: order_header, array: lines, target: order_line
    fields: [...]
  - name: line_taxes, parent: lines, array: taxes, target: line_tax
    fields: [...]
```

#### Scalar arrays

When the array elements are bare scalars (not objects), use `scalar: true`
and declare the wrapper field name. The engine wraps each element as
`{ value: <scalar> }` internally:

```yaml
  - name: order_tags
    parent: order_header
    array: tags                                # tags = ["urgent", "vip", ...]
    scalar: true
    target: order_tag
    fields:
      - { source: value, target: tag_name }
```

Most scalar-array uses are better expressed via `multi_value` on the parent
target (see [Multi-valued fields](#multi-valued-fields)) — only fall back to
scalar-array child mappings when the elements need their own write-back path
or identity.

#### Element ordering

For child mappings, three ordering strategies:

| Property | Behavior |
|---|---|
| `sort:` (list of `{field, direction}`) | Static field-based ordering on reverse reconstruction |
| `order: true` | Inject zero-padded ordinal from source array position; participates in resolution like any other field |
| `order_prev: true` / `order_next: true` | Linked-list ordering: emit identity of preceding / following element via window functions. CRDT-friendly. |

Mutually exclusive: a single child mapping uses one strategy.

```yaml
  - name: order_lines
    parent: order_header
    array: lines
    target: order_line
    sort:
      - { field: line_number, direction: asc }
    fields: [...]
```

### Passthrough

A mapping may declare `passthrough:` to carry source columns straight
through to the renderer's delta output without participating in identity,
resolution, or canonical storage. Use it for audit/lineage columns
(`updated_by`, `source_record_id`), routing or correlation IDs (Kafka
offsets, batch IDs), and diagnostic context that downstream consumers
need alongside the canonical record but that isn't part of the canonical
model.

Unlike systems with a connector layer where unmapped-column merge is
handled at the connector's PUT/PATCH boundary, OSI-mapping has no such
boundary — the renderer's delta output **is** the integration boundary.
Whatever the schema doesn't surface, downstream consumers cannot get.
`passthrough:` is the declarative way to surface non-canonical columns
without polluting the canonical entity.

```yaml
mappings:
  - name: orders_in
    source: orders
    target: order
    passthrough:
      - source: external_ref          # carry verbatim
      - source: kafka_offset
        as: _offset                   # rename in delta output
    fields:
      - { source: id, target: order_id }
      ...
```

Rules:

- Listed source columns are emitted unchanged in the renderer's delta
  output for this mapping. **Forward-pass only** — passthrough columns
  do not participate in the reverse pass.
- `as:` is optional and defaults to the source column name.
- Passthrough columns are **not** part of the canonical entity, **not**
  resolved across sources, **not** subject to `transform:`, `default:`,
  `normalize:`, or any predicate. They bypass the entire mapping pipeline.
- Passthrough is **per-mapping**. The same source-column name appearing
  in two mappings produces two independent passthrough streams (one per
  delta output).
- Output names starting with `_canonical_` are reserved and rejected at
  load time.
- Backend lowering: SQL renderer adds the column to the generated delta
  view's projection. SPARQL renderer emits the value as triples on the
  source-graph IRI under a reserved `passthrough:` predicate prefix.

Passthrough is intentionally a thin escape valve, not a transform layer.
If the consumer needs the value reshaped, do that downstream.

### ETL feedback (kept in core)

| v2 name | v1 name | Purpose |
|---|---|---|
| `id_feedback` | `cluster_members` | Separate table/graph of `(canonical_id, source_id)` pairs written by the connector after inserts |
| `id_feedback_field` | `cluster_field` | Inline column on the source carrying the canonical ID |
| `written_state` | `written_state` | Tracking what the connector last wrote to the target |
| `suppress_unchanged_writes` | `derive_noop` | Skip update dispatch when the resolved value matches the last-written value |
| `track_field_timestamps` | `derive_timestamps` | Synthesize per-field timestamps from the engine's own write log when sources don't carry them |
| `tombstone_field` | `derive_tombstones` | Target field to synthesize as a deletion marker when an entity disappears from the source snapshot |

These describe an **ETL contract** — what state the connector maintains across
runs. The contract is meaningful in any backend; the implementation differs
(PG: tables; RDF: named graphs).

The corresponding test-format token `_cluster_id` is renamed to
`_canonical_id`.

## Expressions

Three distinct expression roles appear in the schema. v2 separates them by
property name:

| Property | When it runs | Produces | Used with |
|---|---|---|---|
| `transform:` | Per source row, before resolution | A value | Any field mapping |
| `aggregate:` | Across all contributing rows during resolution | A value | Only `strategy: expression` |
| `filter:` / `reverse_filter:` | Per row, decides row inclusion | A boolean | Any mapping |

Value-producing expressions (`transform:`, `aggregate:`) and `default:` use
the **backend-keyed escape hatch**, plus a small **curated transform
vocabulary** for the handful of value transforms that survive the
bidirectionality test (see [Curated transforms](#curated-transforms)).
`aggregate:` and `default:` are escape-hatch-only — cross-source
aggregation is too paradigm-specific and defaults are by definition
literals.

`filter:` and `reverse_filter:` accept either the escape hatch or a small
**curated predicate vocabulary** (see [Predicates](#predicates)) because
boolean predicates appear in essentially every non-trivial mapping and are
cleanly portable across both backends.

**`aggregate:` is forward-pass only.** It runs during cross-source
resolution and has no reverse counterpart — a `strategy: expression` field
is read-only with respect to write-back. Declaring `direction:` on a field
that uses `aggregate:` is a load-time error: the only sensible direction is
implicitly `forward_only`.

### Direction interactions

The `direction:` guard runs first, before any value processing.

- A `forward_only` field skips the entire reverse pass (steps 1–5).
  `normalize:` therefore has no effect on `forward_only` fields. Reverse
  transforms (paired `split` for `join`, reverse of `cast`, escape-hatch
  reverse expressions) are not invoked. Declaring them is permitted but
  silently inert; renderers must not warn.
- A `reverse_only` field skips the forward pass. `transform:` is not
  invoked; `default:` only runs on the reverse pass when the canonical
  value is null.
- All curated transforms (`cast`, `join`/`split`) and `normalize:`
  respect the direction guard — they never run for a direction the
  field has opted out of. The grammar still validates at load time so
  that a future direction change does not silently produce nonsense.

### Order of operations

For a single field mapping, the engine processes a value in a fixed order.
This ordering is part of the contract; renderers must not reorder.

**Forward pass (source → canonical):**

1. Read raw source value (via `source:`, with bare-key or path-grammar value).
2. Apply `transform:` (curated primitive or escape hatch) if present.
3. If the result is null/absent, apply `default:` if present.
4. Carry into resolution (per `strategy:` and `priority:`).

**Reverse pass (canonical → source):**

1. Read canonical value.
2. Apply paired reverse transform (`split` for `join`; reverse of
   `cast`; reverse expression for escape-hatch transforms).
3. Apply `reverse_filter:` to decide whether to emit the write at all.
4. Compare against last-written shadow with `normalize:` applied to both
   sides. If equal, suppress the write.
5. Otherwise, emit the write.

`normalize:` runs only at step 4 of the reverse pass. It never modifies
values stored in canonical or written to source; it is a diff lens.

### NULL = unknown

v2 uses three-valued logic. Any expression with a null/unbound operand
evaluates to null. `not_null` is the only primitive that converts a possibly
null value to a definitely non-null boolean.

**Boolean operators follow SQL three-valued logic:**

- `and` returns `true` only if all operands are `true`; `false` if any
  operand is `false`; `null` otherwise (one or more nulls, no false).
- `or` returns `true` if any operand is `true` (short-circuit); `false`
  if all operands are `false`; `null` otherwise.
- `not` of `null` is `null`.

Predicates appearing in `filter:` / `reverse_filter:` reject rows whose
result is null — consistent with SQL `WHERE` and SPARQL `FILTER`. Both
backends implement this natively; the renderer does not need to inject
explicit `IS NOT NULL` / `BOUND()` checks.

**`not_null` and `is_null` semantics:**

- `not_null: { field: x }` returns `true` if `x` is bound and non-null;
  `false` otherwise (never returns null).
- `is_null: { field: x }` returns `true` if `x` is null or unbound;
  `false` otherwise (never returns null).

**Aggregation strategies and null:**

- `any_true` returns `true` if any contributing source is `true`
  (short-circuit); `false` if every contributing source is `false`;
  `null` if every contributing source is `null`. Boolean-typed fields
  only; load-time error otherwise.
- `all_true` returns `true` if every contributing source is `true`;
  `false` if any contributing source is `false`; `null` if every
  contributing source is `null`. Boolean-typed fields only; load-time
  error otherwise. "Contributing" excludes sources that don't map this
  field at all — a missing mapping never counts as `false`.
- `multi_value` filters null out before union. If every source
  contributes null, the resolved value is `null` (absent), not an empty
  set. `default:` applies normally.

### Bidirectionality

Every curated primitive in v2 is bidirectional. Predicates apply
symmetrically to both directions (the same condition is true on both
sides). Escape-hatch expressions are **forward-only by default**: the
renderer applies them on the forward pass and skips the field on the
reverse pass. To make an escape-hatch expression participate in the reverse
pass, declare a paired reverse expression on the field mapping.

### Escape hatch shape

```yaml
# Per-row value transform
fields:
  - source: phone
    target: phone
    transform:
      sql:    "regexp_replace(phone, '[^0-9]', '', 'g')"
      sparql: "REPLACE(?phone, '[^0-9]', '')"

# Cross-source aggregation (strategy: expression only)
fields:
  score:
    strategy: expression
    type: numeric
    aggregate:
      sql:    "max(score)"
      sparql: "(MAX(?score) AS ?score)"

# Row filter (escape-hatch form)
filter:
  sql:    "status = 'active'"
  sparql: "FILTER (?status = 'active')"
```

Each renderer reads only its own key. A field mapping that has no key for
your backend is a **load-time error** in that renderer; the other renderer
is unaffected. This applies uniformly to `transform:`, `aggregate:`,
`default:`, `filter:`, `reverse_filter:`, and the escape-hatch form of
`normalize:`. Both backend keys are required whenever the escape-hatch
shape is used.

Optionally, an escape-hatch block may declare `sources: [...]` listing the
field names the expression reads. This is a **lineage hint** for tooling
(dependency analysis, partial recompute) — not enforced semantically by
the renderer. Curated primitives infer this automatically.

```yaml
transform:
  sources: [first_name, last_name]
  sql:    "first_name || ' ' || last_name"
  sparql: "CONCAT(?first_name, ' ', ?last_name)"
```

**Backend keys are fixed in v2.** Exactly two are valid: `sql:` (PG renderer)
and `sparql:` (triplestore renderer). Any other key in an expression block
is a validation error. Adding a third backend in a future spec version
will add a third key.

### Predicates

Predicates produce a boolean and appear in `filter:` and `reverse_filter:`.
A predicate block is **either** a single curated predicate object **or**
the backend-keyed escape hatch — not both at the same level. Mixing the
two requires nesting via a boolean op:

```yaml
filter:
  and:
    - { equals: { field: status, value: "active" } }
    - { sql: "...", sparql: "..." }       # escape hatch as one branch
```

Curated predicate vocabulary. Every predicate uses a uniform shape:
unary predicates take `{ field: <name> }`; binary comparisons take
`{ field: <name>, value: <literal> }`; n-ary list predicates take
`{ field: <name>, values: [...] }`; boolean operators take a list of
nested predicates (or a single predicate for `not`).

| Primitive | Shape | Description |
|---|---|---|
| `equals` | `{ field, value }` | Field equals the literal |
| `not_equals` | `{ field, value }` | Field does not equal the literal |
| `in` | `{ field, values }` | Field is in the list |
| `not_null` | `{ field }` | Field is bound and non-null |
| `is_null` | `{ field }` | Field is null or unbound |
| `gt` / `gte` | `{ field, value }` | Strictly / non-strictly greater than |
| `lt` / `lte` | `{ field, value }` | Strictly / non-strictly less than |
| `and` | `[ <predicate>, ... ]` | All sub-predicates true |
| `or` | `[ <predicate>, ... ]` | Any sub-predicate true |
| `not` | `<predicate>` | Logical negation |

Predicates nest arbitrarily.

```yaml
filter:
  and:
    - { equals: { field: status, value: "active" } }
    - { in:     { field: country, values: ["NO", "SE", "DK"] } }

reverse_filter:
  not_null: { field: account }

filter:
  or:
    - { not_null: { field: name } }
    - { not_null: { field: org_number } }

filter:
  not:
    or:
      - { equals:   { field: type, value: "draft" } }
      - { is_null:  { field: published_at } }
```

The curated set is intentionally minimal. Anything outside it (regex,
string matching, date arithmetic) uses the backend-keyed escape hatch.
Regex is explicitly **not** curated — PCRE and XPath dialects differ
enough that a portable form would silently mislead.

### Curated transforms

A tiny set of value transforms qualify as curated primitives because each
one is **bidirectional, lossless under documented preconditions, and
lowers cleanly to both backends**. They appear under `transform:` on a
field mapping.

| Primitive | Form | Forward | Reverse |
|---|---|---|---|
| `cast` | `cast: <type>` | Parse source value as the target type | Render canonical value back to source's string form (or whatever the source column actually accepts) |
| `join` / `split` | `join: { sep: " " }` paired with `split: { sep: " ", limit: N }` | Concatenate `sources:` with `sep` | Split target value on `sep` (at most `limit-1` times; the last bucket gets the tail) and assign back to `sources:` |
| `value_map` | `value_map: { Y: true, N: false }` (forward dict; reverse auto-derived when bijective) | Look up source value in the dict; emit canonical value | Reverse-look-up canonical value; emit source value |

`value_map:` is specified in full in [value-map-rfc.md](value-map-rfc.md) —
field-level only, mutually exclusive with `transform:`, fallback policy via
`value_map_fallback: passthrough | null`, null bypasses the map. Boolean
translation (`Y`/`N` → `true`/`false`) and code-list translation are its
primary use cases.

#### `cast`

Closed type list: `integer`, `numeric`, `boolean`, `date`, `timestamp`,
`string`.

**Accepted input formats (forward pass):**

| Target type | Forward accepts | Reverse renders |
|---|---|---|
| `integer` | Integer literals; numeric strings parsing to whole numbers (no fractional part). Reject strings with decimals or non-digits. | Decimal-free string. |
| `numeric` | Numeric literals; numeric strings (`"."` decimal separator). Reject locale-formatted numbers (`,` decimal, thousands separators). | Canonical decimal string with no thousands separator. |
| `boolean` | Native booleans; the literal strings `"true"` / `"false"` (case-sensitive). Reject `"1"` / `"0"` / `"Y"` / `"yes"` and similar — use `value_map:` or the escape hatch for those. | The literal string `"true"` or `"false"`. |
| `date` | ISO 8601 calendar date `YYYY-MM-DD`. Reject partial dates (year-only, year-month) and locale formats. | `YYYY-MM-DD`. |
| `timestamp` | ISO 8601 datetime `YYYY-MM-DDTHH:MM:SS[Z|±HH:MM]`. Sub-second precision and timezone abbreviations (`EST`) are rejected. Naive timestamps (no offset) are accepted and treated as UTC. | `YYYY-MM-DDTHH:MM:SSZ` (UTC). |
| `string` | Anything; `cast: string` is the conventional way to coerce a non-string source value to its canonical string form. | The canonical string verbatim. |

**Failure modes:**

- Forward parse failure produces null (consistent with NULL = unknown).
  No error is raised; the row continues with a null value for this field.
- Reverse render failure is a **runtime per-row error**: the renderer
  logs canonical id, source field, value, and reason, and skips the
  write for that row. Other rows continue.

**Bidirectionality preconditions:** `cast` is bijective only when the
source column can store the target type's full range and precision. If
the source has narrower precision (e.g. `NUMERIC(5,2)`) the round-trip is
lossy; the schema author must either pair with a `normalize:`
(comparison-adapter) primitive that mirrors the loss or accept that
certain values will trigger spurious writes.

```yaml
- source: amount_str
  target: amount
  cast: numeric

- source: account_id
  target: account_id
  cast: integer        # source stores "00000042"; canonical stores 42
```

#### `join` / `split`

Paired primitives that handle the most common name- and address-style
splits. They are **inverses by construction** when declared together:
`join` runs forward (multiple source fields → one canonical field) and
`split` runs reverse (one canonical field → multiple source fields).

```yaml
- target: full_name
  sources: [first_name, last_name]
  join:  { sep: " " }
  split: { sep: " ", limit: 2 }   # "Mary Beth Smith" → first="Mary", last="Beth Smith"
```

Rules:

- `sep` is a **literal string**, not a regex.
- `limit: N` caps the number of buckets; element `N-1` (last) absorbs all
  remaining text including subsequent separators. `limit: 1` is invalid.
- **No escaping or quoting is performed.** If a source value contains
  `sep`, the canonical value will be ambiguous on reverse split. Schema
  authors must guarantee this cannot happen — typically by adding a
  `filter:` or by choosing a separator that cannot appear in the data.
- **Forward null handling:** if any source field is null, the join uses
  an empty string for that segment and emits the result anyway. The
  schema author who needs strict null propagation must add a
  `filter: { not_null: { field: <source> } }` paired with the join.
- **Reverse null handling:** if the canonical value is null, all source
  fields are written as null. If the canonical value contains fewer than
  `limit-1` instances of `sep`, trailing buckets are written as **null**
  (not empty string). Example: canonical `"Mary"` with `sep: " "` and
  `limit: 3` produces `[source1="Mary", source2=null, source3=null]`.
- **`join` is not fully lossless across nulls.** A source value of null
  and a source value of `""` both forward-join to the same empty
  segment, and reverse-split cannot distinguish them. The reverse pass
  prefers null. Schema authors who need to round-trip empty strings
  faithfully must use the escape hatch.
- Sources listed in a `join` / `split` mapping must all have
  `direction: bidirectional` (the default). Mixing `forward_only` /
  `reverse_only` on individual sources is a load-time error — use the
  escape hatch and write the directions explicitly when asymmetry is
  required.
- Backend lowering: `concat_ws` / `string_to_array` in PG, `CONCAT` /
  `STRSPLIT` in SPARQL.

This primitive is **not** an address parser, **not** a person-name
parser, and **not** locale-aware. "van der Berg" splits wrong. So does
"Mary Jane Smith" with `limit: 2`. That's the price of staying
bidirectional and portable; libpostal-grade parsing is escape-hatch
territory and probably an external service.

### `default:`

Static defaults use `{ value: <literal> }` — not an expression, just a YAML
scalar wrapped for unambiguous parsing. Computed defaults use the escape
hatch:

```yaml
default: { value: null }
default: { value: "unknown" }
default: { value: 0 }
default: { sql: "now()", sparql: "NOW()" }
```

The literal in `default: { value: ... }` must be compatible with the
field's declared `type:`. Type mismatch is a load-time error: the loader
rejects `default: { value: "text" }` on a `numeric` field, etc.
Use a computed default (`default: { sql, sparql }`) when the literal
doesn't fit.

`transform:` accepts exactly one curated primitive **or** one
escape-hatch block, never a list. To chain logic, nest it inside the
escape hatch: `transform: { sql: "outer(inner(x))", sparql: "..." }`.
The list form is reserved for `normalize:`, where left-to-right
composition models real source-side lossiness pipelines (see
[`normalize:`](#normalize-comparison-adapter-not-a-transform) below).

### `normalize:` (comparison adapter, not a transform)

`normalize:` is a per-field property declared on a field mapping. It is
**not** a value transform — it does not change the value written to
canonical or back to source. It is a **diff-time comparison adapter**:
when the engine compares a canonical value to a source value (for
no-op detection during reverse pass), both sides are run through
`normalize:` first, and rows where the normalized values agree are
suppressed.

The canonical use cases are all forms of **lossy source-side storage**:
a system that uppercases everything on write, truncates strings to N
characters, stores only integers, caps numeric range, drops Unicode, and
so on. Without `normalize:`, every reverse-pass dispatch would compare a
faithful canonical value against the lossy round-tripped value and look
like a real change \u2014 producing infinite update loops. With `normalize:`,
the engine applies the source's lossy lens to *both* sides of the diff and
suppresses the write when the normalized forms agree.

```yaml
- source: name
  target: name
  normalize: uppercase

- source: short_label
  target: short_label
  normalize:                  # uppercase, then cap at 50 chars
    - uppercase
    - { truncate: { length: 50 } }

- source: price
  target: price
  normalize: { round: { decimals: 2 } }     # source stores 2dp
```

`normalize:` accepts either a single primitive (string or single-key
object) or a **list** of primitives applied left-to-right. Composition
matters: real systems uppercase *then* truncate, or trim *then* fold to
ASCII, and the diff lens must mirror that order.

### Curated primitives

Each primitive models a category of source-side lossiness. The set is
closed; new primitives require a spec change.

**String normalisation:**

| Primitive | Form | Meaning |
|---|---|---|
| `lowercase` | bare | Unicode-aware case fold to lower |
| `uppercase` | bare | Unicode-aware case fold to upper |
| `trim` | bare | Strip leading and trailing whitespace |
| `unicode_nfc` | bare | Unicode NFC normalization |
| `ascii_fold` | bare | Strip diacritics; fold to nearest ASCII (source has no Unicode support) |
| `truncate` | `{ length: N }` | Cap string to N characters (source has a column length limit) |
| `pad` | `{ width: N, char: \" \", side: left }` | Pad short strings with `char` to width `N` (source stores fixed-width strings, e.g. `CHAR(N)` columns or zero-padded IDs). `char` defaults to space, must be a single character. `side` is `left` or `right`, default `left`. The diff lens applies the same pad to both sides before comparison. |\n| `strip_non_digits` | bare | Remove all non-digit characters (extract numeric phone/code from formatted string) |

**Numeric normalisation:**

| Primitive | Form | Meaning |
|---|---|---|
| `round` | `{ decimals: N }` | Round half-to-even to N decimals (source stores fixed precision; `decimals: 0` for integer-only systems) |
| `truncate` | `{ decimals: N }` | Truncate toward zero to N decimals (source stores fixed precision via truncation, not rounding) |
| `clamp` | `{ min: a, max: b }` | Clamp to `[a, b]` (source caps the numeric range) |

**Date / time normalisation:**

| Primitive | Form | Meaning |
|---|---|---|
| `truncate_time` | `{ precision: year \| month \| day \| hour \| minute \| second }` | Truncate to the start of the named unit (source stores `2024` against canonical `2024-03-15T10:22:11Z`, etc.) |

**Null normalisation (any type):**

| Primitive | Form | Meaning |
|---|---|---|
| `null_as` | `{ value: <literal> }` | Source cannot store null; null is silently stored and read back as the given literal. Both sides of the diff replace null with the literal so a canonical `null` no longer looks different from a round-tripped `false` / `0` / `""`. Type of the literal must match the field's declared type. |

`null_as` is the only primitive that crosses type domains \u2014 because the
inability to store null is a property of the source's type system, not of
any single field type. It's also the only primitive that touches the
`NULL = unknown` rule: it explicitly **opts a single field out** of Kleene
semantics on the diff side, because the source has no way to honour them.
The canonical value is unaffected; only the comparison lens changes.

`truncate` is overloaded by parameter shape: `length:` is the string form,
`decimals:` is the numeric form. A field mapping that uses the wrong shape
for the field's declared type is a load-time error.

String primitives on a numeric or date field, numeric primitives on a
string or date field, and date primitives on a string or numeric field are
all load-time errors.

Time-zone normalisation is intentionally **not** in the curated set. Real
TZ semantics depend on whether the source stores naive local time, an
offset, or a named zone, and getting it wrong silently corrupts the diff.
Use the escape hatch when needed.

### Escape hatch

When no curated primitive captures the lossiness, fall back to the
backend-keyed escape hatch. The expression takes a single positional
placeholder `{}` for the value being compared:

```yaml
- source: amount
  target: amount
  normalize:
    sql:    "trunc({}::numeric, 0)"
    sparql: "FLOOR({})"
```

Both backend keys must be present when the escape hatch is used. Both
sides of the diff are wrapped in this expression. Escape-hatch entries
may appear inside a list alongside curated primitives \u2014 the entire list
is applied left-to-right.

`normalize:` is the only curated value-side concept v2 ships, and only
because comparison-adapter logic doesn't fit the transform / aggregate /
predicate model — it has its own evaluation slot in the diff pipeline.

### Cross-entity aggregation

Cross-entity expressions (counting or aggregating related entities) use
`strategy: expression` with an `aggregate:` escape hatch, same two-key
rule as all other escape hatches. Both backends can express cross-entity
aggregation; the syntax is paradigm-different but the semantics are
equivalent.

In the `sql:` form the renderer injects the canonical view names (e.g.
`global_order`, `global_person`). In the `sparql:` form the renderer
injects `?canonicalEntity` as the IRI binding for the current entity.

```yaml
order_count:
  strategy: expression
  type: integer
  aggregate:
    sql: |
      COALESCE((
        SELECT count(*)
        FROM global_order o
        WHERE o.person_ref = global_person.person_id
      ), 0)
    sparql: |
      (COUNT(?order) AS ?order_count)
      WHERE {
        ?order a canonical:order .
        ?order prop:person_ref ?canonicalEntity .
      }
```

The SQL form references the bare logical view name (`global_order`,
`global_person`); the SQL renderer rewrites it to the fully qualified
deployment name. The SPARQL form uses the reserved `canonical:` and
`prop:` curies; the SPARQL renderer expands them against the deployment's
base IRI. `?canonicalEntity` is bound by the renderer to the current
entity's IRI. Nothing deployment-specific appears in the mapping.

## Tests

The test format is unchanged from v1 in structure. Backend-neutral semantics:

```yaml
tests:
  - description: "Shared contact — CRM name wins"
    input:
      crm:
        - { id: "1", email: "a@x.com", name: "Alice" }
      erp:
        - { id: "100", contact_email: "a@x.com", contact_name: "A. Smith" }
    expected:
      erp:
        updates:
          - { id: "100", contact_email: "a@x.com", contact_name: "Alice" }
```

Every renderer must produce identical `updates`/`inserts`/`deletes` from the
same input. This is the conformance contract. The triplestore renderer
projects its named graphs back to source-PK-shaped records to satisfy it; see
[triplestore-backend.md](triplestore-backend.md).

## Removed from v1

| v1 | v2 replacement |
|---|---|
| `strategy: identity` on field | `identity:` list at target |
| `link_group: name` on field | AND-tuple in target `identity:` |
| `strategy: collect` | `strategy: multi_value` (scalar elements) or child target (structured elements) |
| `links:` on mapping | Kept, with cleaned-up syntax (see [Linkage mappings](#linkage-mappings)) |
| `link_key:` on mapping | Dropped — operational concern, handled by the runtime, not the mapping |
| String shorthand for strategy (`name: coalesce`) | Object form only (`name: { strategy: coalesce }`) |
| `expression: "SQL string"` | `transform: { sql: "..." }` (per-row) or `aggregate: { sql: "..." }` (cross-source) |
| Per-row `expression:` and cross-source `expression:` (same property) | `transform:` (per-row) and `aggregate:` (cross-source); split by role |
| Free-string filters | Curated predicates (`equals`, `in`, `and`, ...) or `{ sql: "...", sparql: "..." }` escape hatch |
| Free-string transforms | Curated transforms (`cast`, `join` / `split`) or `{ sql, sparql }` escape hatch |
| `normalize: \"SQL expr\"` (free string) | Closed enum (`lowercase`, `uppercase`, `trim`, `truncate`, `pad`, `strip_non_digits`, `unicode_nfc`, `ascii_fold`, `round`, `clamp`, `truncate_time`, `null_as`) or `{ sql, sparql }` escape hatch |
| `default:` literal + separate `default_expression:` | Single `default: { value: ... }` or `default: { sql: ... }` |
| `array:` and `array_path:` (two properties) | Single `array:` accepting a path expression |
| `source_path:` (separate key for nested access) | Folded into `source:` — single key, parser decides flat vs path |
| Mapping-level `references:` | `lookup_source:` |
| Mapping-level `references:` | `lookup_source:` |
| Mapping-level `references_field:` | `lookup_field:` |
| `links[n].references:` | `links[n].lookup_source:` (parallel to mapping-level) |
| `cluster_members:` | `id_feedback:` |
| `cluster_field:` | `id_feedback_field:` |
| `_cluster_id` (test format token) | `_canonical_id` |
| `_base` (test format token) | Dropped from test format — runtime concern, not a backend-neutral assertion |
| `derive_noop:` | `suppress_unchanged_writes:` |
| `derive_timestamps:` | `track_field_timestamps:` |
| `derive_tombstones:` | `tombstone_field:` |
| Source `primary_key` doubling as identity | `primary_key` is change-detection only; identity is at target |
| `primary_key` (relational name) | `id_field` (planned rename, see [Sources](#sources) note) |
| `pg_runtime:` / `sparql_runtime:` blocks in mapping | Dropped — operational config (connection metadata, schema/view prefix, base IRI) lives outside the mapping schema (see [Sources](#sources)) |

## Linkage mappings

Some MDM scenarios produce a linkage table holding rows like
`(crm_id, erp_id)` where each column is a local ID in a different source
namespace and no shared field value can be written back to either source.
Regular identity matching cannot handle this — `crm_id` and `erp_id` are not
shared values, they are an *assertion* that two local IDs refer to the same
entity.

A **linkage mapping** addresses this. It contributes identity edges to the
union-find without contributing any field values to the target:

```yaml
mappings:
  - name: mdm_links
    source: mdm_links
    target: contact
    links:
      - source_field: crm_id
        lookup_source: crm_contacts    # this value is a CRM contact ID
      - source_field: erp_id
        lookup_source: erp_contacts    # this value is an ERP contact ID
```

One row in `mdm_links` produces one identity edge connecting the CRM record
and the ERP record. The union-find then propagates that connection
transitively across all sources.

A mapping with `links:` may have no `fields:` at all (linkage-only) or may
combine links with field mappings if the linkage table also carries useful
business data.

### Changes from v1

- `links:` entries now use `source_field` / `lookup_source` instead of
  `field` / `references` — making the direction explicit and parallel to the
  mapping-level `lookup_source:` on field mappings.
- `link_key:` is dropped. v1's `link_key` was an IVM-safety optimization
  that let a pre-computed cluster ID arrive with the source row atomically.
  This optimization is valid but belongs in renderer-specific sidecar files
  (see [Renderer-specific configuration](#renderer-specific-configuration)),
  not in the core schema.

### Write-back alternative

If the MDM system can write a shared `match_id` back to each source, a
linkage mapping is unnecessary — `match_id` becomes a regular field in the
target's `identity:` list and union-find handles it directly. The linkage
mapping exists for cases where write-back is not possible.

## See also

- [v2-migration-rfc.md](v2-migration-rfc.md) — clause-by-clause migration
- [v2-prototype-examples.md](v2-prototype-examples.md) — full example translations
- [triplestore-backend.md](triplestore-backend.md) — RDF/SPARQL renderer design
