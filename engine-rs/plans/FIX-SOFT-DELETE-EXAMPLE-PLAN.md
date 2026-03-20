# Fix soft-delete example

**Status:** Implementing

## Problem

The current test is misleading.  CRM Alice is soft-deleted with
`name: "Alice"`, ERP Alice has `name: "A. Smith"`.  Because CRM has
priority 1 for name, the resolved record picks "Alice" and pushes an
update to ERP:

```yaml
expected:
  erp:
    updates:
      - { cust_id: "E1", email: "alice@example.com", name: "Alice" }
```

This demonstrates a side effect: a soft-deleted source still participates
in forward/resolution views, so its data wins priority and flows to other
systems.  That's confusing to showcase as the feature — the point of
soft-delete is **suppression**, not resolution influence.

## Fix

Two tests that demonstrate the actual value proposition:

### Test 1 — Suppression + normal insert

CRM has Alice (soft-deleted) and Bob (active).  ERP has Alice.  Bob is
CRM-only, so he gets inserted into ERP.  Alice is suppressed in CRM's
delta (no stale write-back).  Both systems already agree on Alice's data,
so ERP Alice is noop.

```yaml
input:
  crm:
    - { id: "C1", email: "alice@example.com", name: "Alice", deleted_at: "2026-03-15" }
    - { id: "C2", email: "bob@example.com", name: "Bob", deleted_at: null }
  erp:
    - { cust_id: "E1", email: "alice@example.com", name: "Alice" }
expected:
  erp:
    inserts:
      - { _cluster_id: "crm_customers:C2", email: "bob@example.com", name: "Bob" }
```

This shows:
- Active records propagate normally (Bob → ERP insert).
- Soft-deleted Alice is suppressed in CRM's delta.
- ERP is unaffected for Alice (noop — both systems agree).
- Satisfies CONTRIBUTING.md: expected contains `inserts:`.

### Test 2 — All active, no suppression (baseline)

Same data but without soft-delete (deleted_at all null).  Both CRM
records propagate normally — Bob still gets inserted.  This proves the
feature flag is what controls suppression, not something else.

```yaml
input:
  crm:
    - { id: "C1", email: "alice@example.com", name: "Alice", deleted_at: null }
    - { id: "C2", email: "bob@example.com", name: "Bob", deleted_at: null }
  erp:
    - { cust_id: "E1", email: "alice@example.com", name: "Alice" }
expected:
  erp:
    inserts:
      - { _cluster_id: "crm_customers:C2", email: "bob@example.com", name: "Bob" }
```

Same expected output — the difference is that CRM Alice is NOT suppressed
here (she is noop because data matches).  The pair of tests shows that
suppression only matters when the source signals deletion.

## Changes

1. `examples/soft-delete/mapping.yaml` — replace tests section.
2. `examples/soft-delete/README.md` — update scenario description.
