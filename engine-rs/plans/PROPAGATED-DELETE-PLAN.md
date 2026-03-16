# PROPAGATED-DELETE-PLAN

Support GDPR-style deletion propagation using regular target fields and
existing reverse mechanics — no special columns, no new engine concepts.

## Problem

Current delete detection is **local** — a delta `'delete'` means "this source
row fails `reverse_required` or `reverse_filter`." It's about whether a source
qualifies for reverse sync, not about propagating a deletion request.

GDPR and similar regulations require **cross-system deletion**: when a customer
requests deletion in System A, all systems (B, C, ...) that have data for the
same resolved entity must be notified to delete their records.

## Design: no magic needed

A soft-delete marker is just a **regular target field**. The deletion signal
propagates through the same pipeline as every other field:

1. CRM maps its `deleted_at` column to a shared target field `is_deleted`
2. The target strategy (`bool_or`) resolves it — true if ANY source says deleted
3. Reverse views push the resolved `is_deleted` back to every system
4. Each mapping's `reverse_filter` decides what to do with it

No new properties, no `_deleted` flag, no special delta logic.

### Mapping

```yaml
version: "1.0"
description: >
  GDPR deletion propagation. CRM soft-deletes are mapped to a regular
  target field. Each system's reverse_filter controls its response.

sources:
  crm:
    primary_key: id
  erp:
    primary_key: cust_id

targets:
  customer:
    fields:
      email:
        strategy: identity
      name:
        strategy: coalesce
      tier:
        strategy: coalesce
      is_deleted:
        strategy: bool_or        # true if ANY source marks deleted

mappings:
  - name: crm_customers
    source: { dataset: crm }
    target: customer
    fields:
      - source: email
        target: email
      - source: name
        target: name
      - expression: "deleted_at IS NOT NULL"
        target: is_deleted

  - name: erp_customers
    source: { dataset: erp }
    target: customer
    reverse_filter: "is_deleted IS NOT TRUE"   # ← delete when flagged
    fields:
      - source: email
        target: email
      - source: tier
        target: tier
```

### How it flows

```
CRM (deleted_at = '2026-03-15')          ERP (active record)
         │                                      │
    _fwd_crm_customers                    _fwd_erp_customers
    is_deleted = 'true'                   (no is_deleted)
    email = "alice@..."                   email = "alice@..."
         │                                      │
         └──────────── identity ────────────────┘
                          │
                  _resolved_customer
                  is_deleted = 'true'  (bool_or from CRM)
                          │
              ┌───────────┴───────────┐
              │                       │
      _rev_crm_customers       _rev_erp_customers
      is_deleted = 'true'      is_deleted = 'true'
              │                       │
       _delta_crm               _delta_erp
       action = 'noop'          action = 'delete'
                                (reverse_filter fails!)
```

ERP's `reverse_filter: "is_deleted IS NOT TRUE"` evaluates to false →
existing delta logic emits `'delete'`. No new engine code required.

CRM doesn't have a `reverse_filter` on its mapping, so it gets `'noop'`
(no data changed from CRM's perspective — it already has the deletion marker).

### Why this is better than `delete_when`

| | `delete_when` (rejected) | Regular field + `reverse_filter` |
|---|---|---|
| New engine concept | Yes — `_deleted`, `_delete_requested`, special delta branches | None |
| Per-system control | No — all systems get same delete signal | Yes — each mapping decides via `reverse_filter` |
| Policy flexibility | Needs future `delete_policy` property | Already exists — just choose strategy and filter |
| NULLing non-identity fields | Special forward-view logic | Not needed — coalesce naturally uses other sources |
| Deletion suppression | Special `_src_id IS NULL AND _delete_requested` branch | Natural — `reverse_filter` handles it |
| Consistency | Parallel mechanism alongside `reverse_filter` | Uses the same mechanism |

### Per-system control

Each system independently decides its response to the deletion signal:

**ERP** — must comply with GDPR, deletes the record:
```yaml
  - name: erp_customers
    reverse_filter: "is_deleted IS NOT TRUE"
```

**Billing** — must retain for legal reasons, ignores the signal:
```yaml
  - name: billing_customers
    # no reverse_filter on is_deleted — record is kept
```

**Archive** — soft-deletes by writing the flag back:
```yaml
  - name: archive_customers
    # maps is_deleted back to its own column
    fields:
      - source: is_archived
        target: is_deleted
```

No `delete_policy` needed — the policy is expressed in each mapping's
`reverse_filter` and field choices.

### Identity fields are naturally preserved

The deleted CRM row still emits `email = "alice@..."` in the forward view
because it's a regular field mapping. Identity resolution links it to the
ERP record normally. The `is_deleted` flag rides along as just another
resolved field.

If the source physically deletes the row (hard delete), the forward view
no longer emits it. Identity resolution may still link the ERP record to
an entity, but the `is_deleted` field won't have a value from CRM.
This is correct — a hard-deleted row that's gone from the source can't
contribute any fields, including deletion markers.

## Example

**CRM** — has a `deleted_at` timestamp for GDPR compliance:
```
┌────────────────────────────────┐
│ crm                            │
│  id: "C1"                      │
│  email: "alice@example.com"    │
│  name: "Alice"                 │
│  deleted_at: "2026-03-15"      │
│                                │
│  id: "C2"                      │
│  email: "bob@example.com"      │
│  name: "Bob"                   │
│  deleted_at: null              │
└────────────────────────────────┘
```

**ERP** — no deletion concept, all records active:
```
┌────────────────────────────────┐
│ erp                            │
│  cust_id: "E1"                 │
│  email: "alice@example.com"    │
│  tier: "gold"                  │
│                                │
│  cust_id: "E2"                 │
│  email: "bob@example.com"      │
│  tier: "silver"                │
└────────────────────────────────┘
```

### Test 1: Deletion propagates from CRM to ERP

**Input:** CRM has Alice (deleted) and Bob (active). ERP has both active.

**Expected:**
- Resolved customer "alice@example.com": `is_deleted = 'true'`
- Resolved customer "bob@example.com": normal (name from CRM, tier from ERP)
- CRM delta: Alice = noop, Bob = noop
- ERP delta: Alice = **delete** (`reverse_filter` fails), Bob = noop

### Test 2: No deletion — everything is noop

**Input:** CRM has Alice and Bob both active. ERP has both.

**Expected:**
- Both resolved normally, `is_deleted = 'false'`
- All deltas are noop

## Implementation

The `bool_or` resolution strategy was added to the engine to make this pattern
ergonomic. It generates `bool_or((F)::boolean)` in the resolution view.

Beyond that, **no further engine changes required.** The existing pipeline
handles this:

1. `expression: "deleted_at IS NOT NULL"` — already supported on field mappings
2. `strategy: bool_or` — new; resolves to `true` if any source is `true`
3. `reverse_filter` — already triggers delta `'delete'` when condition fails
4. Identity resolution — already links records across sources

This is purely a **mapping pattern**, not an engine feature.

### Optional: example

Create `examples/propagated-delete/` with the mapping above and test data
demonstrating the propagation. No Rust code changes needed.

## Interaction with existing delete mechanisms

| Mechanism | Triggers | Scope | Example |
|-----------|----------|-------|---------|
| `reverse_required` | Field is NULL in reverse | Local mapping | Customer without email → don't push to ERP |
| `reverse_filter` | SQL condition fails | Local mapping | `is_deleted IS NOT TRUE` → delete from ERP |
| `is_deleted` field | Mapped like any regular field | Cross-system (via resolution) | CRM soft-delete → resolved entity has `is_deleted = true` |

All three compose naturally. `reverse_required` and `reverse_filter` are the
mechanism; `is_deleted` as a target field is the data that drives the filter.

## Cascading to children

- **Embedded / nested array children** — the parent's delta is `'delete'`, so
  children naturally get deleted (parent row disappears from reverse).
- **Referenced entities** — not affected. Deleting a customer doesn't delete
  their orders. If cascade is needed, the order target needs its own
  `is_deleted` field fed by a join or SQL expression.

## Edge cases

**What if the deletion source is lower priority?** With `bool_or`, priority
doesn't matter — any source saying `true` wins. This is the GDPR-safe default.

If the desired behavior is "only the authoritative source can trigger deletion,"
use `coalesce` instead of `bool_or` — the highest-priority source's value wins.

**What about NULLing data fields?** The original `delete_when` plan NULLed
non-identity fields to prevent stale data from participating in coalesce.
With the regular-field approach, this isn't needed — the mapping author
controls which fields are mapped. If CRM maps both `name` and `is_deleted`,
the name will still resolve even when deleted. If that's undesirable, the
CRM mapping can use `expression: "CASE WHEN deleted_at IS NOT NULL THEN NULL ELSE name END"`
for the name field. This is explicit, not magic.

## Risk

**Minimal.** The only engine change is adding the `bool_or` strategy variant.
All other mechanics use existing features. This is a documentation / example
pattern showing how existing features compose to solve GDPR deletion propagation.
