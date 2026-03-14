# Engine Documentation

Technical documentation for the OSI mapping reference engine.

## Contents

| Document | Description |
|----------|-------------|
| [View Pipeline](view-pipeline.md) | Stage-by-stage description of the generated PostgreSQL view funnel (forward → identity → resolution → reverse → delta) |
| [Design Decisions](design-decisions.md) | Rationale for key architectural choices: no diamond dependencies, delta classification, `_base` JSONB, noop detection, cluster identity |

## Quick Reference

The engine compiles a YAML mapping document into a DAG of PostgreSQL views:

```
source table ──► _fwd_{mapping} ──► _id_{target} ──► _resolved_{target} ──► _rev_{mapping} ──► _delta_{mapping}
```

Each stage has exactly one upstream dependency — no diamonds, no cross-layer joins (except the reverse view's LEFT JOIN back to identity, which is transitively safe).

## See Also

- [Engine README](../README.md) — Quick start and CLI usage
- [Mapping Schema](../../spec/mapping-schema.json) — The spec this engine implements
- [Examples](../../examples/) — Mapping files used as test cases
- [Top-level docs](../../docs/) — Schema reference, design rationale, annotated examples
