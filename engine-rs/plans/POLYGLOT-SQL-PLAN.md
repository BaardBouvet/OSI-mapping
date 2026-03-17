# Polyglot SQL — multi-dialect rendering

**Status:** Design

Should the engine use [polyglot-sql](https://github.com/tobilg/polyglot) to
generate SQL for Snowflake, BigQuery, and other targets — and if so, how?

## Context

The engine currently builds SQL as strings via `format!()` and
`String::push_str()`, targeting PostgreSQL exclusively. The entire render
pipeline (~2,800 lines across six modules) emits raw PostgreSQL syntax.

polyglot-sql is a Rust port of Python's sqlglot. It provides:

- **Parser**: SQL string → AST (`Expression` tree), dialect-aware.
- **Generator**: AST → SQL string, dialect-aware, with 35+ target dialects.
- **Transpiler**: One-shot `transpile(sql, read_dialect, write_dialect)`.
- **Transforms**: Dialect-specific rewrites (e.g., `DISTINCT ON` → `ROW_NUMBER`,
  `string_agg` → `LISTAGG`, `jsonb_array_elements` → `FLATTEN`).
- **AST introspection**: `get_column_names()`, `get_identifiers()`,
  `get_table_names()`, etc.

The crate is 220K lines of Rust (vs our engine's 5.7K). It covers PostgreSQL,
Snowflake, BigQuery, DuckDB, Redshift, MySQL, TSQL, and 28 more dialects.

## Question 1: Should we adopt polyglot-sql at all?

### Arguments for

1. **Multi-dialect is table-stakes for adoption.** Enterprise MDM rarely runs
   on a single warehouse. Supporting Snowflake and BigQuery alongside
   PostgreSQL removes the largest adoption blocker.
2. **The hard problems are already solved.** polyglot-sql already handles
   `DISTINCT ON` → `ROW_NUMBER`, `string_agg` → `LISTAGG`/`STRING_AGG`,
   `jsonb_array_elements` → `FLATTEN`/`UNNEST`, JSONB arrow operators →
   `JSON_EXTRACT`/colon-access, `LATERAL` → dialect equivalents,
   `IS NOT DISTINCT FROM` → `COALESCE` comparisons, `WITH RECURSIVE` →
   dialect-specific forms, and type cast syntax differences.
3. **Expression safety improves.** Parsing user expressions into an AST gives
   structural validation for free — no regex heuristics needed. Phase 2 of
   EXPRESSION-SAFETY-PLAN (column reference validation) becomes trivial
   with `get_column_names()`.
4. **Future features benefit.** COMPUTED-FIELDS-PLAN and any cross-target
   access need expression analysis; an AST makes this straightforward.

### Arguments against

1. **Binary size and compile time.** 220K lines is 40× our engine. Debug
   builds will grow significantly. Compile times increase.
2. **Fidelity risk.** polyglot-sql is a port of sqlglot, not a battle-tested
   production SQL compiler. Edge cases in PostgreSQL-specific syntax (e.g.,
   `jsonb_build_object` with complex nesting, `WITH ORDINALITY`, `md5()`,
   `FILTER (WHERE ...)` on aggregates) may not round-trip perfectly.
3. **Coupling to a dependency.** The engine's SQL generation is currently
   self-contained. polyglot-sql is under active development — API changes
   could break our render pipeline.
4. **Not all constructs transpile cleanly.** Some PostgreSQL idioms have no
   semantic equivalent in other dialects (e.g., `DISTINCT ON` in BigQuery
   requires a full rewrite to `ROW_NUMBER`; recursive CTEs may not be supported
   in all BigQuery contexts).

### Verdict

Adopt polyglot-sql, but **incrementally** — not all-in from day one. See
the phased approach below.

## Question 2: Which dialect should snippet-expressions use?

Mapping authors write expressions like `SPLIT_PART(name, ' ', 1)` and
`string_agg(distinct type, ',' order by type)`. These are interpolated
verbatim into generated SQL. Three options:

### Option A: Expressions stay PostgreSQL dialect

Authors write PostgreSQL syntax. The engine renders full views in PostgreSQL,
then transpiles the entire view to the target dialect.

- **Pro:** Zero migration for existing mappings. PostgreSQL has the richest
  function library. Authors already know the syntax.
- **Pro:** Transpilation happens at the view level, where polyglot-sql has the
  most context about surrounding scope (it can see the full SELECT, FROM,
  WHERE context and rewrite accordingly).
- **Con:** Some PostgreSQL functions have no equivalent in target dialects.
  The engine can only detect this at transpile time, not at mapping authoring
  time.
- **Con:** Authors targeting Snowflake-first must still learn PostgreSQL syntax.

### Option B: Expressions use target dialect syntax

Authors write in their deployment dialect (e.g., Snowflake SQL). The engine
parses expressions per-dialect and normalizes to an internal AST.

- **Pro:** Authors write what they know.
- **Con:** Mappings become non-portable — switching from Snowflake to BigQuery
  requires rewriting all expressions.
- **Con:** The engine must parse snippets, not full statements, in varying
  dialects. Fragment-level parsing is fragile.

### Option C: Expressions use a "generic" dialect

Define a portable subset (ANSI SQL + well-known functions). Transform to each
target dialect.

- **Pro:** Truly portable mappings.
- **Con:** Authors can't use dialect-specific functions they actually need.
- **Con:** "Generic SQL" is a leaky abstraction — it must still be defined,
  documented, and tested per function.

### Recommendation: Option A — PostgreSQL-native, transpile on output

This is the pragmatic choice:

1. All existing expressions already are PostgreSQL.
2. polyglot-sql's transpiler works best on full statements, not fragments.
3. PostgreSQL is the most feature-rich open-source SQL dialect — it's the
   best "source of truth" for expressions.
4. If an author needs a function that doesn't transpile, that's a signal
   to add a translation rule, not to change the input dialect.

The engine would: generate PostgreSQL SQL → parse via polyglot → transform
to target dialect → emit dialect-specific SQL. Mapping YAML never changes.

## Question 3: Performance impact

### Current approach (string building)

The engine renders SQL for all examples in under 10ms. String building is
effectively zero-cost — no allocation overhead beyond the output string.

### Polyglot round-trip (render → parse → transform → generate)

Three additional steps per view:

1. **Parse**: Tokenize + build AST from the PostgreSQL SQL we just emitted.
   polyglot-sql's parser handles full DDL/DML; our views are medium-complexity
   SELECTs with CTEs. Estimated: ~1ms per view.
2. **Transform**: Walk AST applying dialect rewrites. Proportional to AST node
   count. Estimated: ~0.5ms per view.
3. **Generate**: Walk AST emitting target SQL. Proportional to node count.
   Estimated: ~0.5ms per view.

For a typical mapping with 20 views, the overhead is ~40ms — imperceptible
for a schema-generation tool that runs at deploy time, not per-query.

### Alternative: Build AST directly (skip parse step)

Instead of generating PostgreSQL strings and re-parsing, build the polyglot
AST directly using `Expression` constructors. This eliminates step 1.

- **Pro:** Removes redundant string→AST round-trip. Slightly faster.
- **Con:** Massive refactor — rewrite 2,800 lines of render code to build
  `Expression` trees instead of strings. polyglot-sql's `Expression` enum has
  hundreds of variants; our `format!()` calls would become deeply nested
  constructor chains. Code readability drops significantly.
- **Con:** Tightly couples render code to polyglot-sql's internal AST
  representation. API changes in polyglot-sql break our entire render pipeline.
- **Con:** Loses the ability to eyeball generated SQL during debugging — you'd
  need to generate→print at every step.

### Recommendation: Render PostgreSQL strings, transpile afterward

The performance cost is negligible for a deploy-time tool. The code stays
readable. The polyglot dependency is isolated to a thin transpilation layer.

## Question 4: What about building the AST directly?

This is the "go all in" option: replace `format!()` with polyglot-sql's
`Expression` builders so the engine never produces a string until the final
generate step.

### Why not

1. **Proportionality.** Our engine is 5.7K lines. polyglot-sql's `Expression`
   enum alone is 13K lines with hundreds of variants. Using it as a builder
   means learning a much larger API surface than the SQL we're generating.
2. **Readability.** Compare:
   ```rust
   // Current — instantly readable:
   format!("SELECT {} FROM {} WHERE {}", cols, table, filter)

   // AST builder — structural but opaque:
   Expression::Select(Box::new(Select {
       expressions: col_exprs,
       from: Some(From { this: table_expr, .. }),
       where_: Some(Where { this: filter_expr }),
       ..Default::default()
   }))
   ```
3. **Debugging.** With string building, every intermediate SQL fragment is
   `println!`-able. With AST building, you need a generate step to see output.
4. **Coupling.** An AST-builder approach couples us to polyglot-sql's internals
   irreversibly. The string-render + transpile approach lets us swap polyglot
   for any other transpiler (or remove it) without touching render code.

### When it might make sense

If the engine grows to support 10+ dialects with incompatible semantics (not
just syntax), building the AST might become worthwhile. That's a post-2.0
concern.

## Proposed implementation plan

### Phase A — Expression-level transpilation (snippet validation)

Scope: validate_expr.rs only. No render pipeline changes.

1. Use `polyglot_sql::parse()` to parse user expressions (wrapped in
   `SELECT expr`) for structural validation.
2. Use `get_column_names()` / `get_identifiers()` for Phase 2 of
   EXPRESSION-SAFETY-PLAN (column reference validation).
3. Use `transpile()` to check that each expression is translatable to the
   configured target dialect. Warn if a function has no equivalent.

**Deliverable:** Expression validation uses AST instead of regex. Warnings
for non-portable expressions. Zero impact on render pipeline.

**Risk:** Low. polyglot-sql is only used in the validation pass. If it fails
to parse an expression, fall back to the existing regex-based validation.

### Phase B — View-level transpilation (multi-dialect output)

Scope: New `transpile` module wrapping the render pipeline.

1. Render pipeline continues to emit PostgreSQL SQL strings (no changes).
2. New `transpile_views(views: &[View], target: DialectType) -> Vec<String>`
   function parses each PostgreSQL view and transpiles to the target dialect.
3. CLI gains a `--dialect` flag (default: `postgresql`).
4. Integration tests run each example against PostgreSQL (native) and verify
   that the Snowflake/BigQuery output is syntactically valid (parse-only, no
   execution — we don't have Snowflake/BigQuery test infrastructure).

**Deliverable:** `osi-engine render --dialect snowflake` produces Snowflake
SQL. Existing PostgreSQL path unchanged.

**Risk:** Medium. Some views may use PostgreSQL idioms that polyglot can't
transpile cleanly. Mitigation: run all ~40 examples through the transpiler
in CI and fix edge cases incrementally. The PostgreSQL path is always the
fallback.

### Phase C — Dialect-aware expression warnings

Scope: Validation pipeline.

1. When `--dialect` is set, validate each user expression for portability
   by transpiling it to the target dialect.
2. Warn on functions or syntax that have no target equivalent (e.g.,
   `REGEXP_REPLACE` with flags in BigQuery).
3. Suggest alternatives where possible.

**Deliverable:** `osi-engine validate --dialect snowflake` warns about
non-portable expressions.

### Phase D — Native dialect back-ends (post-2.0, if ever needed)

If transpilation proves insufficient for specific dialects (e.g., BigQuery's
STRUCT-based approach fundamentally differs from PostgreSQL's relational
model), build dialect-specific render back-ends that emit native SQL directly.

This should only be considered if Phase B's transpilation produces incorrect
results that can't be fixed by adding polyglot-sql transform rules.

## Dependency management

polyglot-sql should be an **optional** dependency gated behind a `multi-dialect`
cargo feature flag:

```toml
[dependencies]
polyglot-sql = { version = "0.1", optional = true }

[features]
default = []
multi-dialect = ["polyglot-sql"]
```

The PostgreSQL-only path (current behavior) has zero dependency on polyglot.
Expression validation (Phase A) and multi-dialect output (Phase B) are
opt-in.

## Summary of recommendations

| Question | Recommendation |
|----------|---------------|
| Adopt polyglot-sql? | Yes, incrementally behind a feature flag. |
| Expression dialect? | PostgreSQL. Transpile on output. |
| Performance? | Render strings → transpile. Negligible overhead for deploy-time tool. |
| Build AST directly? | No. String render + transpile keeps code readable and decoupled. |
| When? | Phase A during expression-safety work. Phase B after 1.0 schema stabilizes. |

## Appendix: dbt's "write in your target dialect" approach

dbt requires users to write SQL in their deployment warehouse's native dialect.
A dbt project targeting Snowflake contains Snowflake SQL; switching to BigQuery
means rewriting models. This is a deliberate design choice, not a limitation.

### Why dbt chose native dialect

**1. What you write is what runs.**
dbt's core promise is that models are just SQL — no hidden transformations.
When an analytics engineer writes `DATE_TRUNC('month', col)` in Snowflake,
that exact text reaches the query planner. No transpiler sits between intent
and execution, so there are no transpilation bugs, no subtle semantic drift
(timezone handling, null coercion, rounding behavior), and no "works in local
testing but breaks in production" surprises.

**2. The full dialect surface matters in analytics.**
dbt models are arbitrary SQL transformations. Analytics engineers routinely
use dialect-specific features: Snowflake's `QUALIFY`, `FLATTEN`, `VARIANT`
type, `OBJECT_CONSTRUCT`, `$$` scripting, semi-structured access via `:`;
BigQuery's `STRUCT`, `UNNEST`, `SAFE_DIVIDE`, `APPROX_COUNT_DISTINCT`,
partitioned/clustered table DDL, ML functions (`ML.PREDICT`); Redshift's
`DISTSTYLE`, `SORTKEY`, late-binding views. A transpiler cannot cover this
surface without becoming the dialect itself.

**3. Analytics engineers already know their warehouse.**
dbt's users are analysts/analytics engineers who work daily in their
warehouse's SQL console. Asking them to learn a generic dialect adds friction
for no benefit — they'd be writing code they can't directly test in their
query editor. dbt optimizes for this workflow: write in your editor, test
with `dbt run`, iterate.

**4. The Jinja macro layer handles cross-dialect where needed.**
dbt doesn't ignore portability — it provides `adapter.dispatch()` and dbt
packages (e.g., `dbt_utils`) with per-adapter macro implementations.
`{{ dbt_utils.datediff('day', 'start', 'end') }}` expands to the right
syntax for each adapter. This is opt-in: only cross-database packages use
it; project-specific models stay in native SQL.

**5. Transpilers are a maintenance liability.**
sqlglot (the Python library polyglot-sql ports) has 1,100+ open issues.
Each dialect pair has edge cases. dbt's adapter authors reasoned that
maintaining per-adapter SQL generation for `ref()`, `source()`,
materialization DDL, and grants was enough complexity without also
maintaining a transpiler for user SQL.

### The counterpoint: sqlmesh

Tobiko Data's sqlmesh (built by the sqlglot author) took the opposite bet: users
write in any dialect, sqlmesh transpiles to the target. This enables writing
once and deploying to multiple warehouses, and testing locally on DuckDB then
deploying to Snowflake. The tradeoff: users inherit sqlglot's edge cases, and
models that use dialect-specific features still need per-dialect variants.

sqlmesh's bet is that most SQL is portable enough that transpilation covers
90%+ of models, and the remaining 10% can use dialect-specific macros. dbt's
bet is that 100% fidelity matters more than portability. Both have succeeded
in the market.

### Why our situation differs from dbt's

Our engine is **not** dbt. The differences are fundamental:

| Dimension | dbt | Our engine |
|-----------|-----|-----------|
| **User-written SQL** | Entire models (SELECT/CTE/JOIN/subquery) | Column-level snippets only (`SPLIT_PART(name, ' ', 1)`) |
| **Generated SQL** | None — the model IS the SQL | ~95% of the SQL is engine-generated (CTEs, JOINs, UNION ALL, CASE, etc.) |
| **Dialect surface** | Full — any construct the warehouse supports | Narrow — functions, operators, casts, CASE, literals |
| **Portability unit** | The model | The mapping YAML |
| **User persona** | Analytics engineer (SQL expert) | Data architect / integration engineer (YAML-first) |

dbt's users write the SQL; our users declare mappings and the engine writes
the SQL. This means:

1. **We control 95% of the output.** The engine generates the CTEs, recursive
   identity resolution, LATERAL joins, DISTINCT ON, delta classification, and
   JSONB reconstruction. The user only contributes scalar snippets. Transpiling
   engine-generated SQL is tractable because we know exactly what constructs
   appear.

2. **User snippets are narrow.** The expression-safety validator already
   rejects `SELECT`, `FROM`, `JOIN`, subqueries, and DDL. What remains —
   function calls, operators, casts, CASE — is exactly the surface that
   transpilers handle best.

3. **Mapping portability is the product.** A mapping YAML should describe the
   relationship between source and target **independently** of the deployment
   warehouse. Requiring Snowflake-dialect expressions in the YAML would mean
   the mapping format itself is dialect-locked — the opposite of our design.

4. **dbt's escape hatch (Jinja macros) doesn't apply.** We don't have a macro
   layer. If expressions must be dialect-specific, every mapping author must
   learn the expression differences between PostgreSQL and Snowflake. With
   transpilation, they learn one dialect (PostgreSQL) and the engine handles
   the rest.

### Conclusion

dbt's choice is correct **for dbt** — full models need full fidelity, and dbt's
users are SQL-native. Our choice (Option A: PostgreSQL expressions, transpile
on output) is correct **for us** — column-level snippets transpile reliably,
engine-generated SQL is fully controlled, and mapping portability matters more
than dialect freedom.

The risk that a PostgreSQL function has no equivalent in a target dialect is
real but manageable: the engine can warn at validation time (Phase C of this
plan), and the number of functions used in typical mapping expressions is
small (we found ~15 distinct functions across all 38 examples).
