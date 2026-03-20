# Reserved delta columns and source-name collisions

**Status:** Proposed

When a source table contains columns like `_base` (or `_action`, `_cluster_id`),
the current delta output contract can collide with user data columns. This plan
defines how to support these names without ambiguity.

## Problem

The delta view currently emits engine metadata columns with fixed names:

- `_action`
- `_cluster_id`
- `_base`

Mappings can also project source columns and passthrough columns into the same
delta view. If user data includes one of the reserved names, SQL output can
become ambiguous or impossible to consume safely.

Example:

- Source has a real business column named `_base`
- Delta also emits metadata `_base` (JSONB snapshot)
- Consumer cannot distinguish business `_base` from metadata `_base`

## Goals

- Allow source/business columns to use any valid identifier, including reserved
  names currently used by delta metadata.
- Preserve a clear, stable metadata contract for ETL consumers.
- Keep generated SQL deterministic and easy to reason about.

## Non-goals

- Backward compatibility shims for pre-1.0 output names.
- Making every internal plumbing column configurable.

## Options considered

### Option A: Keep current names and reject collisions

Validation fails if user-projected columns contain reserved names.

Pros:

- Simple implementation.

Cons:

- Blocks legitimate source schemas.
- Pushes schema renaming burden to users.

### Option B: Auto-rename colliding user columns

Engine rewrites colliding user columns (for example `_base` -> `_base_src`).

Pros:

- Preserves current metadata names.

Cons:

- Surprising output contract.
- Harder for ETL codegen and test expectations.

### Option C: Move engine metadata into a reserved namespace

Rename consumer-facing metadata columns to a dedicated engine namespace:

- `_action` -> `__osi_action`
- `_cluster_id` -> `__osi_cluster_id`
- `_base` -> `__osi_base`

User columns keep their original names unchanged.

Pros:

- Solves all current and future collisions in one model.
- Contract is explicit: `__osi_*` is engine metadata.
- Aligns with pre-1.0 freedom to rename.

Cons:

- Requires docs/example/test updates.
- ETL consumers must switch to new names.

### Option D: Nested metadata object

Expose one metadata JSONB column (for example `__osi_meta`) containing
`action`, `cluster_id`, and `base`.

Pros:

- No scalar name collisions.

Cons:

- Less ergonomic for SQL-based ETL consumers.
- Makes filtering by action more verbose.

## Recommendation

Adopt **Option C**.

Pre-1.0 makes this the cleanest long-term contract: namespaced metadata columns
and unconstrained user column names.

## Proposed contract

Delta output metadata columns:

- `__osi_action` (text)
- `__osi_cluster_id` (text)
- `__osi_base` (jsonb)

Internal view plumbing can keep existing names where useful; only
consumer-facing aliases need to change.

## Implementation plan

1. Render layer

- Update delta outer SELECT aliases in `render/delta.rs` to use `__osi_*`.
- Keep internal CASE/join references untouched where possible.

2. Validation

- Add a reserved metadata namespace check:
  user-projected columns beginning with `__osi_` are rejected.
- Remove/relax checks that effectively reserve `_base`/`_action`/`_cluster_id`
  as user names.

3. Docs and examples

- Update reference docs describing delta output contract.
- Update examples/tests that assert delta rows.

4. Optional follow-up

- Add `output.columns` aliasing (from OUTPUT-CONTRACT-PLAN) for teams that want
  custom metadata names beyond `__osi_*`.

## Migration (pre-1.0)

- Single breaking change release note:
  delta metadata columns renamed to `__osi_*`.
- No compatibility alias views in engine.

## Acceptance criteria

- A mapping with source or passthrough column named `_base` validates.
- Generated delta view contains both:
  - business `_base` (if mapped/projection requires it)
  - metadata `__osi_base`
- No ambiguous/duplicate output column names.
- All integration tests pass with updated expected contracts.
