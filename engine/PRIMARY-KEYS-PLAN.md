# Real Primary Keys — Design Plan

> **Abstract**: Replaces the synthetic `_row_id` with real source primary keys
> throughout the view pipeline. Introduces a `sources:` section in the mapping
> spec (with `primary_key`), a deterministic `_src_id` representation (TEXT for
> single PKs, JSONB-as-text for composite), and the engine changes needed to
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

## Composite PK Representation

`_src_id` is always `TEXT` (decided in ORIGIN-PLAN.md):

- **Single PK**: plain text cast — `person_id::text` → `'P4'`
- **Composite PK**: JSONB object cast to text — `jsonb_build_object('crm_id',
  crm_id, 'region', region)::text` → `'{"crm_id": "CRM1", "region": "EU"}'`

JSONB key ordering is alphabetical in PostgreSQL — deterministic across runs.
Since the engine generates all SQL, key order is controlled (always alphabetical).

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
