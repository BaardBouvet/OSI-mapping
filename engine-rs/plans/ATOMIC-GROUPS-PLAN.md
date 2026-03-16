# Atomic resolution groups

**Status:** Done

## Problem

The `group` property on target fields declares that certain fields must resolve
as an **atomic unit** — all fields in the same group must come from the same
winning source. Today `group` is parsed into the model (`TargetFieldDef.group`)
but **never consumed** in the resolution view renderer.

### Example

```yaml
targets:
  customer:
    fields:
      name:
        strategy: last_modified
      street:
        strategy: last_modified
        group: address
      zip_code:
        strategy: last_modified
        group: address
```

**Intent:** `street` and `zip_code` must resolve from the same source row.
If source A has the newest `street` but source B has the newest `zip_code`,
the group semantics pick the source whose **group-representative timestamp**
wins — and take both fields from that source.

**Current behavior:** Each field resolves independently via its own
`array_agg(field ORDER BY ts DESC)[1]`. Street may come from source A and
zip_code from source B. The value-groups example passes because the expected
data was set to match the (broken) independent resolution — not the atomic
group resolution.

---

## Design

### Group-Aware Resolution

For each group, determine a single winning source row per entity, then take
all grouped fields from that row.

**Strategy: last_modified groups**

The winning row is the one with the best (most recent) timestamp across the
group's representative field. The representative is the field with the latest
per-field timestamp (`_ts_<field>`), falling back to `_last_modified`.

SQL approach — use a window function to rank rows within the group, then pick
rank=1:

```sql
-- For group "address" with fields street, zip_code (both last_modified):
(SELECT sub."street"
 FROM _id_customer sub
 WHERE sub._entity_id_resolved = _id_customer._entity_id_resolved
 ORDER BY GREATEST(
   COALESCE(sub."_ts_street", sub._last_modified),
   COALESCE(sub."_ts_zip_code", sub._last_modified)
 ) DESC NULLS LAST
 LIMIT 1) AS "street",

(SELECT sub."zip_code"
 FROM _id_customer sub
 WHERE sub._entity_id_resolved = _id_customer._entity_id_resolved
 ORDER BY GREATEST(
   COALESCE(sub."_ts_street", sub._last_modified),
   COALESCE(sub."_ts_zip_code", sub._last_modified)
 ) DESC NULLS LAST
 LIMIT 1) AS "zip_code"
```

All fields in the group use the same ORDER BY expression (GREATEST of all
group members' timestamps), guaranteeing they pick the same source row.

**Alternative: Lateral subquery (single pass)**

```sql
_grp_address AS (
  SELECT DISTINCT ON (_entity_id_resolved)
    _entity_id_resolved,
    "street",
    "zip_code"
  FROM _id_customer
  WHERE "street" IS NOT NULL OR "zip_code" IS NOT NULL
  ORDER BY _entity_id_resolved,
    GREATEST(
      COALESCE("_ts_street", _last_modified),
      COALESCE("_ts_zip_code", _last_modified)
    ) DESC NULLS LAST
)
```

Then in the main resolution query, LEFT JOIN `_grp_address` and reference
`_grp_address."street"` instead of the per-field aggregation.

**Recommended:** The lateral/CTE approach. It's a single pass per group, more
efficient than correlated subqueries, and cleanly separable from the
non-grouped field logic.

**Strategy: coalesce groups**

Same pattern but ORDER BY uses priority instead of timestamp:

```sql
ORDER BY _entity_id_resolved,
  LEAST(
    COALESCE("_priority_street", _priority, 999),
    COALESCE("_priority_zip_code", _priority, 999)
  ) ASC NULLS LAST
```

The winning row is the one with the best (lowest) priority across any group
member.

---

## Implementation

### Step 1: Collect Groups in Resolution Renderer

In `render_resolution_view()`, scan target fields for `group` values and
build a map: `group_name → Vec<(field_name, strategy)>`.

### Step 2: Emit Group CTEs

For each group, emit a CTE using `DISTINCT ON (_entity_id_resolved)`:

```rust
for (group_name, fields) in &groups {
    let order_expr = match group_strategy {
        Strategy::LastModified => {
            let parts: Vec<String> = fields.iter()
                .map(|(f, _)| format!("COALESCE({}, _last_modified)", qi(&format!("_ts_{f}"))))
                .collect();
            format!("GREATEST({}) DESC NULLS LAST", parts.join(", "))
        }
        Strategy::Coalesce => {
            let parts: Vec<String> = fields.iter()
                .map(|(f, _)| format!("COALESCE({}, _priority, 999)", qi(&format!("_priority_{f}"))))
                .collect();
            format!("LEAST({}) ASC NULLS LAST", parts.join(", "))
        }
        _ => panic!("group not supported for strategy {:?}", group_strategy),
    };
    // Emit CTE ...
}
```

### Step 3: Reference Group CTEs in Main Query

For grouped fields, replace the per-field aggregation with a reference to the
group CTE:

```sql
SELECT
  _entity_id_resolved AS _entity_id,
  min("customer_id") AS "customer_id",           -- identity (not grouped)
  _grp_address."street",                          -- from group CTE
  _grp_address."zip_code",                        -- from group CTE
  (array_agg(...) ...)[1] AS "name"              -- last_modified (not grouped)
FROM _id_customer
LEFT JOIN _grp_address USING (_entity_id_resolved)
GROUP BY _entity_id_resolved, _grp_address."street", _grp_address."zip_code"
```

Grouped fields must be added to the GROUP BY clause (since they come from a
joined CTE, not an aggregate).

### Step 4: Update value-groups Expected Data

Once atomic groups work, the expected data must reflect the grouped resolution.
With the test data:

- Row 1 (cid=1): street_updated=Jan 15, zip_updated=Jan 5
- Row 2 (cid=2): street_updated=Jan 1, zip_updated=Jan 10

Group winner = Row 1 (GREATEST(Jan 15, Jan 5) = Jan 15 > GREATEST(Jan 1, Jan 10) = Jan 10)

So both street and zip come from Row 1: street="123 Oak St", zip_code="90210".

Current expected (wrong):
```yaml
{ street: "123 Oak St", zip_code: "90211" }  # street from row 1, zip from row 2
```

Correct expected:
```yaml
{ street: "123 Oak St", zip_code: "90210" }  # both from row 1
```

### Step 5: Validation

Add a validation pass (or extend existing): warn/error if a `group` is used
with a strategy other than `coalesce` or `last_modified` (the only strategies
where atomic grouping makes semantic sense).

---

## Risks

1. **GROUP BY expansion**: Adding group CTE columns to GROUP BY changes the
   query semantics. Must ensure the LEFT JOIN is 1:1 (guaranteed by
   DISTINCT ON _entity_id_resolved).

2. **Mixed strategies in a group**: All fields in a group should use the same
   strategy. Mixing `coalesce` and `last_modified` in one group is ambiguous.
   Validation should reject this.

3. **NULL handling**: If the winning row has NULL for some group members, those
   stay NULL. The consumer sees atomically-consistent NULLs rather than
   values from a different source.
