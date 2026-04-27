# SPARQL CONSTRUCT artifact pipeline

**Status:** Done

The SPARQL backend emits **CONSTRUCT-only** artifacts: every derived named
graph is defined by one or more CONSTRUCT queries, never by an imperative
UPDATE / DELETE script. The same artifacts run inside the in-process
Oxigraph executor (for tests) and on a deployed incrementally-maintained
triplestore (RDFox, Stardog reasoning, Jena TDB with rules, …) — there is
exactly one rendering, one execution model.

This document captures the full shape of that pipeline as it exists in
[engine-rs/src/render/sparql.rs](../src/render/sparql.rs) after the
CONSTRUCT-only refactor, including the named-graph IRI scheme, the per-stage
CONSTRUCT templates, the artifact file layout, and the in-process executor
that simulates an incremental triplestore for testing.

## Design principles

1. **CONSTRUCT only, no UPDATE.** Every derived graph is the materialised
   result of CONSTRUCT queries that depend on lower graphs. Retraction is
   the deployed engine's job; we never emit `DELETE`.
2. **One artifact per named graph.** Each artifact file's first line is a
   `# Maintains: GRAPH <IRI>` annotation that tells the deployer (or the
   in-process executor) which graph the rule populates.
3. **Same code path for testing and deployment.** The in-process executor
   runs each CONSTRUCT and inserts the result triples into the declared
   named graph using `Store::insert(QuadRef::new(...))`. The conformance
   test suite exercises the same rules a deployment would register.
4. **LIFT is the only imperative step.** Source rows are JSON-LD-expanded
   into `sourcegraph/<mapping>` by the client (or the executor's `lift`
   helper). After that, all downstream graphs follow automatically — in
   tests by serial execution of CONSTRUCTs, in production by the
   triplestore's IVM.
5. **Pre-1.0, no compatibility shims.** The previous UPDATE-based path,
   `ingest.sparql`, `--incremental` CLI flag, and `update_to_construct`
   helper are all gone — not deprecated.

## Named-graph IRI scheme

Base: configurable, default `https://osi.test/`.  Set via
`render_sparql_with_base(doc, base)` from Rust or `--base-iri <URL>` from
the CLI.  Must end with `/`.  Internally the base lives in a thread-local
that is set for the duration of one render call by an RAII `BaseGuard`,
so concurrent renders on different threads do not collide.

The `SparqlPlan.base_iri` field records what was used for that plan, so
callers can re-derive any IRI without re-parsing.

| Graph                              | Holds                                                    | Built by                |
| ---                                | ---                                                      | ---                     |
| `sourcegraph/<M>`                  | Lifted source rows for mapping `M`                       | Client LIFT (JSON-LD)   |
| `identity/<T>`                     | `?row osi:canonical ?cid` for every row → target `T`     | `identity_<T>.sparql`   |
| `canonical/<T>`                    | Resolved field values: `?cid <prop/f> ?val`              | `canonical_<T>.sparql`  |
| `lists/<parent_target>`            | `osi:hasChild/<arr>` head edges + `rdf:first/rdf:rest` chain | `lists_<parent>.sparql` |
| `reverse/<M>`                      | Canonical state projected back into source-row shape     | `reverse_<M>.sparql`    |

There is no `frame/<…>` graph — framing CONSTRUCTs are run on demand and
piped straight into the JSON-LD framer.

## Pipeline stages

```
LIFT            client / executor      → sourcegraph/<M>
IDENTITY        identity_<T>.sparql    → identity/<T>
FORWARD         canonical_<T>.sparql   → canonical/<T>          (one CONSTRUCT per field)
LIST            lists_<parent>.sparql  → lists/<parent_target>  (cells + heads, two CONSTRUCTs)
REVERSE         reverse_<M>.sparql     → reverse/<M>            (existing rows + insert candidates)
FRAMING         framing_<C>.sparql     → on-demand RDF graph    (then frame_<C>.jsonld → JSON)
```

### LIFT — JSON-LD expansion

For each mapping, the client (or `SparqlPlan::lift`) takes source rows,
attaches the per-mapping JSON-LD `@context` (file `context_<M>.jsonld`),
and inserts the expanded triples into `sourcegraph/<M>`. Subject IRIs are
derived from the source primary key when one exists, otherwise blank-node
synthesis.

The context maps each `source` field name to
`<base>/sourceprop/<M>/<field>` so source-side property IRIs are scoped
per mapping and never collide across mappings.

### IDENTITY — single CONSTRUCT per target

```sparql
# Maintains: GRAPH <https://osi.test/identity/order_line>
CONSTRUCT { ?row <https://osi.test/vocab/canonical> ?cid }
WHERE {
  { GRAPH <https://osi.test/sourcegraph/shop_lines> {
      ?row <…/order_ref> ?val0 . ?row <…/line_number> ?val1 } }
  BIND(IRI(CONCAT("https://osi.test/canonical/order_line/",
                  SHA256(CONCAT(STR(?val0), "\u001F", STR(?val1))))) AS ?cid)
}
```

- One UNION branch per source mapping that targets `T`.
- Composite identity: `\u001F` (Unit Separator) joins parts inside SHA256 to
  prevent ambiguous concatenations.
- The `osi:canonical` predicate is `<base>/vocab/canonical`.

### FORWARD — one CONSTRUCT per field, coalesce or last-modified

Coalesce strategy (`build_forward_coalesce`): for each target field, emit
a CONSTRUCT that selects `(?cid, ?val)` from the source whose
`(priority, decl_order)` is highest (lowest `prio` wins; lower `decl` wins
on tie), using `FILTER NOT EXISTS` against a self-join of candidate
`(mapping_graph, source_pred, prio, decl)` quadruples.

The candidate set lives in a `VALUES` clause — priorities and declaration
order are baked into the rendered SPARQL at compile time, so the engine
never has to look anything up at runtime.

Last-modified strategy (`build_forward_last_modified`): same shape, but
the tiebreaker compares a per-mapping `last_modified` literal pulled from
the row itself rather than a static priority.

### LIST — two CONSTRUCTs per child mapping

For an embedded child mapping `C` whose parent target is `P`:

1. **Cells CONSTRUCT** — emits `?cell rdf:first ?c` and
   `?cell rdf:rest ?next` for each child, with `?cell` derived from the
   parent IRI plus a `cell/<array>/<rank>` suffix. The rank is computed
   by a `COUNT(?c2)` subquery that counts children with a strictly
   smaller `(order_field, child_iri)` lexicographic key.
2. **Heads CONSTRUCT** — emits `?p osi:hasChild/<array> ?head` for each
   parent that has at least one child, where `?head` is the
   rank-0 cell IRI.

Both CONSTRUCTs are stored in a single `String` separated by `\n\n`. The
executor splits on the blank line and runs each part separately so
Oxigraph's parser sees one query at a time. The deployed triplestore
registers them as two independent rules, both maintaining
`lists/<parent_target>`.

Ordering rule: `lexicographic by child identity, IRI as tiebreaker`. The
ordering lives entirely inside the rule — no Rust sort.

### REVERSE — two CONSTRUCTs per mapping

Reverse is a projection of canonical state back into source-row shape so
the delta-diff step can compare against the input rows.

1. **Existing source rows CONSTRUCT** — for every `?row` in
   `sourcegraph/<M>`, look up its `?cid` in `identity/<T>` and emit each
   field as `?row <sourceprop/<M>/<f>> ?val` from `canonical/<T>` (each
   field wrapped in `OPTIONAL` so partial rows survive).
2. **Insert-candidate CONSTRUCT** — for every `?cid` that has *no* source
   row in `M` but exists in `identity/<T>`, emit the canonical fields
   keyed by `?cid` itself. These become INSERT deltas.

Both go into `reverse/<M>`. The DELETE deltas are derived by the executor
diffing `sourcegraph/<M>` against `reverse/<M>` for missing subjects.

### FRAMING — on-demand CONSTRUCT + JSON-LD frame

For every embedded child mapping there is:

- `framing_<C>.sparql` — a CONSTRUCT that walks the parent's canonical
  graph plus the `lists/<P>` chain to materialise an RDF graph shaped
  like `parent { fields…, hasChild/lines: (line₀ line₁ … lineₙ) }`.
- `frame_<C>.jsonld` — a JSON-LD frame document that, applied to that
  RDF graph, yields the framed JSON tree the consumer wants.

These run on demand (per query), not on every LIFT.

## Artifact file layout

`cargo run -- render <mapping.yaml> -b sparql --out-dir <dir>` writes:

```text
<dir>/
  context_<M>.jsonld           ← JSON-LD @context for LIFT
  identity_<T>.sparql          ← CONSTRUCT → identity/<T>
  canonical_<T>.sparql         ← N CONSTRUCTs → canonical/<T> (one per field)
  lists_<P>.sparql             ← 2 CONSTRUCTs → lists/<P>     (cells + heads)
  reverse_<M>.sparql           ← 2 CONSTRUCTs → reverse/<M>   (existing + inserts)
  framing_<C>.sparql           ← on-demand CONSTRUCT
  frame_<C>.jsonld             ← JSON-LD frame doc
```

Multi-CONSTRUCT files separate queries with one blank line. Each query
begins with a `# Maintains: GRAPH <IRI>` comment so the deployer / executor
can route results without parsing the SPARQL.

There is no combined `ingest.sparql` and no `--incremental` flag —
CONSTRUCT-form is the only output mode.

## In-process executor

`SparqlPlan::execute(inputs) -> Deltas` simulates the deployed pipeline:

1. `lift(store, inputs)` — JSON-LD-expand source rows into `sourcegraph/<M>`.
2. For each `(target, construct)` in `identity_constructs`, call
   `run_construct_into_graph(store, construct, identity_graph_iri(target))`.
3. For each `(field_key, construct)` in `forward_constructs`, route to
   `canonical_graph_iri(target)` (extracted from the `<target>.<field>` key).
4. `materialise_child_lists(store)` — for each child mapping, split the
   list_construct on `\n\n` and run each part into `lists/<parent_target>`.
5. `compute_deltas(store, inputs)` — run the reverse CONSTRUCTs, frame
   embedded children where applicable, diff against `inputs`, and emit the
   `Deltas { updates, inserts, deletes }` triple.

`run_construct_into_graph(store, construct, graph_iri)`:

- strips `# `-prefixed comment lines (so the `# Maintains:` annotation
  doesn't break Oxigraph's parser),
- runs `store.query(stripped)` and expects `QueryResults::Graph`,
- inserts each result triple via
  `store.insert(QuadRef::new(s, p, o, NamedNodeRef(graph_iri)))`.

The same construct strings are written to disk for deployment and run in
memory for tests — there is one rendering, one truth.

## `SparqlPlan` shape

```rust
pub struct SparqlPlan {
    pub base_iri: String,
    pub contexts:            IndexMap<String, serde_json::Value>, // per mapping
    pub identity_constructs: IndexMap<String, String>,            // per target
    pub forward_constructs:  IndexMap<String, String>,            // per "<target>.<field>"
    pub list_constructs:     IndexMap<String, String>,            // per child mapping
    pub reverse_existing:    IndexMap<String, String>,            // per mapping
    pub reverse_inserts:     IndexMap<String, String>,            // per mapping
    pub frame_constructs:    IndexMap<String, String>,            // per child mapping
    pub frame_documents:     IndexMap<String, serde_json::Value>, // per child mapping
    doc: Doc,
}
```

`Display` renders the same artifacts as a single annotated text dump,
grouped per mapping, in the execution order
`IDENTITY → FORWARD → LIST → REVERSE → FRAMING`. This is what
`cargo run -- render … -b sparql` prints when no `--out-dir` is given.

## CLI

```sh
cargo run -- render <mapping.yaml> -b sparql                      # text dump to stdout
cargo run -- render <mapping.yaml> -b sparql -o plan.txt          # text dump to file
cargo run -- render <mapping.yaml> -b sparql --out-dir <d>        # one CONSTRUCT file per graph
cargo run -- render <mapping.yaml> -b sparql --base-iri https://example.org/osi/  # override base
```

There is no `--incremental` switch — `--out-dir` always produces
CONSTRUCT artifacts.

## Testing

`engine-rs/tests/conformance.rs` runs four end-to-end scenarios:
`hello_world`, `composite_identity`, `last_modified`, `nested_arrays_shallow`.
Each loads a mapping, renders the plan, executes it via Oxigraph, and
asserts the resulting `Deltas` match the example's `expected:` block.

Because the in-process executor uses the same CONSTRUCTs that ship as
artifacts, passing tests are direct evidence that the deployed rules are
correct (modulo the deployment engine's IVM correctness).

## What this supersedes

This plan replaces the UPDATE-based slices originally described in
[SPARQL-IMPLEMENTATION-PLAN.md](SPARQL-IMPLEMENTATION-PLAN.md). The
identity-closure UPDATE, forward-resolution UPDATE, reverse
materialisation UPDATE, ingest-script chaining, and `--incremental` CLI
flag are all removed. Everything is CONSTRUCT.
