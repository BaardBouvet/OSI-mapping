# Engine Documentation

Technical documentation for the OSI mapping reference engine.

## Contents

| Document | Description |
|----------|-------------|
| [View Pipeline](view-pipeline.md) | Stage-by-stage description of the generated PostgreSQL view funnel |
| [Design Decisions](design-decisions.md) | Rationale for key architectural choices: diamond avoidance, delta classification, `_base` JSONB, noop detection, cluster identity |

## Quick Reference

The engine compiles a YAML mapping document into a DAG of PostgreSQL views:

```
source table ──► _fwd_{mapping} ──► _id_{target} ──► _resolved_{target} ─┬─► {target}          (analytics, always)
                                                                          └─► _rev_{mapping} ──► _delta_{mapping}  (opt-in)
```

The analytics path is diamond-free and IVM-safe. The reverse path has one controlled diamond (reverse LEFT JOINs identity) which is safe for ordered refresh.

## See Also

- [Engine README](../README.md) — Quick start and CLI usage
- [Mapping Schema](../../spec/mapping-schema.json) — The spec this engine implements
- [Examples](../../examples/) — Mapping files used as test cases
- [Top-level docs](../../docs/) — Schema reference, design rationale, annotated examples
