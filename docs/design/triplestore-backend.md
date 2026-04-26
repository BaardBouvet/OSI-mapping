# Triplestore Backend Design

**Status:** Draft

Renderer design for compiling [v2 mappings](v2-spec-draft.md) to an
RDF triplestore that supports SPARQL Update, named graphs, and RDF-star.
Companion to the existing PostgreSQL view renderer; both compile from the
same backend-neutral schema and must satisfy the same test conformance
suite.

## Hard requirements on the target triplestore

- **Named graphs** (RDF 1.1) — for source isolation, written-state tracking,
  and cluster feedback
- **RDF-star** (RDF 1.2 / RDF\*) — for per-triple resolution provenance
  (which source contributed each value, when, with which strategy)
- **SPARQL 1.1 Update** with named-graph clauses (`INSERT`, `DELETE`,
  `INSERT WHERE`, `DELETE WHERE`)
- **SPARQL property paths** (`+`, `*`) — for transitive identity closure
- **Built-in functions** for the curated transform vocabulary (string,
  numeric, regex)

Triplestores known to satisfy these as of writing: GraphDB 10+, Stardog,
Oxigraph, RDF4J 5+, Apache Jena (Fuseki) with RDF-star plugin. AllegroGraph
satisfies the SPARQL/named-graph requirements but uses its own statement
attributes for per-triple metadata in place of standard RDF-star.

The renderer compiles to standards-conformant SPARQL/RDF and treats
triplestore-specific tuning (indexes, isolation levels, federation) as
out-of-scope concerns handled by the renderer's operational config
(connection details, base IRI, etc.), not by blocks inside the mapping file.

## Internal model

### Named graph layout

| Graph IRI | Holds |
|---|---|
| `<base>/source/<mapping_name>` | Raw lifted triples from source `<mapping_name>` — exactly what the source said, untouched. The `_base` equivalent. |
| `<base>/canonical/<target_name>` | Resolved triples for target `<target_name>`. The "golden record" graph. |
| `<base>/written/<mapping_name>` | What the renderer last wrote back to source `<mapping_name>`. Backs `derive_noop` / `written_state`. |
| `<base>/cluster/<mapping_name>` | ETL feedback (`cluster_members` / `cluster_field`) — generated source IDs paired with canonical IRIs. |
| `<base>/identity/<target_name>` | Identity equivalence triples (`owl:sameAs`-flavored, see below). Materialized closure of union-find. |

`<base>` is the renderer's base IRI, configured per deployment in the
renderer's operational config (required; no default). Example: with a
base IRI of `https://data.example.com/`, the canonical contact IRI
becomes `https://data.example.com/canonical/contact/abc123`. The base
IRI must end with `/` or `#`. It never appears in the mapping schema.
Mapping authors writing `sparql:` expressions reference targets and
properties via reserved curies (`canonical:`, `prop:`, `source:`) which
the renderer expands at compile time — see the v2 spec section on
operational configuration.

### Identity in RDF terms

Each contributing source row gets a stable IRI:
`<base>/source/<mapping_name>/<urlencoded(source_pk)>`. These are *not* the
canonical entity IRIs — they are per-source-record IRIs, analogous to the
v1 cluster member rows.

Each canonical entity gets an IRI:
`<base>/canonical/<target_name>/<sha256(identity_value)>`.

The mapping between source-row IRIs and canonical IRIs lives in
`<base>/identity/<target_name>` as triples of the form:

```turtle
<base/source/crm/1>    osi:canonical <base/canonical/contact/abc123> .
<base/source/erp/100>  osi:canonical <base/canonical/contact/abc123> .
```

These triples are computed by SPARQL Update from the identity equivalence
edges:

```sparql
# Pseudocode for the closure step
INSERT { GRAPH <identity/contact> { ?row osi:canonical ?canonical } }
WHERE  {
  # All rows that share an identity value (single-field OR group)
  GRAPH ?g { ?row osi:identityValue/osi:field "email" ;
                  osi:identityValue/osi:value ?v . }
  GRAPH ?g2 { ?other osi:identityValue/osi:field "email" ;
                     osi:identityValue/osi:value ?v . }
  # Compute closure: canonical IRI is sha256 of the smallest IRI in the component
  ...
}
```

Real implementation uses iterated property-path expansion, not the naive form
above. For triplestores with native SAMEAS or owl:sameAs reasoning, that
machinery handles the closure for free.

### Per-triple provenance via RDF-star

Every triple in `<base>/canonical/<target>` carries metadata:

```turtle
<<<canonical/contact/abc> schema:name "Alice">>
    osi:source     <source/crm/1> ;
    osi:strategy   "coalesce" ;
    osi:priority   1 ;
    osi:timestamp  "2026-01-15T10:00:00Z"^^xsd:dateTime .
```

This is the RDF analogue of the PG resolution view's per-field provenance
columns. It enables:

- `coalesce` resolution — pick triple with lowest `osi:priority`
- `last_modified` resolution — pick triple with highest `osi:timestamp`
- `any_true` / `all_true` — emit a single canonical boolean triple per
  three-valued OR / AND of contributing source values
- `multi_value` — emit one canonical triple per distinct contributed value
  (deduplicated bag; v1 `collect` semantics)
- Lineage queries — "which source produced this value?"
- Reverse mapping — write back to `<source/crm/1>`'s named graph using the
  per-triple `osi:source` reference

Without RDF-star this would require either reified statements (4× triple
blowup, awkward queries) or named graph per source-mapping per target field
(combinatorial explosion). RDF-star is therefore a hard requirement, not a
preference.

## Compilation pipeline

For each renderer pass, the SPARQL output is conceptually a sequence of
`INSERT WHERE` / `DELETE WHERE` updates that the engine submits to the
triplestore. There are no in-engine joins or in-process resolution — all
work happens in SPARQL.

### Forward pass: source → canonical

For each mapping `M` with source `S` and target `T`:

1. **Lift**: convert source rows to triples in `<source/M>`.
   - One subject IRI per source row (using PK)
   - Each `field:` mapping becomes a triple `<row> <fieldIRI> "value"`
   - Curated transforms compile to SPARQL function expressions in the
     `INSERT { ... } WHERE { ... }` template
   - `expression: { sparql: "..." }` blocks paste in directly
2. **Identity edges**: for each target identity rule (single field or
   AND-tuple), emit identity-value triples that the closure step consumes.
3. **Closure**: rebuild `<identity/T>` by transitive closure over identity
   values across all sources of `T`.
4. **Resolve**: rebuild `<canonical/T>` from the union of source graphs,
   applying per-field strategies via SPARQL aggregation:
   - `coalesce` — `ORDER BY ?priority ASC, ?timestamp DESC` then `LIMIT 1`
   - `last_modified` — `ORDER BY ?timestamp DESC` then `LIMIT 1`
   - `multi_value` — emit one triple per distinct contributed value
     (deduplicated)
   - `any_true` — logical-OR over `BOUND(?v) && ?v = true`
   - `all_true` — logical-AND over `BOUND(?v) && ?v = true` across
     contributing sources only
   - `strategy: expression` with `aggregate: { sparql: ... }` — aggregate
     via the user expression

Each resolved triple is wrapped with RDF-star provenance.

### Reverse pass: canonical → source

For each mapping `M`, project `<canonical/T>` back through the field mapping
to produce the triples that *should* exist in `<source/M>`. Diff against the
actual `<source/M>` (or `<written/M>` if `derive_noop` is enabled) to
produce the `updates`/`inserts`/`deletes` lists.

This is where JSON-LD framing enters the renderer:

- The target's nested shape (from `parent`/`array_path`/`parent_fields`) is
  compiled to a JSON-LD frame
- The frame is applied to the canonical graph to produce nested JSON
- The nested JSON is shaped to match the source PK conventions and emitted
  as the test-format records

JSON-LD framing is a renderer implementation detail — it does not appear in
the schema. The schema author writes `parent`/`array`/etc. and the SPARQL
renderer compiles those to a frame; the PG renderer compiles the same
constructs to `jsonb_agg` window functions. Both produce the same
`expected:` shape.

### Test execution

To satisfy the cross-backend conformance contract, the SPARQL renderer must
produce identical `updates`/`inserts`/`deletes` records to the PG renderer.
Steps for a test:

1. Load `input:` records into per-source named graphs (lift).
2. Run forward pass.
3. Run reverse pass against each source mapping.
4. Project the diff back to source-PK-shaped JSON records via JSON-LD
   framing + source-shape post-processing.
5. Compare to `expected:` block.

The default test harness wraps **Oxigraph in-memory** — a zero-dependency,
in-process Rust triplestore that loads, queries, and tears down per test
without external services. The same test corpus runs against the SQL
renderer (Postgres) and the SPARQL renderer (Oxigraph); divergence is a
renderer bug or a documented gap (e.g. a mapping that uses
`transform: { sql: ... }` only and skips the SPARQL side).

Operators wanting to validate against a different store (RDF4J,
Fuseki, GraphDB) can swap the harness backend; Oxigraph is the
reference implementation, not a hard dependency of the spec.

## Schema-to-renderer translation table

| Schema construct | RDF/SPARQL realization |
|---|---|
| `targets.<T>.identity: [field]` | Single-field identity-value triples in `<source/M>`; closure in `<identity/T>` |
| `targets.<T>.identity: [[a, b, c]]` | Tuple identity-value triples (concatenated SHA of values); closure as above |
| `field: coalesce` | SPARQL `ORDER BY ?priority, ?timestamp DESC LIMIT 1` aggregation |
| `field: last_modified` | SPARQL `ORDER BY ?timestamp DESC LIMIT 1` aggregation |
| `field: multi_value` | One triple per distinct contributed value; deduplicated bag |
| `field: any_true` | Logical-or aggregation |
| `field: all_true` | Logical-and aggregation over contributing sources |
| `field: { strategy: expression, aggregate: { sparql: ... } }` | Expression pasted into aggregation `SELECT` |
| `references: T2` | Object position becomes IRI; reverse pass uses `<canonical/T2>`'s `osi:canonical` link to find source-local ID |
| `references_field: name` | Reverse pass dereferences the named property of the referenced canonical entity instead of the IRI |
| `parent` (embedded) | Frame entry: child fields appear flat under parent |
| `parent` + `array_path` | Frame entry: `@container: @list` (with `sort:`) or `@container: @set` |
| `default` | `INSERT { ?row ?p ?val } WHERE { FILTER NOT EXISTS { ?row ?p ?_ } }` |
| `value_map` | SPARQL `VALUES` clause inline |
| `normalize: <enum>` | SPARQL function chain (e.g., `REPLACE(?v, "[^0-9]", "")`) |
| `cluster_members` / `cluster_field` | `<cluster/M>` named graph holding `(canonical_iri, source_pk)` pairs |
| `written_state` | `<written/M>` named graph; reverse pass diffs against this when `derive_noop: true` |
| `derive_tombstones` | DELETE triggered when canonical is absent from current `<source/M>` snapshot |
| `filter: { equals: ... }` / `{ sparql: ... }` | SPARQL `FILTER` clause in lift WHERE |

## Provenance examples

### Example: which source set contact name?

```sparql
SELECT ?source ?value ?ts WHERE {
  GRAPH <canonical/contact> {
    <<<canonical/contact/abc> schema:name ?value>>
        osi:source ?source ;
        osi:timestamp ?ts .
  }
}
```

Answers "what value was chosen, from which source-row IRI, with what
timestamp." The source-row IRI dereferences (via `<identity/contact>`) to
the contributing system and that system's local ID.

### Example: explain a coalesce decision

```sparql
SELECT ?source ?value ?priority ?ts WHERE {
  GRAPH <canonical/contact> {
    <<<canonical/contact/abc> schema:name ?value>>
        osi:source ?source ;
        osi:strategy "coalesce" ;
        osi:priority ?priority ;
        osi:timestamp ?ts .
  }
} ORDER BY ?priority DESC ?ts ASC
```

Returns the winning value plus the losing alternatives, in priority order.
The PG resolution view exposes the same information via per-column source
attribution; the SPARQL renderer surfaces it via RDF-star quotation.

## Association entities are reified, not predicate-folded

A common observation when reading a mapping like
[examples/relationship-mapping](../../examples/relationship-mapping/) is that
the `company_person_association` target *looks* like a relational artifact —
in pure RDF it could be flattened into per-relation predicates:

```turtle
# Tempting but NOT what the renderer does
<person/50> ex:primaryContactOf <company/100> .
<person/50> ex:billingContactOf <company/100> .
```

The triplestore renderer **does not** do this. Association targets are
**reified**: each association row becomes its own canonical entity with its
own IRI, identified by the target's `identity:` tuple, with the related
entities referenced via predicates.

```turtle
<base/canonical/company_person_association/sha(person_id,relation_type)>
    a osi:Entity ;
    ex:company       <base/canonical/company/abc> ;
    ex:person        <base/canonical/person/xyz> ;
    ex:relation_type "primary_contact" .
```

### Why reify

1. **Once an association carries any metadata** (`assigned_at`, `assigned_by`,
   `expires_on`, `notes`), predicate-folding collapses — RDF predicates can't
   carry attributes without reification anyway. Designing for the
   pure-tag-only case is YAGNI; designing for the metadata-bearing case
   handles both.

2. **One mental model across backends.** PG renderer materializes the
   association as a table, SPARQL renderer materializes it as a reified
   entity. Both are first-class. The operator does not learn "this case
   becomes predicates, that case becomes an entity, the rule is..." — every
   target is a target, in every backend.

3. **Reification is idiomatic RDF for this case.** Published vocabularies
   (schema.org's `Role`, PROV's `qualifiedAssociation`) use reification
   precisely because pure predicate-only modeling cannot carry assertion
   metadata. The renderer follows the same convention.

4. **Round-trip stability.** Reified entities project cleanly back to
   source-PK-shaped records via the JSON-LD frame, so the `tests:`
   conformance contract holds without backend-specific normalization.

### What this means for schema authors

Treat associations as full target entities with their own `identity:` tuple
and `fields:`. The triplestore renderer reifies them. There is no separate
"multi-valued reference with qualifier" type in the schema, and no need for
one — the existing target/identity/fields machinery already covers it.

## What stays out of scope

- **Inference.** No OWL reasoning, no SHACL validation, no rule engines on
  top. The renderer produces a self-consistent triple set; downstream
  consumers can add reasoning if they want.
- **Property name choices.** The `osi:` vocabulary is renderer-private. The
  schema author writes domain-meaningful field names; the renderer chooses
  IRIs (default `<base>/prop/<field>`). Binding a field to a custom
  external IRI like `schema:email` is a possible post-v2 extension via
  schema annotations, not via blocks in the mapping file today.
- **Federation.** Triples can come from federated sources via SPARQL
  `SERVICE`, but the renderer treats those as opaque source datasets — the
  schema's `sources:` block is the boundary of authority.
- **Full-text search.** Triplestore vendors have proprietary FTS extensions;
  use them via `transform: { sparql: ... }` or `aggregate: { sparql: ... }`
  if needed. Not a curated primitive.

## Open implementation questions

1. **Closure algorithm** — naive iterated `INSERT WHERE` works but is O(n²)
   on changesets. Worth exploring incremental algorithms or vendor-specific
   `owl:sameAs` reasoning for production deployments.

2. **RDF-star write performance** — quoted triples are heavyweight in some
   stores. For very high write rates the canonical graph might use plain
   triples and lineage might be projected from a separate `<provenance/T>`
   graph. Defer this until benchmarked.

3. **Test runner CI cost** — running full integration tests against a real
   triplestore in CI adds time. Oxigraph's in-process Rust binding is the
   default test harness (chosen above); alternative stores can be
   opted into via `--triplestore` flag.

4. **Cross-engine determinism** — when two sources have the same
   `(priority, timestamp)`, the tiebreaker order needs to be deterministic
   *across renderers*, not just within one. The PG view uses a stable
   `ORDER BY` on source name; the SPARQL renderer must replicate that
   ordering. Worth adding to the conformance test suite.
