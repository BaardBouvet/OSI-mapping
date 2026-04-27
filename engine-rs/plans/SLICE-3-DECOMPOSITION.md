# Slice 3 — JSON-LD framing & nested arrays — decomposition

**Status:** Foundation + 3a + 3b landed (real JSON-LD framing on the
SPARQL side via `render::framing`). Sub-slices 3c–3d pending.

Slice 3 of [SPARQL-IMPLEMENTATION-PLAN.md](SPARQL-IMPLEMENTATION-PLAN.md)
("real JSON-LD framing + nested arrays + `parent:` mappings") is too
large to land in one pass. This document breaks it into incremental
sub-slices that can each be implemented + tested + validated in
isolation.

## Foundation (done)

- Model surface: `Mapping.parent`, `Mapping.array`,
  `Mapping.parent_fields`, `FieldMap.references`. All `#[serde(default)]`,
  `deny_unknown_fields` preserved.
- Parser accepts these keys; v2 examples can use them.
- Both backends `bail!` with a clear "slice 3a not yet implemented" or
  "slice 4 not yet implemented" message when these are present.
- v2-shaped example at `examples/nested-arrays-v2/` demonstrates the
  intended shape; not yet wired into conformance.

## 3a — Array expansion, child-only round-trip (DONE)

Goal: a child mapping with `parent:` + `array:` produces independent
child entities that round-trip through their own
`<child_mapping>_{updates,inserts,deletes}` views/queries. The parent's
reverse view stays flat (no embedded nested array yet).

Deliverables:

- **Lift.** Pre-process input rows to expand `array:` columns into one
  logical row per element, with `parent_fields:` aliases injected.
- **PG forward view.** Use `LATERAL jsonb_array_elements(parent_table.<col>) WITH ORDINALITY`
  to materialise child rows; element fields read from the JSON object,
  parent fields read from the surrounding row.
- **SPARQL lift.** Each array element becomes a separate JSON-LD object
  with `@id` derived from `<parent_iri>/<array>/<index_or_pk>`.
  `parent_fields:` aliases land as ordinary source-prop predicates on
  the element subject.
- **Identity / forward / reverse.** Reuse slice-2 composite identity;
  child target's `<M>_reverse` is exactly the same machinery as a
  top-level mapping.

Out of scope for 3a: parent-level reverse aggregation, deep nesting
(`parent` of a child is itself a child), `references:`.

Conformance: a new `nested-arrays-shallow` example with one parent
source carrying `lines: jsonb`, asserting that `<child>_updates`/
`<child>_inserts`/`<child>_deletes` agree across PG and SPARQL.

## 3b — Parent reverse aggregation via JSON-LD framing (DONE)

Goal: a parent target's reverse projection embeds child entities as a
nested array, byte-equivalent across PG and SPARQL, scaling to deeper
nesting via a frame-driven mechanism.

Deliverables landed:

- **PG reverse.** Parent's `<M>_reverse` view `LEFT JOIN LATERAL`s
  against a per-canonical `jsonb_agg(jsonb_build_object(...) ORDER BY
  child_identity)` of the child's resolved view. The resulting JSONB
  column appears in `<M>_updates` / `<M>_inserts` / `<M>_deletes` and
  participates in the `IS DISTINCT FROM` round-trip diff.
- **SPARQL reverse.** Real frame-driven aggregation via the new
  [`render::framing`](../src/render/framing.rs) module. The SPARQL
  side issues a `CONSTRUCT` producing `(child rdf:type ChildTarget)`,
  the child's mapped properties, and a synthetic
  `osi:embedFor/<child_mapping>` predicate carrying the parent
  linkage value. A [`Frame`] with `@type = ChildTarget` and one
  [`FrameProp::Scalar`] per child field is applied via
  [`apply_frame_grouped_by`], grouped on the linkage scalar, sorted
  by child identity, and canonical-JSON-encoded. The output bytes
  match PG's `jsonb_agg`.
- **Conformance.** `examples/nested-arrays-shallow` asserts both
  mappings’ deltas are empty when input matches the canonical agg.

**On the framer.** The framer is a focused subset of [JSON-LD 1.1
framing](https://www.w3.org/TR/json-ld11-framing/): root-type match,
scalar property projection, embedded array projection, sort-key
ordering. It does *not* implement `@embed` modes beyond "once",
`@reverse`, `@nest`, `@included`, `@graph` inside frames, or frame
inheritance. The public API ([`Frame`], [`Triple`], [`apply_frame`])
is intentionally small to make swapping in the full `json-ld` crate
(0.21) painless when broader semantics are needed. Until then, the
current implementation handles every shape the v2 spec actually
defines.

## 3c — Deep nesting (PENDING)

Goal: a child mapping can itself have `parent:` pointing at another
child mapping. The lift chain is recursive; the reverse view embeds
multi-level nested arrays.

Deliverables:

- **Lift.** Pre-process recursively; `parent_fields:` aliases compose
  along the chain.
- **PG.** Recursive CTE-style materialisation of the lift chain (or
  nested `LATERAL` joins).
- **SPARQL.** JSON-LD framing handles arbitrary depth natively; the
  work is in the lift step computing element IRIs along the chain.
- **Conformance.** Reuses the v1 `nested-arrays-deep` example,
  re-shaped for v2.

## 3d — `array_path:` syntax

Goal: `array: data.lines[0].items` (multi-segment paths into nested
JSON) work uniformly with the `source: foo.bar` grammar.

Deliverables:

- **Path parser.** Single shared grammar for `array:`/`source:`
  expressions — bare segment, dotted, bracketed for keys with `.`/`[`.
- **PG.** Use `jsonb_path_query` for path-based access.
- **SPARQL.** Resolve paths client-side during lift (the path lives in
  the source data, not the RDF model).

## Open questions

1. **Element ordering.** The v2 spec lists three options: `sort:`,
   `order: true` (zero-padded ordinal), `order_prev:`/`order_next:`.
   Decide ordering per-sub-slice or fold into a single 3e?

2. **Reverse contract for inserts of nested children.** When a parent
   has no source row but children exist canonically, how does the
   reverse view distinguish "insert this parent and its children" from
   "insert these orphan children"?  Likely needs a parent-driven view.

3. **`scalar: true` arrays.** Bare-scalar arrays — the v2 spec keeps
   them but flags `multi_value` as preferred. Sequence vs. set
   semantics decided by the strategy on the target field, not by the
   array shape. Probably belongs with slice 5b (`multi_value`), not
   slice 3.

## Scope contract

This document is design only. No code beyond the foundation has been
written. Each sub-slice gets its own commit + conformance test.
