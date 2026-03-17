# dbt project as output target

**Status:** Design

Analysis of generating a complete dbt project from a mapping YAML, as an
alternative to the current monolithic SQL script.  The engine already
produces a DAG of PostgreSQL views; this plan maps that DAG onto dbt
concepts and identifies the structural decisions, trade-offs, and
implementation steps.

---

## Motivation

The current `render` command emits a single SQL script:

```
BEGIN;
CREATE OR REPLACE VIEW _fwd_crm ...;
CREATE OR REPLACE VIEW _id_company ...;
...
COMMIT;
```

This is a fine deployment artefact for `psql -f`, but many teams already
run **dbt** as their transformation layer.  A dbt output would let them:

1. **Slot OSI mappings into an existing dbt project** — materialise views
   alongside hand-written models, share the same `profiles.yml`, same CI.
2. **Leverage dbt's built-in features** — incremental materialisation,
   `dbt test`, `dbt docs generate`, lineage graph, freshness checks.
3. **Version-control transformations** — dbt projects live in Git; generated
   models can be committed and reviewed like any other model.
4. **Use dbt Cloud / dbt-core orchestration** — scheduling, environment
   promotion, audit logs.
5. **Future multi-dialect** — dbt adapters already handle Snowflake,
   BigQuery, Redshift, Databricks.  If the engine eventually supports
   dialect transpilation (see POLYGLOT-SQL-PLAN), the dbt adapter handles
   DDL differences automatically.

### Non-goal

This plan does **not** propose running dbt at engine compile time.  The
engine remains a static compiler: `mapping.yaml → dbt project files`.
dbt-core (or dbt Cloud) runs the project separately.

---

## dbt project anatomy

A minimal dbt project for a single mapping would look like:

```
dbt_osi/
  dbt_project.yml
  models/
    _sources.yml             # source declarations
    staging/
      _fwd_crm_orgs.sql     # forward view
      _fwd_erp_orgs.sql
    identity/
      _id_company.sql        # identity view (recursive CTE)
    resolution/
      _resolved_company.sql  # resolved golden record
    marts/
      company.sql            # analytics (consumer-facing)
    reverse/
      _rev_crm_orgs.sql      # reverse projection
      _rev_erp_orgs.sql
    delta/
      _delta_crm.sql         # change detection
      _delta_erp.sql
  tests/                     # optional schema tests
    ...
```

Each `.sql` file is a dbt model containing `SELECT ...` (no `CREATE VIEW`
wrapper—dbt handles materialisation).

### Key mapping: engine concepts → dbt concepts

| Engine concept | dbt equivalent |
|---|---|
| Source table | `sources:` in `_sources.yml` |
| `CREATE OR REPLACE VIEW` | dbt model (`.sql` file) with `{{ config(materialized='view') }}` |
| View dependency order (DAG) | dbt `{{ ref('model_name') }}` / `{{ source('schema','table') }}` |
| `BEGIN; ... COMMIT;` | Not needed — dbt manages transactions per model |
| `--create-tables` flag | `sources:` declaration + `dbt seed` or external loader |
| `--annotate` flag | dbt model description in `.yml` schema file |
| `_osi_text_norm()` function | dbt macro in `macros/` directory |
| Topological view order | Automatic — dbt resolves from `ref()` / `source()` calls |

---

## Design decisions

### D1: Model file structure

**Decision:** One `.sql` file per view, organised in subdirectories by
pipeline stage.

Alternatives considered:
- **Flat directory** — all models in `models/`.  Gets unwieldy with 20+
  views.  Rejected.
- **One file per target** — combine forward + identity + resolution into a
  single model with CTEs.  Breaks dbt's per-model materialisation and
  lineage graph.  Rejected.
- **Stage directories** — `staging/`, `identity/`, `resolution/`, `marts/`,
  `reverse/`, `delta/`.  Aligns with dbt best practice (staging → marts)
  and maps cleanly to the engine's view types.  **Chosen.**

### D2: Source references via `{{ source() }}`

Forward views currently `SELECT ... FROM crm`.  In dbt this becomes:

```sql
SELECT ... FROM {{ source('osi', 'crm') }}
```

The engine emits a `_sources.yml` declaring each source dataset:

```yaml
sources:
  - name: osi
    schema: "{{ var('osi_source_schema', 'public') }}"
    tables:
      - name: crm
        description: "Source dataset: crm"
      - name: registry
        description: "Source dataset: registry"
```

Using a `var()` for the schema lets teams override it per environment.

### D3: Model references via `{{ ref() }}`

Inter-view dependencies (e.g., identity → forward, resolution → identity)
use `{{ ref('_fwd_crm_orgs') }}` instead of bare table names.  This gives
dbt full lineage visibility and correct execution ordering.

Example — identity view body:

```sql
-- models/identity/_id_company.sql
{{ config(materialized='view') }}

WITH RECURSIVE
  _id_base AS (
    SELECT * FROM {{ ref('_fwd_crm_orgs') }}
    UNION ALL
    SELECT * FROM {{ ref('_fwd_erp_orgs') }}
  ),
  ...
```

### D4: Materialisation strategy

Default: `view` for all models (matching current behaviour).

But the user may want to materialise some views as tables or incremental
models for performance.  The engine should respect an optional
`materialisation` override per pipeline stage, configured either:

- **Per-mapping YAML property** (future, not in v1):
  ```yaml
  targets:
    company:
      materialisation: table   # resolved + analytics as TABLE
  ```
- **dbt_project.yml defaults** (generated, user can edit):
  ```yaml
  models:
    osi:
      staging:
        +materialized: view
      identity:
        +materialized: view
      resolution:
        +materialized: table
      marts:
        +materialized: view
      reverse:
        +materialized: view
      delta:
        +materialized: view
  ```

For v1, the engine generates all models as `view` and emits a
`dbt_project.yml` with sensible defaults that users can override.

### D5: The `_osi_text_norm()` function

This PL/pgSQL function is needed for nested-array noop detection.  In dbt
it becomes a **macro**:

```sql
-- macros/osi_text_norm.sql
{% macro create_osi_text_norm() %}
  CREATE OR REPLACE FUNCTION _osi_text_norm(j jsonb) RETURNS jsonb ...;
{% endmacro %}
```

Called via a **run-operation** or a **pre-hook** on models that need it.

Alternatively, wrap the function in a dbt `on-run-start` hook in
`dbt_project.yml`:

```yaml
on-run-start:
  - "{{ create_osi_text_norm() }}"
```

This ensures the function exists before any model references it.

### D6: Cluster members tables

Mappings with `cluster_members: true` reference per-mapping tables
(`_cluster_members_{mapping}`).  These are declared as additional sources:

```yaml
sources:
  - name: osi
    tables:
      - name: _cluster_members_crm_orgs
        description: "Cluster members feedback table for mapping crm_orgs"
```

The forward view references them via `{{ source('osi', '_cluster_members_crm_orgs') }}`.

### D7: Schema tests (optional)

The engine can generate dbt schema tests from the mapping structure:

```yaml
# models/marts/_schema.yml
models:
  - name: company
    description: "Golden record: company"
    columns:
      - name: _cluster_id
        tests:
          - not_null
          - unique
      - name: org_number
        tests:
          - not_null  # only if identity strategy
```

- `identity` strategy fields → `not_null` + `unique` (within entity)
- `_cluster_id` on analytics → always `not_null` + `unique`

This is opt-in — generated only when a `--tests` flag is passed.

### D8: Naming convention

dbt model names must be unique across the project.  The engine already
generates globally unique view names (`_fwd_{mapping}`, `_id_{target}`,
etc.), so model file names map directly:

| View | Model file |
|---|---|
| `_fwd_crm_orgs` | `staging/_fwd_crm_orgs.sql` |
| `_id_company` | `identity/_id_company.sql` |
| `_resolved_company` | `resolution/_resolved_company.sql` |
| `company` | `marts/company.sql` |
| `_rev_crm_orgs` | `reverse/_rev_crm_orgs.sql` |
| `_delta_crm` | `delta/_delta_crm.sql` |

### D9: Multi-mapping support

A single `mapping.yaml` may define multiple targets, each with multiple
source mappings.  All end up in the same dbt project, coexisting naturally
because view names are already unique.

For workflows where multiple mapping files should merge into one dbt
project, the engine could accept a directory:

```bash
osi-engine dbt mappings/ --output dbt_osi/
```

This is a v2 concern — v1 supports one mapping file → one dbt project.

### D10: The delta view and multi-mapping merging

Delta views merge multiple reverse views for the same source (child merge,
routing, etc.) using CTEs and LEFT JOINs.  These translate directly into
dbt SQL with `{{ ref() }}` calls replacing bare view names:

```sql
-- models/delta/_delta_crm.sql
{{ config(materialized='view') }}

WITH
  _p AS (SELECT * FROM {{ ref('_rev_crm_orgs') }}),
  _e1 AS (SELECT _src_id, tags, _base FROM {{ ref('_rev_crm_tags') }}),
  _merged AS (
    SELECT _p.*, _e1.tags,
           _p._base || COALESCE(_e1._base, '{}'::jsonb) AS _base
    FROM _p LEFT JOIN _e1 ON _e1._src_id = _p._src_id
  )
SELECT
  CASE ... END AS _action,
  ...
FROM _merged;
```

---

## Implementation plan

### Phase 1: Core scaffolding

Add a `dbt` subcommand to the CLI:

```bash
osi-engine dbt <mapping.yaml> --output <dir> [--tests] [--annotate]
```

**Deliverables:**
1. **`render/dbt.rs`** — new render module:
   - `render_dbt_project(doc, dag, output_dir, tests, annotate) → Result<()>`
   - Writes files to disk (unlike `render_sql` which returns a string)
2. **`dbt_project.yml` generation** — project name derived from mapping
   `description` or file name; default materialisation config
3. **`_sources.yml` generation** — one entry per source dataset +
   cluster_members tables
4. **Model files** — one `.sql` per view, using `{{ ref() }}` and
   `{{ source() }}` instead of bare table names
5. **Macro files** — `_osi_text_norm` macro when nested arrays are present
6. **CLI integration** — new `Command::Dbt` variant in `main.rs`

**SQL transformation required:**

The existing render functions return `CREATE OR REPLACE VIEW name AS SELECT ...`.
For dbt output we need only the `SELECT ...` body (dbt wraps it in DDL).
Two approaches:

- **A) Strip the wrapper** — regex or string split on `AS\n` after the
  view name.  Fragile if view definitions evolve.
- **B) Factor render functions** — each render function returns a struct
  `ViewOutput { name, body, deps }` where `body` is the raw SELECT.  The
  SQL renderer wraps it in `CREATE VIEW`; the dbt renderer writes it as-is
  with `{{ ref() }}` substitution.

**Recommendation: B** — cleaner separation, makes both renderers
first-class.  The refactor is internal and doesn't change the SQL output.

```rust
pub struct ViewOutput {
    pub name: String,
    pub body: String,
    pub annotations: Vec<String>,
    pub deps: Vec<ViewRef>,        // source() or ref() dependencies
}

pub enum ViewRef {
    Source { schema: String, table: String },
    Ref(String),  // model name
}
```

Each render function (`render_forward_view`, etc.) returns `ViewOutput`.
`render_sql()` wraps: `CREATE OR REPLACE VIEW {name} AS\n{body}`.
`render_dbt_model()` writes: `{{ config(...) }}\n\n{body_with_refs}`.

### Phase 2: ref() substitution

The body returned by render functions contains bare table/view references
(`FROM crm`, `FROM _fwd_crm_orgs`).  The dbt renderer must replace these:

| Pattern | Replacement |
|---|---|
| `FROM {source_table}` | `FROM {{ source('osi', '{source_table}') }}` |
| `FROM _fwd_{mapping}` | `FROM {{ ref('_fwd_{mapping}') }}` |
| `FROM _id_{target}` | `FROM {{ ref('_id_{target}') }}` |
| `FROM _resolved_{target}` | `FROM {{ ref('_resolved_{target}') }}` |
| `JOIN _cluster_members_{m}` | `JOIN {{ source('osi', '_cluster_members_{m}') }}` |

With the `ViewOutput` approach from Phase 1, the deps already enumerate
every reference.  The substitution replaces quoted identifiers in the body
string using the dep list (no regex guessing).

**Better approach:** render functions emit placeholder tokens in the body:

```sql
FROM __REF__(_fwd_crm_orgs)__
```

- SQL renderer: `__REF__(name)__` → `"name"` (quoted identifier)
- dbt renderer: `__REF__(name)__` → `{{ ref('name') }}`
- Source refs: `__SRC__(schema, table)__` → `{{ source('schema', 'table') }}`

This avoids fragile post-hoc string replacement entirely.

### Phase 3: Schema and tests

Generate `_schema.yml` files per subdirectory with model descriptions and
optional column-level tests.

```yaml
# models/marts/_schema.yml
version: 2
models:
  - name: company
    description: "Resolved golden record for target: company"
    columns:
      - name: _cluster_id
        description: "Stable entity identifier"
        tests:
          - not_null
          - unique
```

### Phase 4: Incremental materialisation (future)

For large datasets, delta views are natural candidates for incremental
materialisation.  The delta view already classifies rows by `_action`;
an incremental dbt model would use `_action != 'noop'` as the filter:

```sql
{{ config(materialized='incremental', unique_key='_src_id') }}

SELECT ... FROM {{ ref('_rev_crm_orgs') }}
{% if is_incremental() %}
WHERE _last_modified > (SELECT max(_last_modified) FROM {{ this }})
{% endif %}
```

This is a future optimisation and depends on the mapping having a
`last_modified` timestamp available.

---

## Output example

Given the `hello-world` example (2 sources, 1 target), the dbt command:

```bash
osi-engine dbt examples/hello-world/mapping.yaml --output dbt_hello/
```

would produce:

```
dbt_hello/
  dbt_project.yml
  macros/
    (empty — no nested arrays)
  models/
    _sources.yml
    staging/
      _fwd_crm_contacts.sql
      _fwd_erp_contacts.sql
    identity/
      _id_contact.sql
    resolution/
      _resolved_contact.sql
    marts/
      contact.sql
    reverse/
      _rev_crm_contacts.sql
      _rev_erp_contacts.sql
    delta/
      _delta_crm.sql
      _delta_erp.sql
```

**`dbt_project.yml`:**
```yaml
name: osi_hello_world
version: '1.0.0'
config-version: 2
profile: default

model-paths: ["models"]
macro-paths: ["macros"]

models:
  osi_hello_world:
    staging:
      +materialized: view
    identity:
      +materialized: view
    resolution:
      +materialized: view
    marts:
      +materialized: view
    reverse:
      +materialized: view
    delta:
      +materialized: view
```

**`models/_sources.yml`:**
```yaml
version: 2
sources:
  - name: osi
    schema: "{{ var('osi_source_schema', 'public') }}"
    tables:
      - name: crm
      - name: erp
```

**`models/staging/_fwd_crm_contacts.sql`:**
```sql
{{ config(materialized='view') }}

SELECT
  md5('crm_contacts' || ':' || "id"::text)::text AS _src_id,
  'crm_contacts'::text AS _mapping,
  md5('crm_contacts' || ':' || "id"::text) AS _cluster_id,
  1 AS _priority,
  NULL::timestamptz AS _last_modified,
  "email"::text AS "email",
  ...
  jsonb_build_object('email', "email", 'name', "name") AS _base
FROM {{ source('osi', 'crm') }}
```

**`models/identity/_id_contact.sql`:**
```sql
{{ config(materialized='view') }}

WITH RECURSIVE
  _id_base AS (
    SELECT * FROM {{ ref('_fwd_crm_contacts') }}
    UNION ALL
    SELECT * FROM {{ ref('_fwd_erp_contacts') }}
  ),
  ...
```

---

## Relationship to other plans

| Plan | Interaction |
|---|---|
| [POLYGLOT-SQL-PLAN](POLYGLOT-SQL-PLAN.md) | If adopted, dbt output benefits automatically — dbt adapters handle dialect DDL; polyglot transpiles expression snippets. The `ViewOutput` refactor benefits both. |
| [EXPRESSION-SAFETY-PLAN](EXPRESSION-SAFETY-PLAN.md) | No direct interaction — expression validation runs before rendering. |
| [COMPUTED-FIELDS-PLAN](COMPUTED-FIELDS-PLAN.md) | Enriched views (`_enriched_`) become additional dbt models between resolution and reverse models. |
| [ANALYTICS-PROVENANCE-PLAN](ANALYTICS-PROVENANCE-PLAN.md) | Provenance views map to additional dbt models in a `provenance/` subdirectory. |
| [PASSTHROUGH-PLAN](PASSTHROUGH-PLAN.md) | Passthrough columns flow through forward → delta; no dbt-specific concern. |

---

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| **dbt version churn** | Generated YAML/Jinja may break on dbt upgrades | Target dbt-core 1.x conventions (stable); test against latest release in CI |
| **Recursive CTEs** | Some dbt adapters (BigQuery) limit recursive CTEs | Identity view is PostgreSQL-specific today; cross-dialect is a POLYGLOT-SQL concern, not a dbt-output concern |
| **PL/pgSQL function** | `_osi_text_norm` requires `CREATE FUNCTION` permission | Emit as `on-run-start` macro; document that nested-array mappings need function-creation privileges |
| **Large projects** | 50+ models from a complex mapping may clutter dbt lineage | Stage subdirectories keep it organised; dbt tags and selectors let users build subsets (`dbt run --select staging+`) |
| **Render refactor scope** | `ViewOutput` refactor touches all 6 render modules | Each module is independent; refactor one at a time with existing tests as safety net |

---

## Effort estimate

| Phase | Scope | Size |
|---|---|---|
| 1 | `ViewOutput` refactor + `render/dbt.rs` + CLI | Medium — touches all render modules but changes are mechanical (extract body from CREATE VIEW) |
| 2 | `ref()` / `source()` substitution | Small — token-based replacement on body strings |
| 3 | Schema YAML + tests | Small — template generation from model metadata |

---

## Open questions

1. **Project naming** — derive from mapping `description`, filename, or
   require an explicit `--name` flag?
2. **Multiple mapping files** — v2 concern, but should the directory
   structure anticipate it (e.g., `models/{mapping_name}/staging/`)?
3. **Custom dbt profiles** — should the engine generate a `profiles.yml`
   or only `dbt_project.yml` (letting the user supply their own profile)?
4. **Existing dbt project** — should the engine support writing *into* an
   existing dbt project (appending models) rather than creating a standalone
   one?  This avoids conflicts but needs care around `_sources.yml` merging.
5. **Materialisation hints in YAML** — not needed.  Users override
   materialisation in `dbt_project.yml`, which is standard dbt practice.
   The stage subdirectories make this straightforward.

---

## Appendix A: Custom materialisations (e.g., pg_trickle)

### Design philosophy

The engine generates plain dbt models with `materialized='view'`.
It does **not** embed knowledge of specific dbt packages, materialisation
plugins, or PostgreSQL extensions.  Users bring their own materialisation
strategy by editing the generated `dbt_project.yml` and `packages.yml` —
which is standard dbt practice.

This keeps the engine simple and avoids coupling to any specific plugin's
lifecycle, configuration schema, or PostgreSQL version requirements.

### How dbt materialisation overrides work

dbt lets users override materialisation at any level — per-model, per-
directory, or project-wide — in `dbt_project.yml`.  The engine generates
models in stage-based subdirectories (`staging/`, `identity/`,
`resolution/`, `marts/`, `reverse/`, `delta/`) specifically so that
users can target any stage with a blanket override.

Example: switching identity and resolution stages to `table`
materialisation requires no changes to the generated `.sql` files:

```yaml
# dbt_project.yml — user edits after generation
models:
  osi_project:
    identity:
      +materialized: table       # was: view
    resolution:
      +materialized: table       # was: view
```

This works with any dbt-supported materialisation: `view`, `table`,
`incremental`, `ephemeral`, or any custom materialisation provided by
a dbt package.

### Example: pg_trickle stream tables

[pg_trickle](https://github.com/grove/pg-trickle) is a PostgreSQL 18
extension that provides incrementally maintained stream tables.  The
[dbt-pgtrickle](https://github.com/grove/pg-trickle/tree/main/dbt-pgtrickle)
package adds a `stream_table` materialisation to dbt.

Since the engine generates standard SQL models with `{{ ref() }}` and
`{{ source() }}` calls, a user can swap any model to `stream_table`
with zero changes to the generated SQL:

**Step 1:** Add dbt-pgtrickle to `packages.yml`:

```yaml
packages:
  - git: "https://github.com/grove/pg-trickle.git"
    revision: v0.7.0
    subdirectory: "dbt-pgtrickle"
```

**Step 2:** Override materialisation in `dbt_project.yml`:

```yaml
models:
  osi_project:
    staging:
      +materialized: view
    identity:
      +materialized: stream_table
      +schedule: '1m'
      +refresh_mode: DIFFERENTIAL
    resolution:
      +materialized: stream_table
      +schedule: null              # CALCULATED — refreshes after identity
    marts:
      +materialized: view
    reverse:
      +materialized: stream_table
      +schedule: null              # CALCULATED
    delta:
      +materialized: stream_table
      +schedule: null              # CALCULATED — refreshes after reverse
```

**Step 3:** `dbt deps && dbt run`

No engine changes, no special flags, no code generation differences.

### Why this works without engine support

The generated models are pure SQL `SELECT` statements with `{{ ref() }}`
dependencies.  dbt packages like dbt-pgtrickle operate at the DDL layer
— they control *how* the model is created (CREATE VIEW vs.
`pgtrickle.create_stream_table()`), not *what* SQL the model contains.
The SELECT body is identical regardless of materialisation.

This means the same generated dbt project is compatible with:

| Plugin | Materialisation | Use case |
|---|---|---|
| (none) | `view` | Default — zero overhead |
| (none) | `table` | Snapshot for heavy queries |
| (none) | `incremental` | Append-only delta consumers |
| dbt-pgtrickle | `stream_table` | Incremental view maintenance |
| Any future dbt package | Any custom materialisation | Vendor-specific optimisations |

### SQL compatibility with pg_trickle

For teams considering pg_trickle specifically, the OSI engine's SQL is
compatible with pg_trickle's DVM engine.  The key constructs:

| Engine SQL construct | pg_trickle DVM support |
|---|---|
| `WITH RECURSIVE` (identity) | Full + Differential + Immediate |
| `GROUP BY` + `array_agg`, `bool_or`, `FILTER` (resolution) | Group-rescan strategy |
| `DISTINCT ON` (resolution groups) | Auto-rewritten to ROW_NUMBER() |
| `LATERAL jsonb_array_elements` (nested arrays) | Row-scoped recomputation |
| `LEFT JOIN` (reverse, delta merge) | Full differential support |
| `UNION ALL` (identity base) | Full differential support |
| `CASE`, `COALESCE`, `IS NOT DISTINCT FROM` (delta) | Full |
| `md5()`, `jsonb_build_object()` (IMMUTABLE) | Full |

The `_osi_text_norm()` PL/pgSQL function is declared `IMMUTABLE`, which
pg_trickle accepts in defining queries.

### Which stages benefit most

For documentation purposes, here is a cost-benefit guide teams can
reference when choosing which stages to materialise:

| Stage | Materialisation value | Why |
|---|---|---|
| Forward (`_fwd_*`) | Low | Simple projection; cheap to recompute |
| Identity (`_id_*`) | **High** | Recursive CTE; most expensive operation |
| Resolution (`_resolved_*`) | **High** | Multi-source aggregation |
| Analytics (`{target}`) | Low | Trivial projection from resolved |
| Reverse (`_rev_*`) | Medium | LEFT JOIN; benefits if delta is materialised |
| Delta (`_delta_*`) | **High** | ETL consumers read this; always-current is valuable |

This guidance belongs in the generated `README.md` or as comments in
`dbt_project.yml`, not encoded in the engine.

---

## Appendix B: Could the engine be a pure dbt package?

### The question

Instead of a Rust binary that generates SQL, could the entire mapping
compiler live inside dbt as a Jinja macro package?  The user would write
`mapping.yaml`, and a dbt package would read it, validate it, and generate
models — no external tooling required.

### What dbt's macro system can do

dbt macros are Jinja2 templates with access to:

- **`{{ var() }}`** — project variables (YAML values from `dbt_project.yml`)
- **`{% set %}`** — local variables, lists, dicts
- **`{% for %}`** — iteration over lists and dicts
- **`{% if %}`** — conditionals
- **`{% macro %}`** — reusable functions with parameters
- **`{{ ref() }}`** — model dependency tracking
- **`{{ source() }}`** — source declarations
- **`adapter.dispatch()`** — multi-adapter SQL dispatch
- **`run_query()`** — execute SQL during compilation
- **`log()`** — debug output
- **YAML parsing** — dbt natively reads `schema.yml` and `dbt_project.yml`
  but macros cannot read arbitrary YAML files at compile time

dbt also supports **pre-hook / post-hook / on-run-start / on-run-end**
for procedural DDL.

### What the engine does that Jinja can handle

| Engine feature | Jinja feasibility | Notes |
|---|---|---|
| Read mapping YAML | Partial | No native arbitrary-file YAML reader.  Would need to encode the mapping as `dbt_project.yml` vars or a `schema.yml` extension. |
| Field mapping → SELECT | Easy | `{% for field in fields %} "{{ field.source }}" AS "{{ field.target }}" {% endfor %}` |
| Strategy dispatch | Easy | `{% if strategy == 'coalesce' %} (array_agg(...)) {% elif ... %}` |
| Source/target iteration | Easy | `{% for mapping in mappings %}` |
| View naming | Easy | String concatenation |
| Basic type casting | Easy | `::{{ field.type }}` |
| DAG ordering | Free | dbt handles this via `{{ ref() }}` dependency tracking |
| `_base` JSONB construction | Medium | `jsonb_build_object()` call with field iteration |
| Forward filter / reverse_filter | Easy | `{% if filter %} WHERE {{ filter }} {% endif %}` |

### What Jinja cannot realistically handle

**1. YAML parsing of arbitrary files**

dbt macros cannot read `mapping.yaml` from disk.  The mapping would
need to be encoded as dbt variables in `dbt_project.yml`:

```yaml
# dbt_project.yml
vars:
  osi_mapping:
    sources:
      crm: { primary_key: id }
    targets:
      company:
        fields:
          email: { strategy: identity }
    mappings:
      - name: crm_companies
        source: { dataset: crm }
        target: company
        fields:
          - { source: email, target: email }
```

This duplicates the spec inside dbt's own YAML structure.  Users lose
standalone `mapping.yaml` files, JSON Schema validation, and
editor-level autocompletion against our schema.

**2. 11-pass semantic validation**

The engine runs 11 sequential validation passes with cross-referencing:
strategy consistency checks (does coalesce have priorities? does
last_modified have timestamps?), field coverage analysis (are all
target fields mapped?), expression safety checking (balanced parens,
prohibited SQL keywords, internal view reference blocking), column
reference validation, parent chain resolution, and test data PK
verification.

Jinja has no exception model, no structured error accumulation, no
regex support, and limited string introspection.  Trying to validate
`reverse_filter: "name IS NOT NULL OR org_number IS NOT NULL"` for
balanced parentheses and prohibited keywords in Jinja is effectively
impossible without `run_query()` hacks.

The best approximation: skip validation entirely and let PostgreSQL
catch errors at runtime.  This is a significant regression in user
experience — the Rust engine catches 30+ categories of errors before
any SQL is executed.

**3. Expression safety enforcement**

The engine's `validate_expr.rs` strips string literals, checks for 24
prohibited SQL keywords (SELECT, INSERT, DROP...), blocks internal view
references (`_fwd_`, `_id_`, etc.), verifies balanced parentheses and
quotes, and extracts identifiers for column reference checking.  This
is effectively a mini SQL parser.

Jinja has no regex engine, no character-level string parsing, and no
backtracking.  This entire subsystem would have to be dropped.

**4. Recursive CTE generation for identity resolution**

The identity view builds a `WITH RECURSIVE` CTE that:
- UNIONs all forward views for a target
- Computes entity IDs via `md5(_mapping || ':' || _src_id)`
- Constructs matching conditions from identity fields, cluster IDs,
  and pairwise link edges
- Runs transitive closure to find connected components
- Selects the MIN component as canonical entity ID

The matching condition generation is the hard part.  For each pair of
identity fields across all contributing mappings, the engine builds
`n._field IS NOT NULL AND n._field = n2._field` clauses, groups them
by `link_group`, and combines with cluster ID matching and link edge
conditions.  The logic varies based on whether mappings have
`cluster_field`, `cluster_members`, `links`, or `link_key`.

This is ~300 lines of Rust with multiple levels of conditional logic.
In Jinja it would be a deeply nested macro with 10+ conditional
branches, generating a SQL string character by character.  Possible
but essentially unmaintainable.

**5. Nested JSONB reconstruction in delta views**

The delta module builds a `JsonNode` tree from dotted source paths
(`metadata.tags[0].value`) and emits `jsonb_build_object()` /
`jsonb_build_array()` calls that reconstruct the original source
JSONB structure from denormalised target columns.  This requires:

- Path parsing (`metadata.tags[0].value` →
  `[Key("metadata"), Key("tags"), Index(0), Key("value")]`)
- Tree insertion (recursive key-by-key traversal)
- Tree-to-SQL serialisation (recursive `jsonb_build_object` nesting)
- Array gap handling (sparse indices → `jsonb_build_array`)

Jinja has no recursive data structure support.  It can iterate over
flat lists but cannot build or traverse trees.  This would require
flattening the reconstruction logic into sequential string operations,
which is fragile and hard to test.

**6. Multi-pass parent chain resolution**

Nested array mappings use `parent:` references that can chain
(grandchild → child → parent).  The parser resolves these iteratively,
inheriting `source.dataset`, building compound `source.path`, and
promoting `parent_fields`.  Each pass resolves one level; the loop
continues until all are resolved.

Jinja's `{% for %}` cannot modify the collection it's iterating over.
Fixed-point iteration requires workarounds (recursive macros with
depth limits) that are brittle and hard to debug.

**7. Composite primary key handling**

Composite keys use `jsonb_build_object('col_a', col_a, 'col_b', col_b)`
for deterministic serialisation and `((_src_id::jsonb)->>'col_a')` for
reverse extraction.  The logic varies by key type (single vs. composite)
across forward, identity, reverse, and delta views.  Each view needs
type-aware casting based on `source.fields.type` declarations.

In Jinja this is possible but verbose — every PK appearance in every
view template needs a `{% if pk_type == 'composite' %}` branch.  With
~6 views per target and ~4 PK reference sites per view, this becomes
a maintenance burden.

### What a "dbt-only" architecture would look like

If we accepted the limitations above, the closest viable approach:

```
User writes:
  dbt_project.yml         (mapping encoded as vars)

dbt package provides:
  macros/
    osi_forward.sql       (generate_model macro per view type)
    osi_identity.sql
    osi_resolution.sql
    osi_reverse.sql
    osi_delta.sql
    osi_analytics.sql
  
  models/
    (empty — user generates models via dbt run-operation osi_generate)
```

The user would run `dbt run-operation osi_generate` to create model
files, then `dbt run`.  But this means the dbt package is a code
generator running inside dbt — essentially rebuilding the Rust engine
in Jinja, with worse tooling, no type safety, and no validation.

### Quantitative comparison

| Dimension | Rust engine | Hypothetical dbt package |
|---|---|---|
| Source lines | ~5,700 Rust | ~3,000-4,000 Jinja (estimate) |
| Validation passes | 11 | 0-2 (basic structural only) |
| Expression safety | Full (24 keyword rules, balanced checks, column refs) | None |
| Error messages | 30+ categories with file/line context | PostgreSQL runtime errors |
| Test infrastructure | 58 unit + 12 integration tests | dbt test (schema-level only) |
| Mapping format | Standalone `mapping.yaml` + JSON Schema | Embedded in `dbt_project.yml` vars |
| IDE support | JSON Schema → autocompletion, validation | None (opaque vars block) |
| Debugging | `osi-engine render --annotate`, `osi-engine dot` | dbt compile + manual inspection |
| Nested arrays | Full (arbitrary depth, sparse indices, type-aware) | Flat only (1 level, no reconstruction) |
| Primary keys | Single + composite with type casting | Single only (composite too complex) |
| Build time | <100ms | dbt compile overhead (~2-5s) |

### Recommendation: do not reimplement as a dbt package

The OSI mapping engine is a **domain-specific compiler**, not a SQL
templating tool.  Its value comes from:

1. **Validation** — catching errors before SQL hits the database
2. **Expression safety** — preventing SQL injection in user expressions
3. **Correctness** — type-aware PK handling, nested JSONB reconstruction,
   transitive closure connectivity
4. **Composability** — the `ViewOutput` refactor (Phase 1 of this plan)
   makes the engine a backend-agnostic compiler that can target dbt,
   raw SQL, or any future output format

dbt is the right **deployment vehicle** — the engine compiles to dbt
models, and dbt handles materialisation, scheduling, and lineage.
But dbt's Jinja macro system is the wrong **implementation language**
for a 5,700-line compiler with 11 validation passes, expression
parsing, tree construction, and graph algorithms.

The hybrid approach (Rust compiler → dbt project output) gives the best
of both: compile-time safety from the engine, runtime flexibility from
dbt.

### When to reconsider

This recommendation should be revisited if:

- **dbt gains a Python model API** — dbt already has experimental Python
  model support.  If this matures to support general-purpose compilation
  (not just pandas/Spark transforms), a Python reimplementation of the
  engine could run inside dbt directly.
- **The mapping spec shrinks dramatically** — if the spec drops nested
  arrays, composite keys, expression safety, and multi-pass validation,
  the remaining logic might fit in Jinja.  But that would remove the
  features that differentiate the tool.
- **dbt packages gain pre-compile hooks** — if dbt supported running
  an external binary before compilation (similar to `on-run-start` but
  for code generation), the engine could remain Rust while appearing as
  a native dbt package.
