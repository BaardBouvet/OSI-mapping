# Forward Views — Separate vs Inlined CTEs

## Status: Done (Option A implemented)

## Problem

The view-consolidation refactor inlined forward query bodies as CTEs inside
the identity view. This reduces the view count but hurts two important
scenarios:

1. **System rollout** — when a new source (e.g. hubspot) is being added, the
   team wants to `SELECT * FROM _fwd_hubspot LIMIT 100` to verify the mapping
   independently *before* it flows into identity resolution. With CTEs this
   requires manually extracting and running the subquery.

2. **Debugging** — when a resolved record looks wrong, the first question is
   "what did each source contribute?". Separate forward views let you inspect
   `_fwd_crm`, `_fwd_erp` side-by-side, which is much faster than reading
   through nested CTEs.

## Options

### A. Bring back separate forward views (always)

Re-emit `CREATE OR REPLACE VIEW _fwd_{mapping}` for every mapping, and have
the identity view `SELECT * FROM _fwd_crm UNION ALL SELECT * FROM _fwd_erp`.

- Pro: simple, debuggable, each view is independently queryable
- Pro: zero cost in practice — Postgres planning is cheap for views
- Con: more views in `pg_views`

### B. CLI flag `--inline-forward`

Default to separate views, but offer a flag to inline them as CTEs for
deployment scenarios where view count matters.

- Pro: best of both worlds
- Con: two code paths to maintain and test

### C. Keep CTEs (current)

Forward bodies are inlined as CTEs in the identity view. No separate views.

- Pro: minimal view count, tidy `pg_views`
- Con: not debuggable, rollout friction

## Recommendation

**Option A**: bring back separate forward views unconditionally.

View count is not a real concern — Postgres handles hundreds of views with no
overhead. The debuggability and rollout benefit outweigh the cosmetic
tidiness of fewer views.

The identity view already has all the CTE plumbing; the change is to emit
`CREATE VIEW _fwd_X` alongside and rewrite the identity CTE as
`SELECT * FROM _fwd_X` instead of inlining the body.

## Implementation

1. `render_forward_body` → `render_forward_view` (restore CREATE VIEW wrapper)
2. Identity view: `_id_base AS (SELECT * FROM _fwd_crm UNION ALL ...)` — reference the views instead of inlining bodies
3. DAG: re-add `ViewNode::Forward` between Source and Identity
4. Tests: update dump view arrays to include `_fwd_*`

## Open Questions

- Should embedded mappings that share a parent source also get their own
  forward view, or stay inlined?
