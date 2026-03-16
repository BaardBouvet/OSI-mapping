# Primary keys

**Status:** Done

> **Abstract**: Replaces the synthetic `_row_id` with real source primary keys
> throughout the view pipeline. Introduces a `sources:` section in the mapping
> spec (with `primary_key`), typed PK handling with deterministic
> canonicalization only where needed, and the engine changes needed to
> thread real PKs through forward, identity, resolution, reverse, and delta views.

## Problem

The engine currently injects a synthetic `_row_id SERIAL PRIMARY KEY` into every
source table (in the test harness) and threads it through the entire pipeline as
`_src_id`. This works, but:

1. **Deployment gap** — real source tables don't have `_row_id`; the deployer must
   add one or wrap the table.
2. **Semantic loss** — the spec's test data already carries meaningful PKs, but the
   engine ignores them. A survey of the 36 examples reveals **40+ distinct PK column
   names** across source datasets: `id`, `_id`, `db_id`, `cid`, `customer_id`,
   `person_id`, `contact_id`, `employee_id`, `order_id`, `billing_id`, `line_item_id`,
   `invoice_id`, `order_number`, `user_id`, etc.
3. **Composite keys** — some sources have multi-column PKs
   (e.g. `erp_order_lines` → `(order_id, line_no)`). Synthetic `_row_id` collapses
   this information.
4. **Delta quality** — the delta view's `FULL OUTER JOIN` on `_row_id = _src_id` only
   works because both sides come from the same physical table. With real PKs, the
   delta can join on business-meaningful keys, enabling true insert/delete detection
   across systems.

## PK Representation

`_src_id` is TEXT internally throughout the view pipeline. This is necessary
because identity views, resolution GROUP BY, and recursive CTEs require a
consistent scalar type. Primary keys retain their native types only at external
boundaries (link tables, cluster_members tables).

- **Single PK**: `column::text` in forward view → e.g. `'P4'`
- **Composite PK**: `jsonb_build_object(...)::text` with keys sorted
  alphabetically for determinism → e.g. `'{"crm_id":"CRM1","region":"EU"}'`
- **Single-element list normalization**: `primary_key: [id]` and
  `primary_key: id` produce identical SQL (both → `id::text`).

## Spec Extension

Add `sources:` section with `primary_key`:

```yaml
sources:
  crm_contacts:
    table: crm_contacts
    primary_key: [contact_id]

  erp_order_lines:
    table: erp_order_lines
    primary_key: [order_id, line_no]
```

Schema:

```json
"sources": {
  "type": "object",
  "additionalProperties": {
    "type": "object",
    "properties": {
      "table": {
        "type": "string",
        "description": "Physical table name. Defaults to the source key."
      },
      "primary_key": {
        "description": "Column(s) that uniquely identify a row.",
        "oneOf": [
          { "type": "string" },
          { "type": "array", "items": { "type": "string" }, "minItems": 1 }
        ]
      }
    },
    "required": ["primary_key"]
  }
}
```

**Backward compatibility**: When `sources:` is absent, the engine falls back to
`_row_id` (current behavior). No existing mapping files break.

## Engine Changes

### 1. Model (`model.rs`)

```rust
pub struct Source {
    pub table: Option<String>,
    pub primary_key: PrimaryKey,
}

pub enum PrimaryKey {
    Single(String),
    Composite(Vec<String>),
}

impl Source {
    pub fn pk_columns(&self) -> Vec<&str> { ... }
    pub fn table_name(&self, key: &str) -> &str {
        self.table.as_deref().unwrap_or(key)
    }
}
```

### 2. Forward View (`forward.rs`)

Replace the hardcoded `_row_id AS _src_id` with the declared PK:

```sql
-- Single PK:     contact_id::text AS _src_id
-- Composite PK:  jsonb_build_object('line_no', line_no, 'order_id', order_id)::text AS _src_id
```

### 3. Identity View (`identity.rs`)

No structural change — `_src_id` flows through via `SELECT *`. The recursive
CTE works on `_entity_id` (now `md5(_mapping || ':' || _src_id)`), independent
of the PK representation.

### 4. Resolution View (`resolution.rs`)

No change — groups by `_entity_id_resolved`.

### 5. Reverse View (`reverse.rs`)

Replace `id._src_id` with the original PK columns:

```sql
-- Single PK:
SELECT id._src_id AS contact_id, ...

-- Composite PK:
SELECT
  (id._src_id::jsonb->>'order_id') AS order_id,
  (id._src_id::jsonb->>'line_no') AS line_no,
  ...
```

The PK columns become the leading output columns of the reverse view, restoring
the source's natural key.

### 6. Delta View (`delta.rs`)

Join on real PK instead of `_row_id = _src_id`:

```sql
-- Single PK:
FROM source AS src
FULL OUTER JOIN _rev_{name} AS rev ON src.contact_id::text = rev._src_id

-- Composite PK:
FROM source AS src
FULL OUTER JOIN _rev_{name} AS rev
  ON rev._src_id = jsonb_build_object('line_no', src.line_no, 'order_id', src.order_id)::text
```

### 7. Test Harness (`integration.rs`)

- When `sources:` declares a PK: create table with the declared PK as
  `PRIMARY KEY`, no `_row_id` column.
- When `sources:` is absent: current behavior (`_row_id SERIAL PRIMARY KEY`).
- Reverse view comparison joins on declared PK columns, not `_row_id`.

### 8. Validator (`validate.rs`)

New validation checks:
- PK column(s) must exist in every test input row for that dataset.
- PK values must be unique within a test input dataset.
- PK column(s) should not also be mapped as target fields (warning, not error).

## Migration Path

1. **Phase A**: Add `sources:` to schema + model + parser. No engine behavior
   change yet. Validator checks PK consistency.
2. **Phase B**: Update forward/reverse/delta renderers to use declared PK when
   present, fall back to `_row_id` when absent.
3. **Phase C**: Update all 36 example mapping files to declare `sources:` with
   `primary_key`. Each source dataset needs its PK identified from the test
   input data.
4. **Phase D**: Update integration test harness. Remove `_row_id` injection for
   examples that declare PKs. Run all examples end-to-end.
5. **Phase E** (optional): Make `sources:` required in spec v1.1. Deprecate
   `_row_id` fallback.

## Impact on Examples

From the survey of all 36 examples:

| Dataset pattern | Typical PK | Composite? |
|-----------------|-----------|------------|
| `crm`, `erp`, `source_*`, `system_*` | `id` | No |
| `crm_contacts`, `erp_contacts` | `contact_id` | No |
| `crm_companies`, `erp_companies` | `company_id` | No |
| `erp_customer` | `_id` | No |
| `crm_company` | `db_id` | No |
| `customers` | `id`, `cid`, or `customer_id` | No |
| `erp_order_lines` | `[order_id, line_no]` | **Yes** |
| `warehouse_lines` | `line_id` | No |
| `*_linkage` tables | `[system_a_id, ...]` | **Yes** |
| vocabulary tables | `name` or `code` | No |

Approximately 3–4 datasets have composite PKs. The rest are single-column.

## Relationship to Other Plans

- **ORIGIN-PLAN.md**: `_src_id` representation and `links` mechanism depend on
  `sources:` being available. `links` references a source by name and joins on
  its declared PK.
- **Deterministic hashing**: `md5(_mapping || ':' || _src_id)` requires `_src_id`
  to be the real PK (not a synthetic row number) for the hash to be stable
  across table reloads.

## Analysis: Source Column Types

### Current State

There are three separate "type" concerns:

1. **Render pipeline** — completely type-agnostic. Forward/reverse/delta views
   reference column names as bare SQL identifiers. The only cast is
   `pk::text AS _src_id`. The engine emits column names; Postgres infers types
   from the underlying tables and expressions. This is correct and robust.

2. **Test harness** — now infers Postgres types from JSON values in `tests.input`
   via `infer_column_types()`. This determines `CREATE TABLE` column types when
   standing up test data against a real Postgres instance.

3. **Mapping expressions** — user-written SQL snippets like
   `TO_DATE(dob, 'DD/MM/YY')` or `REGEXP_REPLACE(phone, ...)` implicitly depend
   on the source column having a compatible type (usually TEXT for string
   functions, but DATE/NUMERIC for comparisons and arithmetic).

### Is Test-Based Inference Fragile?

The inference only affects test execution, not production rendering. Risks:

| Scenario | What happens | Severity |
|----------|-------------|----------|
| Column is all strings → `TEXT` | Correct and harmless | None |
| Column has `42` (JSON number) → `NUMERIC` | Correct — enables numeric ops | None |
| Column has `true` → `BOOLEAN` | Correct — enables bool logic | None |
| Row 1 has `"42"`, row 2 has `42` → `TEXT` fallback | Safe but loses type intent | Low |
| All rows have `null` → `TEXT` | Fine — NULL is untyped | None |
| Test author means DATE but writes `"2024-01-15"` → `TEXT` | Postgres string, so `TO_DATE()` still works on it | Low |

The main fragility: **JSON's type system is much coarser than SQL's.** JSON has no
date, timestamp, uuid, or decimal type — they're all strings. So the inference can
only distinguish TEXT/NUMERIC/BOOLEAN/JSONB, not the full Postgres type zoo.

### Would Explicit Source Column Types Help?

Three options:

#### Option A: Status quo (infer from test data)

```yaml
sources:
  crm:
    primary_key: contact_id
# No column types — inferred from tests, or "whatever Postgres gets"
```

**Pros**: Simple, zero config, works for current examples.
**Cons**: Can't express DATE, TIMESTAMP, UUID, INTEGER vs NUMERIC, etc.
JSON numbers become NUMERIC (arbitrary precision), not INTEGER.

#### Option B: Optional `columns` map on sources

```yaml
sources:
  system_a:
    primary_key: person_id
    columns:
      dob: TEXT           # input is "15/06/85", expression does TO_DATE()
      person_id: TEXT
      first_name: TEXT
      phone_number: TEXT
```

**Pros**: Explicit, self-documenting, enables precise types for production DDL,
can serve as a contract between source owner and mapping author.
**Cons**: Verbose — every column must be declared for it to be complete. Most
columns are TEXT anyway. The test harness can still override or supplement from
test data.

#### Option C: Optional `columns` map, only for non-TEXT columns

```yaml
sources:
  orders:
    primary_key: order_id
    columns:
      quantity: INTEGER
      unit_price: NUMERIC(10,2)
      is_active: BOOLEAN
      created_at: TIMESTAMPTZ
# Unlisted columns default to TEXT
```

**Pros**: Minimal config — you only declare deviations from TEXT. Self-documenting
for the columns that matter. Test harness uses these declarations, falling back to
inference for unlisted columns.
**Cons**: Adds spec complexity. Production deployments might have their own DDL
anyway, making this redundant outside tests.

### Recommendation

**Option C is the right direction** — but it solves a future problem, not a current
one. Right now:

- The render pipeline is already type-agnostic (correct).
- Test data is overwhelmingly string-typed (all YAML values are quoted).
- The few type-sensitive expressions (`TO_DATE`, `SPLIT_PART`, etc.) operate on
  TEXT inputs by design.
- The inference correctly falls back to TEXT for ambiguous cases.

**Suggested sequencing:**

1. **Now**: Keep inference as-is. It's safe for the existing test suite.
2. **When needed**: Add optional `columns:` to `Source` (Option C) when a concrete
   example requires non-TEXT source columns for correct SQL execution — e.g., if
   someone adds a test with `quantity: 5` (JSON number) and the mapping does
   arithmetic on it, the inference will produce NUMERIC, which is correct enough.
3. **For production DDL generation** (if ever): `columns:` becomes essential because
   real tables need precise types. But that's a different feature (DDL rendering),
   not a mapping concern.

### What If Tests Have Inconsistent Types?

The current `infer_column_types()` handles this: if a column has mixed non-null JSON
types across rows (e.g., `"42"` in one row, `42` in another), it falls back to TEXT.
This is safe — Postgres will accept any literal as TEXT.

The real guard rail is the validator (`validate.rs`), which could add a warning:
"column `quantity` in dataset `orders` has mixed JSON types across test rows". This
would catch accidental inconsistencies in test data authoring, without blocking
execution.
