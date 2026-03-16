# Null Propagation

Propagate intentional NULLs through coalesce resolution using sentinel values — no engine changes needed.

## Problem

Coalesce resolution skips NULLs: `FILTER (WHERE field IS NOT NULL)`. When the authoritative source (CRM) clears a phone number, the stale value from a lower-priority source (ERP) wins instead. The golden record shows a phone that CRM explicitly removed.

## Solution: sentinel pattern

Use `expression` to convert authoritative NULLs to a non-NULL sentinel (`'__CLEARED__'`) in the forward direction, so the sentinel survives coalesce filtering. Use `reverse_expression` on **every mapping** that reads the field to convert the sentinel back to NULL.

```yaml
# Authoritative source — sentinel on forward
- source: phone
  target: phone
  priority: 1
  expression: "COALESCE(phone, '__CLEARED__')"
  reverse_expression: "NULLIF(phone, '__CLEARED__')"

# Non-authoritative source — just strip sentinel on reverse
- source: phone
  target: phone
  priority: 2
  reverse_expression: "NULLIF(phone, '__CLEARED__')"
```

## How it works

1. CRM has `phone = NULL` → forward expression produces `'__CLEARED__'`
2. `'__CLEARED__'` is non-NULL → survives `FILTER (WHERE phone IS NOT NULL)` in coalesce
3. CRM has priority 1 → `'__CLEARED__'` wins resolution
4. Reverse view: `NULLIF(phone, '__CLEARED__')` converts sentinel back to NULL
5. ERP's delta: `_base->>'phone' = '555-1234'` vs resolved `NULL` → update (clear phone)

## Trade-offs

| Pro | Con |
|-----|-----|
| No engine changes | Sentinel must be on every mapping that reads the field |
| Works today with existing features | `'__CLEARED__'` leaks into analytics view |
| Per-field, per-source control | Fragile if sentinel collides with real data |

The analytics view (`contact`) shows `'__CLEARED__'` instead of NULL. Consumers need to know to treat it as NULL, or a `default_expression` on the target field could convert it:

```yaml
phone:
  strategy: coalesce
  default_expression: "NULLIF(phone, '__CLEARED__')"
```

## When to use

- The authoritative source's NULLs are always intentional (not "unknown")
- Acceptable to coordinate the sentinel across all mappings for this field

## See also

- [NULL-WINS-PLAN](../../engine-rs/plans/NULL-WINS-PLAN.md) — engine-level solution with `null_wins` property (eliminates sentinel workaround)
- [propagated-delete](../propagated-delete/) — similar pattern using `bool_or` + `reverse_filter` for deletion signals
