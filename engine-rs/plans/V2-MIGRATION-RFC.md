# v2 Migration RFC

**Status:** Draft

Companion to [v2-spec-draft.md](V2-SPEC-DRAFT.md). Itemizes every v1 schema
construct, what happens to it in v2, and what existing examples need to
change. Pre-1.0: no compatibility layer. v1 files do not load on v2.

For full worked examples see [v2-prototype-examples.md](V2-PROTOTYPE-EXAMPLES.md).

## Migration principle

If a v1 construct expresses a **load-bearing concept**, it survives in v2
under a possibly-different name. If it is a **stylistic alternative** to
another construct, the alternative wins and the loser is removed. If it is a
**leaky abstraction over PG runtime**, it moves to a backend-neutral form or
becomes a renderer-specific concept.

## Document structure

| v1 | v2 | Change |
|---|---|---|
| Single file required | Single file OR folder of `.yaml` | Additive |
| `version: "1.0"` | `version: "2.0"` | Required version bump |
| `sources` | `sources` | Same shape; `primary_key` no longer doubles as identity |
| `targets` | `targets` | New `identity:` block at target level |
| `mappings` | `mappings` | Field syntax restructured (transforms, expression keying) |
| `tests` | `tests` | Identical shape; new conformance contract across renderers |

## Identity

| v1 | v2 |
|---|---|
| `strategy: identity` on a field | `identity:` list on the target |
| `link_group: name` on multiple fields | AND-tuple in target `identity:` list |
| `links:` on a mapping | Kept with cleaned-up entry syntax (`source_field` / `references`) |
| `link_key:` on a mapping | Dropped — move to `pg_runtime:` / `sparql_runtime:` if needed |
| Source `primary_key` doubling as identity for matching | Decoupled — `primary_key` is change-detection only |

### Translation rules

1. **Single-field identity**:
   ```yaml
   # v1
   targets:
     contact:
       fields:
         email: identity
   # v2
   targets:
     contact:
       identity:
         - email
       fields:
         email: coalesce
   ```

2. **Composite identity via `link_group`**:
   ```yaml
   # v1
   targets:
     person:
       fields:
         first_name: { strategy: identity, link_group: name_dob }
         last_name:  { strategy: identity, link_group: name_dob }
         dob:        { strategy: identity, link_group: name_dob }
   # v2
   targets:
     person:
       identity:
         - [first_name, last_name, dob]
       fields:
         first_name: coalesce
         last_name: coalesce
         dob: coalesce
   ```

3. **Multiple identity strategies (OR semantics)**:
   ```yaml
   # v1
   targets:
     contact:
       fields:
         email: identity
         tax_id: identity
         first_name: { strategy: identity, link_group: name_dob }
         last_name:  { strategy: identity, link_group: name_dob }
         dob:        { strategy: identity, link_group: name_dob }
   # v2
   targets:
     contact:
       identity:
         - email
         - tax_id
         - [first_name, last_name, dob]
   ```

## Strategies

Per-field strategies that survive: `coalesce`, `last_modified`, `any_true`,
`expression`. Unchanged in semantics. New: `multi_value` (replaces `collect`).

`identity` as a strategy is **removed** — identity is now declared at the
target level. A field that was previously `identity`-only still appears in
`fields:` (it must, in order to be mapped from sources and written back to
them) and gets a real resolution strategy. `coalesce` is the natural default.

`collect` is **renamed** to `multi_value` to match the field-property naming
convention (`collect` was a verb in a list of nouns). The semantics are
unchanged: the resolved value is the deduplicated union of every value
contributed by every source for that field.

For structured elements (line items, addresses) the answer is unchanged from
v1: use a child target via `parent:` + `array:`. `multi_value` is for bare
scalars only.

## Expressions: split into transform / aggregate / filter

v1 used a single `expression:` property for both per-row transforms and
cross-source aggregations. v2 splits by role:

| Role | Property | When it runs | Produces |
|---|---|---|---|
| Per-row value transform | `transform:` | Before resolution, per source row | Value |
| Cross-source aggregation | `aggregate:` | During resolution, across all rows | Value (with `strategy: expression`) |
| Row filter | `filter:`/`reverse_filter:` | Per row, decides inclusion | Boolean |

All three use the **backend-keyed escape hatch**. `filter:` /
`reverse_filter:` additionally accept a small curated **predicate**
vocabulary (`equals`, `in`, `not_null`, `gt`, `and`, ...) because boolean
predicates appear in essentially every non-trivial mapping and are cleanly
portable across both backends. There is no curated value-transform
vocabulary; `transform:` and `aggregate:` are escape-hatch-only.

Migration of v1 expressions:

| v1 | v2 |
|---|---|
| Per-row `expression: "SQL"` | `transform: { sql: "..." }` |
| Cross-source `expression:` on `strategy: expression` field | `aggregate: { sql: "..." }` |
| `default_expression: "SQL"` | `default: { sql: "..." }` |
| `default: true` (bare literal) | `default: { value: true }` |
| `filter: "SQL WHERE clause"` | Curated predicate or `filter: { sql: "..." }` |
| `reverse_filter: "SQL"` | Curated predicate or `reverse_filter: { sql: "..." }` |
| `normalize: "regexp_replace(...)"` (top-level) | `normalize:` closed enum or `normalize: { sql, sparql }` (still a comparison adapter, not a transform) |
| `value_map:` (top-level on field) | Kept; specified in `value-map-rfc.md`, ships with v2. Field-level, mutually exclusive with `transform:`, fallback via `value_map_fallback`. |

Renderers read only their own key. A field with no key for your backend is
a compile-time error in that renderer.

The PG renderer accepts everything v1 did (just under a different syntax)
because the `sql:` escape hatch is always available.

The triplestore renderer requires a `sparql:` key wherever a per-row
transform or filter is needed. v1 examples that have only `expression:
"SQL"` will fail to compile under the SPARQL renderer until a `sparql:` key
is added.

## Two backends, fixed keys

v2 targets exactly two renderers: PG views and SPARQL/RDF. Inside
expression blocks (`transform:`, `aggregate:`, `default:`, `filter:`,
`reverse_filter:`), exactly two keys are valid:

- `sql:` — consumed by the PG renderer
- `sparql:` — consumed by the triplestore renderer

Any other key is a validation error. There is no top-level `renderers:`
declaration; the keys are fixed. Adding a third backend in a future spec
version will add a third key.

## ETL feedback (renamed)

The contracts are unchanged in semantics. The names are renamed to drop
engine-internal jargon ("cluster", "derive") in favour of self-explanatory
labels:

| v1 | v2 |
|---|---|
| `cluster_members` | `id_feedback` |
| `cluster_field` | `id_feedback_field` |
| `_cluster_id` (test format token) | `_canonical_id` |
| `derive_noop` | `suppress_unchanged_writes` |
| `derive_timestamps` | `track_field_timestamps` |
| `derive_tombstones` | `tombstone_field` |
| `written_state` | `written_state` (unchanged) |

In the triplestore backend these compile to named graphs; in PG they remain
tables. The schema-level contract is identical — the connector populates
these state stores after each cycle and the engine reads them on the next.

## References (renamed at mapping level and links)

The target-level `references:` keeps its name — it states "this field's
resolved value is a reference to entities of type X." Type information.

The mapping-level `references:` (and `links[n].references:`) is renamed
because it carried a *different* concept under the same name: "this source
value lives in source X's namespace; look it up there to translate IDs."

| v1 | v2 |
|---|---|
| Mapping field `references: crm_company` | `lookup_source: crm_company` |
| Mapping field `references_field: iso_code` | `lookup_field: iso_code` |
| `links[n].references: crm_contacts` | `links[n].lookup_source: crm_contacts` |

When `lookup_field:` is omitted, it defaults to the looked-up source's
`primary_key` — the natural choice for most FK relationships.

`references_field` was also misplaced on target fields in earlier v2 drafts;
it only appears on mapping-level field mappings.

## Nested data and ordering

`parent`, `parent_fields`, `sort`, `order`, `order_prev`, `order_next` are
unchanged in semantics. v2 adds full coverage in the spec doc (v1 documented
these only via examples).

**Changed:** v1's `array:` (single column) and `array_path:` (dotted path)
are collapsed into a single `array:` property accepting a path. A
single-segment path is just a path of length one.

**Scalar arrays kept** — `scalar: true` on a child mapping handles bare-string
arrays. For most cases `multi_value` on the parent is cleaner; reserve
scalar-array child mappings for when elements need their own write-back
path.

## Tests

Test format is unchanged in shape. Two changes:

- Token rename: `_cluster_id` → `_canonical_id`.
- `_base` is **dropped** from the test format. It was a runtime/engine
  concern (the original source snapshot used for noop detection) leaking
  into a backend-neutral assertion format. Tests now assert only on
  user-visible outcomes (`updates`/`inserts`/`deletes`).

Every renderer must reproduce the same outcomes from the same input. This
is the conformance contract.

Test runner gains a `--backend` flag:

```sh
osi test ./examples/hello-world --backend pg-views
osi test ./examples/hello-world --backend sparql
osi test ./examples/hello-world                       # runs against both backends
```

A test passing on PG but failing on SPARQL is a renderer bug or a feature
gap (e.g., the mapping uses an `aggregate:` with no `sparql:` key) — never
a test bug.

## Folder loading

```
# Both work; no schema difference between them
mapping.yaml
mapping/contact.yaml
mapping/company.yaml
mapping/tests/integration.yaml
```

Merge errors (duplicate source/target/mapping name across files) are reported
with file path and line number.

## Renderer-specific configuration (out of the spec)

v1 allowed renderer-specific blocks (`pg_runtime:`, `sparql_runtime:`)
inside the mapping file. v2 **removes these from the mapping file entirely**.
They move to renderer-specific sidecar files consumed by the corresponding
renderer.

Why: renderer config is deployment, not semantics. Mixing them couples the
portable mapping to a specific runtime, which contradicts the multi-backend
goal. The mapping file stays portable; sidecars carry deployment details.

The spec does not govern sidecar formats — each renderer defines its own.
v1 properties that moved to sidecars: `pg_runtime:`, `sparql_runtime:`,
`link_key:`. The v1 `passthrough:` property is **kept** in the core
schema (see the Passthrough section in the v2 spec) — there is no
connector layer to delegate unmapped-column merge to, so the mapping
must declare which non-canonical source columns to surface in the
renderer's delta output.

## Unknown keys

Unknown YAML keys are **errors by default** at every level of the mapping
file. This prevents typos (`fileds:`, `defualt:`, `lookup_souce:`) from
becoming silent no-ops. Renderers must validate against the schema and fail
loudly.

## What this RFC does not commit to

- **An exhaustive curated value-transform vocabulary.** v2 ships a small
  curated `transform:` set (`cast`, `join` / `split`) plus a field-level
  `value_map:` for enum/code translation (specified in
  [value-map-rfc.md](VALUE-MAP-RFC.md)). Everything else uses the
  backend-keyed escape hatch. `aggregate:` remains escape-hatch-only.
- **The specific RDF vocabularies used by the triplestore renderer.** That is
  a renderer choice, not a spec decision. See
  [triplestore-backend.md](TRIPLESTORE-BACKEND-DESIGN.md).

## Migration checklist for an existing example

1. Bump `version` to `"2.0"`.
2. For each target, lift `strategy: identity` and `link_group:` declarations
   into a target-level `identity:` list.
3. Replace each lifted field's `strategy: identity` with `coalesce` (or
   `last_modified` if appropriate) so the field still gets a write strategy.
4. Convert every string-shorthand strategy (`name: coalesce`) to object form
   (`name: { strategy: coalesce }`).
5. Rename `strategy: collect` → `strategy: multi_value` and add
   `element_type:` (required). Semantics unchanged. For structured
   elements that v1 already mapped via `parent:` + `array:`, no change.
6. Split v1 `expression:` by role:
   - Per-row transforms → `transform: { sql: "..." }`
   - Cross-source aggregations (with `strategy: expression`) →
     `aggregate: { sql: "..." }`
7. Move `default_expression:` content into `default: { sql: "..." }`.
   Convert literal `default:` values to `default: { value: ... }`.
8. Convert top-level `normalize: "SQL expr"` strings to either the closed
   normalize enum (`lowercase`, `uppercase`, `trim`, `strip_non_digits`,
   `unicode_nfc`) or `normalize: { sql, sparql }` if no enum value fits.
   `normalize:` stays a comparison adapter; do not move it into
   `transform:`. Keep any v1 `value_map:` declarations — v2 retains
   `value_map:` (see [value-map-rfc.md](VALUE-MAP-RFC.md)) with
   bijective auto-inversion and an explicit `value_map_fallback:` policy.
9. Convert filter/reverse_filter strings to either curated predicates
   or `{ sql: "..." }` (or the equivalent backend key) when no curated
   form fits. Predicate shape is uniform: unary `{ field }`, binary
   `{ field, value }`, list `{ field, values }`, boolean ops take a list
   of nested predicates (or a single predicate for `not`).
10. Replace `array_path:` with `array:` (path syntax now uniform). If a v1
    mapping used `source_path:` for nested-field access, fold the value
    into `source:` — v2 has a single `source:` key that accepts either a
    bare field name or a path expression (dot, `[N]`, `['key']`).
    Inspect any v1 AND-tuple identity groups: v2 explicitly skips a tuple
    for a record when **any** field in the tuple is null on that record.
    Mappings that relied on partial-null matches must use a separate OR-
    group instead.
11. Rename mapping-level `references:` → `lookup_source:`,
    `references_field:` → `lookup_field:`, and
    `links[n].references:` → `links[n].lookup_source:`.
12. Rename ETL feedback properties: `cluster_members` → `id_feedback`,
    `cluster_field` → `id_feedback_field`, `derive_noop` →
    `suppress_unchanged_writes`, `derive_timestamps` →
    `track_field_timestamps`, `derive_tombstones` → `tombstone_field`.
13. Rename test-format token `_cluster_id` → `_canonical_id`. Remove any
    `_base` references from test data — it’s no longer in the test format.
14. Move `pg_runtime:` / `sparql_runtime:` blocks out of the mapping file
    into renderer-specific sidecar files. Move `link_key:` similarly.
    `passthrough:` stays in the mapping schema (no connector layer to
    delegate it to) — just verify the v1 syntax (list of source columns,
    optional `as:` rename) is intact.
15. Decide whether to split into a folder. (Single file is still fine.)
16. Re-run tests against the new renderer. PG-renderer tests should pass
    unchanged once the syntax is migrated.

Worked examples in [v2-prototype-examples.md](V2-PROTOTYPE-EXAMPLES.md).
