# Source Removal and Cluster Split Risk

## Purpose

This note documents what can happen when a source system (mapping) is removed
from a target and clusters were previously connected through that source via
transitive identity edges.

Goal: keep behavior explicit, avoid accidental duplicate inserts, and preserve
the engine's stateless architecture.

## Core Risk

If a removed source was the bridge between two remaining sources, connected
components can split.

Example:

```text
Before: CRM -- Billing -- ERP   (one cluster)
After removing Billing: CRM      ERP   (two clusters)
```

Result:
- A single previously-resolved entity can become multiple entities.
- Delta behavior can shift from update to insert in one or more mappings.
- ETL may create duplicate rows if no preservation strategy is applied.

## How Splits Happen by Edge Type

1. Identity-field edges
- Most vulnerable to silent transitive loss.
- If CRM linked to Billing by one field, and Billing linked to ERP by another,
	removing Billing removes the path.

2. Link edges (`links` without `link_key`)
- Same graph behavior: removing a mapping removes edges incident to that
	mapping's rows.
- Remaining direct links still apply.

3. Cluster-ID edges (`links` with `link_key`, `cluster_members`, `cluster_field`)
- More resilient if remaining rows already share a persisted `_cluster_id`
	value.
- If cluster propagation had occurred previously, components may remain joined
	even after bridge removal.

## Architectural Constraint

The engine is stateless by design. It should not introduce hidden persisted
cluster memory as part of core identity computation.

Implication:
- Any continuity across source-removal events must come from explicit data
	inputs (links, membership rows, cluster fields), not implicit engine state.

## Option Set

### Option 1: Accept split as canonical behavior

Definition:
- Source removal changes the identity graph.
- Connected components are recomputed from remaining evidence only.

Pros:
- Simple and principled.
- Fully aligned with stateless model.

Cons:
- High surprise potential for users.
- Higher duplicate-insert risk during decommissioning.

Best fit:
- Teams that prefer strict semantics over continuity.

### Option 2: Validation warning for transitive dependency risk

Definition:
- Add validator diagnostics warning that removing mapping `X` may split
	clusters for target `T`.

Pros:
- Low implementation complexity.
- Good safety signal with no runtime behavior change.

Cons:
- Informational only.
- Does not preserve clusters by itself.

Best fit:
- Baseline safeguard to ship regardless of chosen migration strategy.

### Option 3: Decommission mode (`active: false` style)

Definition:
- Keep mapping in identity graph, but disable business-field participation and
	delta output for that mapping.

Pros:
- Smooth migration path.
- Preserves transitive cluster structure while phasing out operational writes.

Cons:
- Source must still be queryable.
- Not true removal.
- Adds model complexity (new lifecycle mode).

Best fit:
- Planned source retirement where the old table remains temporarily available.

### Option 4: Pre-removal bridge migration

Definition:
- Before removing a source, generate explicit links between remaining sources
	that were only transitively connected through the removed source.

Example output artifact:
- A bridge table or view, then a linkage-only mapping with `links`.

Pros:
- True source removal while preserving intended connectivity.
- Explicit and auditable.
- Still stateless at runtime.

Cons:
- Requires migration step and operational discipline.
- Quality depends on bridge-generation logic.

Best fit:
- Production decommissioning where continuity is required.

### Option 5: Rely on existing feedback persistence

Definition:
- If `_cluster_id` feedback already exists (`cluster_members` or
	`cluster_field`), allow those persisted cluster-ID edges to preserve
	continuity after source removal.

Pros:
- Uses existing mechanism.
- No extra engine behavior needed.

Cons:
- Works only when feedback coverage is sufficient.
- Can be partial and non-obvious.

Best fit:
- Environments with mature ETL feedback loops already in place.

### Option 6: Engine-managed cluster snapshots

Definition:
- Engine stores persistent cluster membership snapshots and reuses them to
	preserve continuity when sources are removed.

Pros:
- Most automatic continuity.

Cons:
- Breaks stateless architecture.
- Adds substantial lifecycle and consistency complexity.

Best fit:
- Not recommended under current design principles.

## Recommendation

Recommended baseline:
- Option 2 (validator warnings) as default safety mechanism.

Recommended continuity path:
- Option 4 (pre-removal bridge migration) as the explicit decommission workflow.

Pragmatic support path:
- Option 5 (existing feedback persistence) where already available, documented
	as a resilience benefit but not guaranteed.

Avoid:
- Option 6, unless product direction intentionally changes from stateless to
	stateful engine operation.

## Suggested Decommission Workflow

1. Detect risk
- Run validation and identity graph diagnostics to identify mappings that act
	as transitive bridges.

2. Choose preservation strategy
- If continuity required, generate bridge links among remaining sources.
- If strict semantics desired, accept split and prepare ETL for inserts.

3. Stage and verify
- Run with both old source and new bridge links in a staging cycle.
- Confirm cluster count and delta inserts stay within expected bounds.

4. Remove source mapping
- Apply config change only after bridge evidence exists.

5. Post-change monitoring
- Track insert spikes and duplicate candidate rates for one or more cycles.

## Guardrails to Add to Validator (Proposed)

- Warn when removing mapping `M` increases connected-component count for target
	`T` in a representative snapshot.
- Warn when mappings for target `T` have insert-producing behavior but no
	continuity mechanism (`cluster_members`, `cluster_field`, or explicit links)
	after planned removal.
- Info note: existing `_cluster_id` feedback can preserve continuity if shared
	values remain present in surviving mappings.

## Product Positioning Statement

Removing a source can legitimately split clusters when that source carried the
only transitive path. This is expected in a stateless graph model. If continuity
is required, preserve it explicitly with persisted cluster evidence or bridge
links before removal.
