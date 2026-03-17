# Learning guide

**Status:** Planned

Structured educational content that teaches the mapping concepts
progressively, from first principles to advanced patterns.

## Problem

Today's docs are **reference material** — complete but not pedagogical.
A newcomer encounters `schema-reference.md` (a flat property index),
`annotated-example.md` (one walkthrough), and 42 example directories
with no suggested order. There is no path from "what is this?" to
"I can design a mapping for my use case."

The concepts stack: you need identity resolution before you can understand
references, references before nested arrays make sense, and nested arrays
before depth-mismatch patterns click. Without a guided learning path, users
must discover this dependency order themselves.

## Goals

1. **Progressive disclosure.** Each chapter builds on the previous one.
   A reader who follows the order can design real mappings by the end.
2. **Concept-first, not property-first.** Explain *why* before *how*.
   What problem does identity resolution solve? Why does coalesce need
   priority? Why do references exist?
3. **Runnable at every step.** Each chapter references one or more example
   directories the reader can run locally.
4. **Engine-agnostic.** Like the rest of `docs/`, the guide describes the
   mapping language, not the Rust implementation.

## Proposed structure

```
docs/
  learning-guide/
    README.md           ← overview and reading order
    01-first-mapping.md
    02-identity-resolution.md
    03-merge-strategies.md
    04-references.md
    05-nested-arrays.md
    06-expressions.md
    07-advanced-patterns.md
```

### Chapter outline

#### 01 — Your first mapping

- What a mapping file is and what it produces
- `version`, `sources`, `targets`, `mappings`
- Identity and coalesce — the two strategies you always need
- Run the `hello-world` example end to end
- **Referenced examples:** hello-world

#### 02 — Identity resolution

- The entity linking problem: two systems, same real-world entity
- How identity fields create a match graph
- Transitive closure: if A=B and B=C then A=B=C
- Compound identity (`link_group`)
- What happens when identities conflict
- **Referenced examples:** composite-keys, merge-generated-ids

#### 03 — Merge strategies

- Coalesce with priority — picking the best value
- Last-modified — timestamp-based conflict resolution
- Atomic groups — all-or-nothing field sets
- Expression — custom aggregation (max, string\_agg)
- Bool\_or — flag propagation
- **Referenced examples:** merge-threeway, merge-groups, value-groups,
  custom-resolution, merge-partials

#### 04 — References and foreign keys

- The cross-system FK problem
- How `references:` on targets declares entity FKs
- How `references:` on field mappings tells the reverse view which namespace
- Reference preservation after merge
- `references_field` for vocabulary/lookup tables
- **Referenced examples:** references, reference-preservation,
  vocabulary-standard, vocabulary-custom

#### 05 — Nested arrays

- JSONB source data with embedded arrays
- `parent:` + `array:` syntax
- `parent_fields:` — how children reference parent values
- Multi-level nesting and qualified `parent_fields`
- **Referenced examples:** nested-arrays, nested-arrays-deep,
  nested-arrays-multiple, json-fields

#### 06 — Expressions and filters

- Forward expressions (transform on the way in)
- Reverse expressions (reconstruct on the way out)
- `direction: forward_only` / `reverse_only`
- Filters and reverse\_filter
- Default values and default\_expression
- What the expression safety validator allows and rejects
- **Referenced examples:** value-conversions, value-derived,
  value-defaults, route, propagated-delete

#### 07 — Advanced patterns

- Hierarchy merge (extra ancestor level)
- Depth mismatch (missing intermediate level)
- Embedded vs referenced relationships
- Multi-value cardinality mismatch
- Inserts and deletes
- When to use which pattern
- **Referenced examples:** hierarchy-merge, depth-mismatch,
  embedded-vs-many-to-many, multi-value, inserts-and-deletes

## Approach

- Write in second person ("you define…", "your mapping…")
- Each chapter: 1-page concept explanation → YAML snippet → "try it" link
  to the example directory → key takeaway box
- Diagrams where they help (entity graphs, pipeline stages)
- Cross-link to schema-reference.md for full property details
- No engine internals (no `_fwd_`, `_resolved_`, SQL generation)

## Implementation

1. Create `docs/learning-guide/` directory
2. Write chapters 01–07
3. Update `docs/README.md` to link the learning guide prominently
4. Review: ensure every example referenced actually exists and passes
