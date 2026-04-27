# SPARQL backend — real implementation

**Status:** Slices 1–2 + 3a + 3b + 5a done (slice 3b uses real JSON-LD
framing via `render::framing`). Slices 3c–3d, 4, 5b/c, 6–8 pending.

The current `render::sparql` module is a stub: it uses Oxigraph as an
in-memory KV store and does identity clustering, forward resolution, and
delta computation in **Rust**. It produces no SPARQL artifact and does
not match the [triplestore backend design](TRIPLESTORE-BACKEND-DESIGN.md).
This plan replaces it with a real SPARQL/RDF pipeline.

The conformance contract is unchanged: every example must produce
identical `updates`/`inserts`/`deletes` in PG and SPARQL.

---

## What the stub gets wrong

| Design intent | Stub does |
|---|---|
| Lift inputs via JSON-LD `@context` | Rust loop over `Row`s calling `store.insert(Quad)` |
| Identity closure via SPARQL `INSERT WHERE` over `owl:sameAs` / `osi:canonical` | Union-find in a Rust `HashMap` |
| Resolution via SPARQL aggregation (`ORDER BY ?priority LIMIT 1`) per field | Rust `Vec::sort_by_key` over SELECT results |
| Reverse projection via JSON-LD framing | Per-field `SELECT ... LIMIT 1` assembled in Rust |
| `render_sparql()` returns an inspectable artifact (set of SPARQL UPDATE strings) | Returns `SparqlPlan { doc: doc.clone() }`; CLI prints `{plan:#?}` |
| `<base>/identity/<T>`, `<base>/written/<M>`, `<base>/cluster/<M>` graphs | Not present |
| RDF-star provenance | Not present |

The stub passes hello-world only because the conformance harness compares
deltas and hello-world is one mapping with one coalesce field.

---

## Slice 1 — real RDF pipeline for `hello-world` (and only hello-world)

Goal: prove the architecture end-to-end on the simplest example. No new
features beyond what hello-world exercises (single-field identity,
coalesce strategy, no nesting, no references, no written-state).

### Deliverables

1. **`SparqlPlan` carries SPARQL strings, not just a `Doc` clone.**
   ```rust
   pub struct SparqlPlan {
       pub base_iri: String,
       pub jsonld_contexts: IndexMap<String, JsonValue>,    // per source mapping
       pub lift_updates: IndexMap<String, String>,          // mapping → SPARQL UPDATE
       pub identity_closure: IndexMap<String, String>,      // target → SPARQL UPDATE
       pub forward_updates: IndexMap<String, Vec<String>>,  // target → per-field UPDATE
       pub reverse_frames: IndexMap<String, JsonValue>,     // mapping → JSON-LD frame
   }
   ```
   `cargo run render <yaml> --backend sparql` prints these as a single
   readable artifact (analogous to the PG DDL output): the contexts,
   the SPARQL UPDATE strings in execution order, and the frames.

2. **JSON-LD lift.** Each mapping gets a JSON-LD `@context` mapping its
   source field names to `<base>/sourceprop/<mapping>/<field>` predicates,
   with the source PK becoming `@id` (=  `<base>/source/<mapping>/{pk}`).
   The executor:
   - serialises input rows to a JSON-LD document using the context,
   - parses the JSON-LD into RDF quads in `<base>/sourcegraph/<mapping>`,
   - never touches `store.insert(Quad)` directly for input data.

   Crate: `oxrdfio` + `oxjsonld` (already in Oxigraph workspace), or
   `sophia_jsonld`. Pick whichever is simplest behind a thin adapter.

3. **Identity closure as SPARQL UPDATE.** For each target with single-field
   identity:
   ```sparql
   INSERT { GRAPH <base/identity/contact> { ?row osi:canonical ?cid } }
   WHERE  { GRAPH ?g { ?row <base/sourceprop/.../email> ?v } . ... }
   ```
   The canonical IRI is computed inside SPARQL via `IRI(CONCAT(...))` over
   `SHA256(?v)` (Oxigraph supports `SHA256` and `IRI()`). Equivalence
   classes use `owl:sameAs` triples plus a property-path closure step:
   ```sparql
   INSERT { GRAPH <base/identity/contact> { ?a owl:sameAs ?b } }
   WHERE  { ?a osi:identityValue ?v . ?b osi:identityValue ?v . }
   ```
   Slice 1 keeps the closure step trivial (single-hop) since hello-world
   doesn't need transitive merging across more than two sources sharing
   one value. Multi-hop closure lands in slice 2.

4. **Forward resolution as SPARQL UPDATE.** Per target field, one
   `INSERT { GRAPH <canonical/T> { ?cid prop:f ?val } } WHERE { ... }`
   query that picks the winner via `ORDER BY ?priority ?decl_order LIMIT 1`
   inside a sub-SELECT. No Rust-side sorting.

5. **Reverse projection via SPARQL CONSTRUCT.** Per mapping, two
   `CONSTRUCT` queries:
   - existing source rows (joined to canonical via `osi:canonical`),
     subject = source-row IRI,
   - insert candidates (canonicals with no source row in this mapping),
     subject = canonical IRI.

   Resulting triples are grouped per subject in Rust to produce flat
   `Row`s. This is *not* JSON-LD framing — `oxjsonld 0.1` declares the
   framing profile but does not implement it, and pulling in the
   `json-ld` crate to frame flat objects would be over-engineering for
   slice 1. Real framing lands in slice 3 when nested arrays (`parent:`,
   `array_path:`) genuinely require `@container: @list` / `@embed`
   semantics.

6. **Delta classification stays in the harness for now.** The framed
   reverse output is a list of source-shaped rows; the harness diffs
   against the input as today. The framed output replaces the
   ad-hoc `build_reverse_row` SELECT loop.

7. **CLI output.** `osi-engine render <yaml> --backend sparql` emits a
   single text artifact:
   ```
   # Base IRI: https://osi.test/

   # JSON-LD contexts
   ## mapping crm
   { ... }

   # SPARQL UPDATE: lift crm
   INSERT DATA { GRAPH ... }   # or LOAD <jsonld:...> if we go that route

   # SPARQL UPDATE: identity closure for contact
   INSERT { ... } WHERE { ... }

   # SPARQL UPDATE: forward contact.name
   INSERT { ... } WHERE { ... ORDER BY ?priority LIMIT 1 }

   # JSON-LD frame: reverse crm
   { ... }
   ```
   That artifact is what a user could in principle run against any
   compliant triplestore. (Slice 1 doesn't promise external-store
   compatibility — Oxigraph is the reference target — but the artifact
   should be readable.)

### Out of scope for slice 1

- Composite / AND-tuple identity (`[[a, b]]`)
- Strategies other than `coalesce`
- `references:` / FK reverse resolution
- Nested arrays / `parent:` / `array_path:`
- `written_state` / `derive_noop` (`<base>/written/<M>` graph)
- `cluster_members` feedback (`<base>/cluster/<M>` graph)
- RDF-star per-triple provenance
- `derive_tombstones`
- Transitive (multi-hop) identity closure across >2 sources

These all have their own slices below.

---

## Subsequent slices

| Slice | Adds | Examples it should unlock |
|---|---|---|
| 2 | Composite (AND-tuple) identity — canonical IRI from `SHA256(STR(v0) || \u001f || STR(v1) || …)` inside SPARQL; PG identified/reverse views accept tuple identity. **Done.** | `composite-identity` |
| 3 | Pull in `json-ld` crate; replace Rust grouper with real JSON-LD framing; nested arrays + `parent:` mappings; `@container: @list` with `sort:` | `nested-arrays`, `nested-array-path` |
| 4 | `references:` reverse resolution via `<identity/T>` lookup | `references`, `relationship-mapping` |
| 5 | Resolution strategies beyond coalesce (`last_modified`, `multi_value`, `any_true`, `all_true`) | `crdt-ordering`, `merge-threeway`, `multi-value` |
| 6 | `written_state` named graph + noop detection | `derive-noop`, `merge-internal` |
| 7 | RDF-star provenance triples on `<canonical/T>` | (no new examples; enables provenance queries) |
| 8 | `derive_tombstones`, soft delete, hard delete | `soft-delete*`, `hard-delete`, `propagated-delete` |

Each slice extends the conformance test set; no slice regresses an
already-passing example.

---

## Open questions

1. **JSON-LD framing crate.** *Resolved (slice 1):* `oxjsonld 0.1`
   declares the framing profile but does not implement it. Slice 1
   uses CONSTRUCT plus a Rust-side per-subject grouper for reverse
   projection. Slice 3 pulls in the `json-ld` crate (0.21) for real
   framing once nested-array shapes demand `@container: @list`
   semantics.

2. **`SHA256` digest length.** Slice 1 uses the *full* 64-char
   SHA-256 hex digest inside SPARQL (`SHA256(STR(?val))`) for the
   canonical IRI suffix — no truncation. The PG renderer can match
   this in slice 5 when we revisit cross-renderer canonical-IRI
   stability.

3. **`base_iri` configuration.** Hard-coded to `https://osi.test/` today.
   Per the design doc this is operational config, not in the mapping
   file. Slice 1 keeps the hard-coded value; deployment-time config
   lands later.

4. **Closing the CLI loop.** Should `render --backend sparql` produce
   an artifact that's directly executable against an external
   triplestore, or is the artifact informational and the harness
   keeps its own executor? Slice 1 assumes the latter (artifact is
   informational, executor lives in the SPARQL module). Revisit at
   slice 5 once we have enough constructs to be worth shipping.

---

## Exit criteria for slice 1

*All met as of the slice-1 commit.*

- `cargo test --test conformance -- hello_world` passes against a
  rebuilt SPARQL backend that:
  - lifts via JSON-LD,
  - runs identity closure as a SPARQL UPDATE,
  - runs forward resolution as a SPARQL UPDATE per field,
  - reverses via SPARQL `CONSTRUCT` + Rust-side per-subject grouping
    (real framing deferred to slice 3).
- `cargo run render examples/hello-world/mapping.yaml --backend sparql`
  emits a multi-section text artifact (contexts, UPDATEs, frames),
  not a Rust `Debug` dump.
- `cargo fmt --check`, `cargo clippy --tests -- -D warnings`,
  `cargo test` all green.
- The old union-find / per-field SELECT code is **deleted**, not left
  behind as a fallback.
