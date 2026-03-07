# ADR 0002: Remove `source_columns` from mapping spec

- Status: Accepted
- Date: 2026-03-07

## Context

Each `field_mapping` included an optional `source_columns` array listing the source columns consumed by `source_expression`. The intent was to support lineage tracking and validation without parsing SQL.

## Problem

`source_columns` is redundant with the expression it describes:

- The columns referenced are already visible in `source_expression`.
- The list can drift when expressions are edited but `source_columns` is not updated.
- It adds manual effort per field mapping with no effect on mapping execution.

This mirrors the rationale in ADR 0001 (removal of `transform_type`): metadata that duplicates executable content should be inferred by tooling, not hand-maintained.

## Decision

Remove `source_columns` from:

1. `specs/osi-mapping-schema.json`
2. All example mapping files.

Column lineage is derived from expression parsing by tooling when needed.

## Consequences

### Positive

- Leaner field mappings — only `target_field`, `source_expression`, and optional `reverse_expression` are needed.
- No stale lineage metadata.
- Simpler authoring.

### Negative

- Lineage tooling must parse expressions to extract column references.
- Parsing is dialect-dependent and may require dialect-aware tokenization.

## Alternatives considered

### Keep `source_columns` as required

Rejected — too much manual overhead and drift risk for the minimal spec.

### Keep `source_columns` as tooling-generated (not hand-authored)

Viable as a future extension. Tooling could emit `source_columns` into a resolved/compiled output without requiring it in the source file.

## Follow-up

If lineage tracking becomes a first-class requirement, consider a tooling command that computes and optionally injects `source_columns` into mapping files (similar to a lockfile pattern).
