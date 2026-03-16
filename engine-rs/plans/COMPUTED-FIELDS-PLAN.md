# Computed target fields

**Status:** Design

Analysis and design for target fields whose values are computed from other
targets — cross-target aggregation (`employee_count` on department) and
recursive self-traversal (employee hierarchy path).

## Problem

Today every target field resolves in isolation: the resolution view aggregates
contributions from forward views for that single target and nothing else. Two
common use cases need values derived from **other resolved targets**:

1. **Cross-target aggregation** — a `department` target wants an
   `employee_count` field showing how many resolved employees reference it.
2. **Recursive self-traversal** — an `employee` target wants a
   `hierarchy_path` field like `CEO / VP Sales / Regional Manager / Alice`
   built by walking the `manager` self-reference chain.

Neither is expressible today. Expression strings are validated as column-level
SQL snippets and cannot contain subqueries or reference other views.

## Existing related work

- **EXPRESSION-SAFETY-PLAN Phase 3** describes `lookup:` on field mappings
  for cross-target access in **reverse views only** (mapping-level, not
  target-level). It generates correlated subqueries against `_resolved_`.
- **References** create DAG dependencies (`_resolved_A` depends on `_id_B`)
  but only affect reverse FK translation — resolution views never join other
  targets.
- **Identity views** use `WITH RECURSIVE` for connected-component discovery,
  so the engine already emits recursive CTEs.

## Use case 1 — Cross-target aggregation

### Scenario

```yaml
targets:
  department:
    fields:
      name: { strategy: identity }
      location: { strategy: coalesce }
      employee_count: ???

  employee:
    fields:
      email: { strategy: identity }
      name: { strategy: coalesce }
      department:
        strategy: coalesce
        references: department
```

Desired: `employee_count` = number of resolved employees whose `department`
field points to this department entity.

### Where does the computation live?

| Layer | SQL | Pros | Cons |
|-------|-----|------|------|
| Resolution view | Subquery in aggregation | Single view | Resolution becomes cross-target; DAG changes; circular risk |
| Analytics view | `LEFT JOIN LATERAL (SELECT count(*) ...)` | Clean separation; no pipeline impact | New view type; analytics currently trivial |
| Separate post-resolution view | Dedicated `_computed_{target}` view | Explicit DAG node; clear dependency | More views; consumers need to know which view to query |

**Recommendation: analytics view.** The analytics view is already the
consumer-facing layer. Adding computed columns there keeps the resolution
pipeline pure (per-target only) and avoids cross-target contamination of the
core aggregation logic.

### Proposed YAML

```yaml
targets:
  department:
    fields:
      name: { strategy: identity }
      location: { strategy: coalesce }
    computed:
      employee_count:
        aggregate: count
        from: employee
        match:
          department: _cluster_id    # employee.department = department._cluster_id
```

`computed:` lives alongside `fields:` but is NOT part of resolution. It
produces columns in the **analytics view only**.

### Generated SQL

```sql
-- Analytics view for department (with computed fields)
CREATE OR REPLACE VIEW "department" AS
SELECT
  r._entity_id AS _cluster_id,
  r."name",
  r."location",
  COALESCE(c_employee_count.val, 0) AS "employee_count"
FROM "_resolved_department" r
LEFT JOIN LATERAL (
  SELECT count(*) AS val
  FROM "employee" e
  WHERE e."department" = r._entity_id
) c_employee_count ON true;
```

Note: references the `employee` **analytics view** (which itself references
`_resolved_employee`). This creates a DAG dependency:
`analytics(department)` depends on `analytics(employee)`.

### DAG impact

```
_resolved_department ─────────────────────┐
                                          ↓
_resolved_employee → employee (analytics) → department (analytics)
```

Today analytics views have no inter-dependencies. Computed fields add edges
between analytics views. The existing topological sort handles this — it
already supports cross-target edges.

Circular `computed:` references (department counts employees, employee counts
departments) would be detected as a cycle in topological sort and rejected
at compile time.

## Use case 2 — Recursive self-traversal

### Scenario

```yaml
targets:
  employee:
    fields:
      email: { strategy: identity }
      name: { strategy: coalesce }
      manager:
        strategy: coalesce
        references: employee       # self-reference

    computed:
      hierarchy_path:
        traverse:
          follow: manager          # FK field to walk
          collect: name            # field to collect at each level
          separator: " / "
          direction: root_first    # CEO / VP / Manager / Self
          max_depth: 10
```

### Generated SQL

```sql
CREATE OR REPLACE VIEW "employee" AS
WITH RECURSIVE hierarchy AS (
  SELECT
    _entity_id,
    "name",
    "manager",
    "name"::text AS path,
    1 AS depth
  FROM "_resolved_employee"
  WHERE "manager" IS NULL           -- roots (no manager)

  UNION ALL

  SELECT
    e._entity_id,
    e."name",
    e."manager",
    h.path || ' / ' || e."name",
    h.depth + 1
  FROM "_resolved_employee" e
  JOIN hierarchy h ON e."manager" = h._entity_id
  WHERE h.depth < 10
)
SELECT
  r._entity_id AS _cluster_id,
  r."email",
  r."name",
  r."manager",
  COALESCE(h.path, r."name") AS "hierarchy_path"
FROM "_resolved_employee" r
LEFT JOIN hierarchy h ON h._entity_id = r._entity_id;
```

### Complexity

Recursive self-traversal is significantly more complex than cross-target
aggregation:

- Requires `WITH RECURSIVE` CTE in the analytics view
- Must detect and handle cycles (self-referencing employees)
- `max_depth` needed as a safety valve
- Direction control (`root_first` vs `leaf_first`)
- Multiple traversal patterns (path, depth, subtree count, roll-up sum)

## Proposed `computed:` properties

### Aggregation (cross-target)

```yaml
computed:
  field_name:
    aggregate: count | sum | min | max | array_agg | bool_or
    from: target_name            # source target
    match:                       # join conditions
      remote_field: local_field  # remote.field = local.field
    filter: "optional SQL predicate on remote target"
    field: remote_field_name     # for sum/min/max — which field to aggregate
```

### Traversal (recursive self-reference)

```yaml
computed:
  field_name:
    traverse:
      follow: fk_field           # FK field to walk
      collect: field_name        # what to collect at each step
      separator: " / "           # for string concatenation
      aggregate: string | array | count | sum
      direction: root_first | leaf_first
      max_depth: 10
```

## Implementation estimate

### Phase 1 — Cross-target aggregation (~80 lines)

- **Model:** `ComputedField` struct with `aggregate`, `from`, `match`, `filter`, `field`
- **Parser:** parse `computed:` section on targets
- **Validator:** check `from` target exists, `match` fields exist on both sides
- **DAG:** add edges between analytics views based on `computed.from`
- **Analytics renderer:** emit `LEFT JOIN LATERAL (SELECT ...)` for each computed field

### Phase 2 — Recursive traversal (~120 lines, separate effort)

- **Model:** `TraverseSpec` struct
- **Validator:** check `follow` field has `references: self_target`, reject cycles
- **Analytics renderer:** emit `WITH RECURSIVE` CTE
- Phase 2 requires phase 1 infrastructure

## Interaction with existing plans

- **EXPRESSION-SAFETY-PLAN Phase 3 (`lookup:`)** — `lookup:` operates on
  **reverse views** (per-mapping, for sync back to sources). `computed:`
  operates on **analytics views** (per-target, for consumer output). They
  are complementary, not overlapping.
- **ANALYTICS-PROVENANCE-PLAN** — provenance views sit alongside analytics.
  Computed fields add to the analytics view; provenance stays separate.

## Alternatives considered

### A — Put computed fields in resolution

Rejected. Resolution views aggregate forward contributions per target.
Injecting cross-target subqueries violates this contract, complicates the
DAG, and risks circular dependencies between resolution views
(resolution-A needs resolution-B needs resolution-A).

### B — Use `lookup:` from EXPRESSION-SAFETY-PLAN

`lookup:` is designed for **mapping-level** field overrides in reverse views:
a specific source gets a derived value from another target during sync.
Target-level computed fields are **consumer-facing aggregations** that appear
in every view consumer, not tied to any source mapping. Different scope,
different layer.

### C — External SQL views (do nothing)

Always viable. The analytics view is already a SQL view — consumers
can create their own view on top. This plan formalizes common patterns
(count, sum, hierarchy path) so the mapping YAML is self-describing and the
engine manages the DAG ordering automatically.

## Recommendation

Start with **Phase 1 (cross-target aggregation)** — `count` and `sum` cover
the most common use cases and the implementation is straightforward. Defer
Phase 2 (recursive traversal) until there's a concrete real-world mapping
that needs it. Document "create your own SQL view" as the escape hatch for
anything the declarative `computed:` doesn't cover.
