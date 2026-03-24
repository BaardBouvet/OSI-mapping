# Enriched expressions

**Status:** Done

Let users write full SQL scalar expressions — including correlated
subqueries — against other targets' resolved views. The user writes bare
target names; the engine rewrites them to internal view names, places the
expression in the enriched layer, and validates that references stay within
declared targets.

No DSL, no dot-path parsing, no join inference. The user writes SQL; the
engine provides the sandbox.

---

## Motivation

Cross-target aggregation (sum of child quantities, max of child dates) is
a real need, but it's niche. Building a DSL for it — `from:` + `match:`,
dot-path syntax, join inference algorithms — adds complexity
disproportionate to the use case.

The engine already generates stable, queryable views. If we let the user
reference other targets by name and rewrite those to the internal views,
users can write the SQL themselves without coupling to engine internals:

```yaml
targets:
  line_item:
    fields:
      line_item_id: { strategy: identity }
      order: { strategy: coalesce, references: order }
      qty: { strategy: coalesce, type: numeric }
      unit_price: { strategy: coalesce, type: numeric }
      warehouse_loc: { strategy: coalesce }

      total_shipped:
        strategy: expression
        expression: |
          COALESCE((
            SELECT sum(s.qty)
            FROM shipment s
            WHERE s.line_item = line_item._entity_id
          ), 0)
        type: numeric

      last_ship_date:
        strategy: expression
        expression: |
          SELECT max(s.ship_date)
          FROM shipment s
          WHERE s.line_item = line_item._entity_id

      fully_shipped:
        strategy: expression
        expression: |
          COALESCE((
            SELECT sum(s.qty)
            FROM shipment s
            WHERE s.line_item = line_item._entity_id
          ), 0) >= qty
```

No new properties. No new YAML syntax. Just SQL in `expression:`, now
permitted to include subqueries referencing other targets by name.

---

## Target name rewriting

The engine rewrites bare target names to internal view names. The user
never writes `_resolved_` — that's an engine implementation detail.

### In `FROM` / `JOIN` clauses

Target names after `FROM` and `JOIN` keywords are rewritten to quoted
resolved view names:

```
FROM shipment s        →  FROM "_resolved_shipment" s
JOIN line_item li      →  JOIN "_resolved_line_item" li
```

### Self-reference via target name

The enriched view aliases the outer query as the current target name.
The user references the local row with `{target_name}.{field}`:

```sql
-- User writes:
SELECT sum(s.qty) FROM shipment s WHERE s.line_item = line_item._entity_id
-- Engine renders:
SELECT sum(s.qty) FROM "_resolved_shipment" s WHERE s.line_item = line_item._entity_id
```

No `_self` keyword needed. The target name is the alias. Since target
names follow `^[a-z][a-z0-9_]*$`, there's no collision with SQL
keywords or user-chosen subquery aliases (users can avoid the conflict
by aliasing their subquery tables differently).

### Column contract

Every resolved view exposes:

| Column | Type | Description |
|--------|------|-------------|
| `_entity_id` | text | Unique resolved entity identifier |
| `{field_name}` | per field | One column per target field definition |

No other columns are guaranteed. Internal columns (`_last_modified`,
`_priority`, `_ts_*`) are implementation details.

---

## Detection: when is an expression "enriched"?

The engine detects an enriched expression by scanning for target name
references in FROM/JOIN position. During validation, the expression
validator extracts identifiers following `FROM` and `JOIN` keywords:

- If any match a declared target name → enriched expression
- Otherwise → regular expression (evaluated in resolution layer as today)

This is a natural extension of the existing expression safety validator.

---

## Enriched view layer

Enriched expressions are placed in `_enriched_{target}`, a thin view
between `_resolved_` and reverse. See
[COMPUTED-FIELDS-PLAN § Where in the pipeline?](COMPUTED-FIELDS-PLAN.md)
for the full rationale.

```
_resolved_shipment ───────────────────────────┐
                                              ↓
_resolved_line_item → _enriched_line_item → _rev_b_items
                                          → _rev_a_items
```

### Generated SQL

Every enriched expression is placed in a `LEFT JOIN LATERAL`. This is
the single rendering strategy — no branching between inline SELECT and
lateral. Benefits:

- **One code path** — no detection logic for `WITH RECURSIVE` vs scalar
- **`WITH RECURSIVE` just works** — valid inside lateral, invalid inline
- **Grouping is natural** — fields sharing the same subquery body share
  one lateral join

#### Aggregation example (line_item → shipment)

```sql
CREATE OR REPLACE VIEW "_enriched_line_item" AS
SELECT
  line_item.*,
  _lat_total_shipped.val AS "total_shipped",
  _lat_last_ship_date.val AS "last_ship_date",
  _lat_fully_shipped.val AS "fully_shipped"
FROM "_resolved_line_item" line_item
LEFT JOIN LATERAL (
  SELECT COALESCE(sum(s.qty), 0) AS val
  FROM "_resolved_shipment" s
  WHERE s.line_item = line_item._entity_id
) _lat_total_shipped ON true
LEFT JOIN LATERAL (
  SELECT max(s.ship_date) AS val
  FROM "_resolved_shipment" s
  WHERE s.line_item = line_item._entity_id
) _lat_last_ship_date ON true
LEFT JOIN LATERAL (
  SELECT COALESCE(sum(s.qty), 0) >= line_item.qty AS val
  FROM "_resolved_shipment" s
  WHERE s.line_item = line_item._entity_id
) _lat_fully_shipped ON true;
```

The engine wraps each user expression in
`LEFT JOIN LATERAL ({rewritten_expr}) _lat_{field} ON true`
and selects `_lat_{field}.val` in the outer query.

#### Recursive example (employee hierarchy)

```yaml
targets:
  employee:
    fields:
      email: { strategy: identity }
      name: { strategy: coalesce }
      manager: { strategy: coalesce, references: employee }

      hierarchy_path:
        strategy: expression
        expression: |
          WITH RECURSIVE cte AS (
            SELECT name::text AS path, manager, 1 AS depth
            FROM employee
            WHERE _entity_id = employee._entity_id
            UNION ALL
            SELECT p.name || ' / ' || cte.path, p.manager, cte.depth + 1
            FROM employee p
            JOIN cte ON cte.manager = p._entity_id
            WHERE cte.depth < 10
          )
          SELECT path FROM cte WHERE manager IS NULL LIMIT 1

      depth:
        strategy: expression
        expression: |
          WITH RECURSIVE cte AS (
            SELECT manager, 1 AS depth
            FROM employee
            WHERE _entity_id = employee._entity_id
            UNION ALL
            SELECT p.manager, cte.depth + 1
            FROM employee p
            JOIN cte ON cte.manager = p._entity_id
            WHERE cte.depth < 10
          )
          SELECT max(depth) FROM cte
        type: numeric
```

Generated SQL — same lateral pattern:

```sql
CREATE OR REPLACE VIEW "_enriched_employee" AS
SELECT
  employee.*,
  _lat_hierarchy_path.val AS "hierarchy_path",
  _lat_depth.val AS "depth"
FROM "_resolved_employee" employee
LEFT JOIN LATERAL (
  WITH RECURSIVE cte AS (
    SELECT name::text AS path, manager, 1 AS depth
    FROM "_resolved_employee"
    WHERE _entity_id = employee._entity_id
    UNION ALL
    SELECT p.name || ' / ' || cte.path, p.manager, cte.depth + 1
    FROM "_resolved_employee" p
    JOIN cte ON cte.manager = p._entity_id
    WHERE cte.depth < 10
  )
  SELECT path AS val FROM cte WHERE manager IS NULL LIMIT 1
) _lat_hierarchy_path ON true
LEFT JOIN LATERAL (
  WITH RECURSIVE cte AS (
    SELECT manager, 1 AS depth
    FROM "_resolved_employee"
    WHERE _entity_id = employee._entity_id
    UNION ALL
    SELECT p.manager, cte.depth + 1
    FROM "_resolved_employee" p
    JOIN cte ON cte.manager = p._entity_id
    WHERE cte.depth < 10
  )
  SELECT max(depth) AS val FROM cte
) _lat_depth ON true;
```

The engine:
1. Renders `_resolved_{target}` as usual
2. Detects fields with enriched expressions (target names in FROM/JOIN)
3. For each enriched field: wraps expression in `LEFT JOIN LATERAL`,
   aliases result column as `val`, rewrites target names to resolved views
4. Outer SELECT: `{target}.*, _lat_{field1}.val AS "{field1}", ...`
5. Downstream views (reverse, analytics) read from `_enriched_`

Targets without enriched expressions skip this layer entirely.

### Future optimization: lateral grouping

Fields sharing the same FROM/WHERE (like `total_shipped` and
`fully_shipped` above) could share one lateral join returning multiple
columns. This is a render optimization — the user's YAML stays the same.

---

## Safety validation

### New expression context: `EnrichedExpression`

```rust
enum ExprContext {
    // ... existing contexts ...
    /// expression: on target field referencing _resolved_ views
    EnrichedExpression,
}
```

### Rules for `EnrichedExpression`

| Rule | Rationale |
|------|-----------|
| Allow `SELECT`, `FROM`, `WHERE`, `JOIN`, `GROUP`, `HAVING`, `ORDER`, `DISTINCT`, `LIMIT` | Needed for subqueries |
| Allow `WITH`, `RECURSIVE`, `UNION` | Needed for recursive CTEs |
| Target names in FROM/JOIN must be declared targets | Typo protection |
| Allow `{target_name}.{field}` and `{target_name}._entity_id` | Self-correlation and cross-target |
| Block `_fwd_*`, `_id_*`, `_resolved_*`, `_rev_*`, `_delta_*`, `_grp_*`, `_ordered_*`, `_enriched_*` | Internal views — user writes bare target names |
| Block `INSERT`, `UPDATE`, `DELETE` | No DML |
| Block `CREATE`, `DROP`, `ALTER`, `TRUNCATE` | No DDL |
| Block `BEGIN`, `COMMIT`, `ROLLBACK` | No transaction control |
| Block `GRANT`, `REVOKE`, `EXECUTE`, `COPY` | No system access |
| Block `;` | No multi-statement |
| Require balanced parentheses and quotes | Same as today |

### Validating target references

The validator extracts identifiers following `FROM` and `JOIN` keywords
and checks each one:
- Must match a declared target name
- Rejects references to undeclared targets (typo protection)
- Rejects internal view prefixes (`_resolved_`, `_fwd_`, etc.)

The engine rewrites matched target names to `"_resolved_{name}"` at
render time.

---

## DAG impact

The enriched view depends on every resolved view referenced in its
expressions. The engine scans enriched expressions for target names in
FROM/JOIN position and adds edges to the DAG:

```
_resolved_shipment → _enriched_line_item
_resolved_line_item → _enriched_line_item
```

**Cycle detection:** if `_enriched_a` references `_resolved_b` and
`_enriched_b` references `_resolved_a`, that's safe — enriched views
only depend on `_resolved_` (pre-enrichment), never on other `_enriched_`
views. True cycles would require `_enriched_a` → `_enriched_b` →
`_enriched_a`, which can't happen since enriched expressions can only
reference resolved views (via bare target names).

---

## Direction semantics

Enriched expression fields are **reverse-only** — computed post-resolution,
pushed to sources via reverse views. They have no forward contribution.
See [COMPUTED-FIELDS-PLAN § Direction semantics](COMPUTED-FIELDS-PLAN.md).

## Noop detection

See [COMPUTED-FIELDS-PLAN § Noop detection](COMPUTED-FIELDS-PLAN.md). No
special handling needed.

---

## Missing-bottom example

The same scenario from COMPUTED-FIELDS-PLAN — Warehouse A has shipments,
Warehouse B doesn't — works with enriched expressions:

```yaml
targets:
  line_item:
    fields:
      line_item_id: { strategy: identity }
      order: { strategy: coalesce, references: order }
      item_name: { strategy: coalesce }
      qty: { strategy: coalesce, type: numeric }
      unit_price: { strategy: coalesce, type: numeric }
      warehouse_loc: { strategy: coalesce }

      total_shipped:
        strategy: expression
        expression: |
          COALESCE((
            SELECT sum(s.qty)
            FROM shipment s
            WHERE s.line_item = line_item._entity_id
          ), 0)
        type: numeric

      fully_shipped:
        strategy: expression
        expression: |
          COALESCE((
            SELECT sum(s.qty)
            FROM shipment s
            WHERE s.line_item = line_item._entity_id
          ), 0) >= qty

  shipment:
    fields:
      shipment_id: { strategy: identity }
      line_item: { strategy: coalesce, references: line_item }
      ship_date: { strategy: coalesce }
      qty: { strategy: coalesce, type: numeric }
      carrier: { strategy: coalesce }
```

Warehouse B receives `total_shipped` and `fully_shipped` via the reverse
pipeline without knowing about shipments.

---

## Multi-hop example

For an order-level aggregate across two hops (order → line_item →
shipment), the user writes a multi-level subquery:

```yaml
targets:
  order:
    fields:
      order_id: { strategy: identity }
      customer: { strategy: coalesce }

      total_shipped:
        strategy: expression
        expression: |
          COALESCE((
            SELECT sum(s.qty)
            FROM shipment s
            JOIN line_item li
              ON li._entity_id = s.line_item
            WHERE li."order" = order._entity_id
          ), 0)
        type: numeric
```

No new syntax — the user composes SQL joins naturally.

---

## Comparison with other approaches

| Aspect | `from:` + `match:` | Dot-path DSL | **Enriched expressions** |
|--------|-------------------|-------------|------------------------|
| New YAML properties | `from`, `match` | None (implicit) | None |
| Join condition | Declarative map | Inferred from graph | User-written SQL |
| Multi-hop | Not supported | Path segments | SQL JOINs |
| Arbitrary SQL | No | No | **Yes** |
| Learning curve | New properties | New syntax | SQL knowledge |
| Engine complexity | Match → SQL | Parse → infer → SQL | Pass-through + validate |
| Error messages | Structural | Graph traversal | PostgreSQL errors |
| Composability | Low | Medium | **High** — any SQL pattern |

### What users can do that other approaches can't

- **Recursive traversal**: `WITH RECURSIVE` for hierarchy paths, depth
- **Window functions**: `(SELECT ... OVER (PARTITION BY ...) ...)`
- **Conditional aggregation**: `sum(CASE WHEN ... THEN ... END)`
- **EXISTS predicates**: `EXISTS (SELECT 1 FROM shipment ...)`
- **String aggregation with custom ordering**: `string_agg(... ORDER BY ...)`
- **Correlated scalar lookups**: `(SELECT ... LIMIT 1)`
- **Multi-table joins in subqueries**: join across several resolved targets

### Trade-offs

- **No engine-level optimization**: the engine can't group subqueries or
  rewrite them as lateral joins (user controls the SQL)
- **PostgreSQL errors at deploy time**: syntax errors in complex expressions
  aren't caught until the view is created
- **User must know the join condition**: no inference from `references:`
  graph — the WHERE clause is the user's responsibility

---

## Implementation

### Changes to existing code

| Component | Change | Lines |
|-----------|--------|-------|
| `validate_expr.rs` | Add `EnrichedExpression` context, relax keyword rules | ~15 |
| `validate_expr.rs` | Validate FROM/JOIN targets are declared target names | ~10 |
| New: `render_enriched()` | Emit `_enriched_{target}` with lateral joins per field | ~35 |
| `render_enriched()` | Rewrite bare target names → `"_resolved_{name}"` | ~10 |
| `render_reverse()` | Read from `_enriched_` when target has enriched fields | ~5 |
| `render_analytics()` | Read from `_enriched_` when available | ~5 |
| `dag.rs` | Extract target names from FROM/JOIN, add DAG edges | ~15 |
| `model.rs` | No changes — reuses existing `expression:` property | 0 |

**Total: ~95 lines.** No model changes, no new YAML properties, no parser.

### Detection logic

```rust
fn is_enriched_expression(expr: &str, target_names: &[&str]) -> bool {
    // Scan for target names in FROM/JOIN position
    // Returns true if any declared target name appears after FROM or JOIN
}
```

### Target name rewrite

```rust
fn rewrite_target_refs(expr: &str, target_names: &[&str]) -> String {
    // Replace FROM {target} with FROM "_resolved_{target}"
    // Replace JOIN {target} with JOIN "_resolved_{target}"
    // Leave subquery aliases and other references untouched
}
```

### Render

```rust
fn render_enriched(target: &Target, enriched_fields: &[&TargetField]) -> String {
    // For each enriched field:
    //   LEFT JOIN LATERAL ({rewritten_expr}) _lat_{field} ON true
    // Outer: SELECT {target}.*, _lat_{f1}.val AS "{f1}", _lat_{f2}.val AS "{f2}", ...
    // FROM "_resolved_{target}" {target_name}
    // with target names in expressions rewritten to "_resolved_{name}"
}
```

---

## Interaction with other plans

- **COMPUTED-FIELDS-PLAN**: shares the enriched view layer. `from:` +
  `match:` becomes a convenience shorthand for users who don't want to
  write SQL subqueries. Enriched expressions are the general mechanism.
- **DOT-PATH-EXPRESSIONS-PLAN**: dot-paths become syntactic sugar that
  the engine compiles down to enriched expressions internally. Users who
  want the convenience use dot-paths; users who want control write SQL.
- **EXPRESSION-SAFETY-PLAN**: extended with the `EnrichedExpression`
  context. Existing expression contexts are unchanged.
- **COMPUTED-FIELDS-PLAN `traverse:`**: superseded — recursive hierarchy
  is expressible via `WITH RECURSIVE` in enriched expressions with
  lateral placement. No dedicated `traverse:` syntax needed.

## Open questions

1. **Should we optimize repeated subqueries?** Multiple fields referencing
   the same target with the same WHERE clause could share a
   `LEFT JOIN LATERAL`. This is a rendering optimization — the user's YAML
   stays the same. Worth doing in a follow-up if performance matters.

2. **Should we validate subquery structure?** The current plan validates
   FROM/JOIN targets but not SQL syntax. A future phase could use a
   lightweight SQL parser (like `sqlparser-rs`) to catch syntax errors
   at compile time. For now, PostgreSQL catches them at deploy time.

3. **Target name collision with user aliases.** If a user writes
   `FROM shipment shipment` (target name as alias), the rewrite produces
   `FROM "_resolved_shipment" shipment` — valid SQL, just redundant.
   If a user aliases a subquery column as a target name, the outer
   reference stays unambiguous because target-name rewriting only applies
   to FROM/JOIN position. Low risk.
