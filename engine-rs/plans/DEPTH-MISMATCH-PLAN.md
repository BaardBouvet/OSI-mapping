# DEPTH-MISMATCH EXAMPLE PLAN

Demonstrate merging when one system has a deeper level than the other: System A
has Product → Feature (2 levels), System B has Product → Module → Feature
(3 levels). The extra depth is a **descendant** — an intermediate grouping layer
that one system doesn't have.

## How this differs from hierarchy-merge

| Aspect | Hierarchy-merge (extra ancestor) | Depth-mismatch (extra descendant) |
|--------|----------------------------------|-----------------------------------|
| **Missing level** | Ancestor (program above projects) | Intermediate (module between product and feature) |
| **Shared entities** | Projects + tasks exist at same depth in both | Features exist at different depths (1-deep vs 2-deep) |
| **Merge challenge** | Easy — shared entities match naturally | Hard — same entity sits at depth 1 in A, depth 2 in B |
| **Data loss risk** | None — ancestor is just missing from one system | Module grouping lost when flowing to System A |
| **Reverse flow** | Straightforward — each system reconstructs its own depth | System A can't reconstruct the intermediate modules |

## Scenario

**System A — Simple product tracker** (2 levels):
Product with a flat `features` array. No module concept.
```
┌───────────────────────────────────┐
│ system_a                          │
│  id: "A1"                         │
│  product_name: "Acme Platform"    │
│  product_owner: "Alice"           │
│  features: [                      │
│    { name: "SSO", status: "done", │
│      effort: 5 },                 │
│    { name: "RBAC",                │
│      status: "active", effort: 8} │
│  ]                                │
└───────────────────────────────────┘
```

**System B — Modular product manager** (3 levels):
Product with nested `modules`, each containing nested `features`.
```
┌─────────────────────────────────────────┐
│ system_b                                │
│  id: "B1"                               │
│  product_name: "Acme Platform"          │
│  roadmap_url: "https://..."             │
│  modules: [                             │
│    { name: "Auth",                      │
│      module_lead: "Bob",                │
│      features: [                        │
│        { name: "SSO",  priority: 1 },   │
│        { name: "RBAC", priority: 2 }    │
│      ]                                  │
│    },                                   │
│    { name: "Billing",                   │
│      module_lead: "Carol",              │
│      features: [                        │
│        { name: "Invoicing",             │
│          priority: 1 }                  │
│      ]                                  │
│    }                                    │
│  ]                                      │
└─────────────────────────────────────────┘
```

## Target model

```
product                module                 feature
───────                ──────                 ───────
product_name (id)      module_name (id)       feature_name (id)
product_owner (coal)   product_name (id)      product_name (id)
roadmap_url (coal)     module_lead (coal)     module_name (ref, nullable)
                                              status (coal)
                                              effort (coal)
                                              priority (coal)
```

**Key design decisions:**

1. **Module is a target** even though System A has no modules. Only System B
   contributes module entities. System A features get `module_name = NULL`.

2. **Feature identity** is `(feature_name, product_name)` — scoped to product,
   NOT to module. This is critical: SSO is the same feature regardless of
   whether it came from System A (no module) or System B (Auth module).
   If identity included `module_name`, features from System A (where
   module_name is NULL) would never match features from System B.

3. **Feature has an optional `module_name` reference** — populated from System B,
   NULL from System A. After resolution, the coalesced feature has a module
   if System B provides it.

## The hard problems

### Problem 1: Identity at different structural depths

System A features are at depth 1 (product → features).
System B features are at depth 2 (product → modules → features).

Both map to the same `feature` target with the same identity fields. The engine
handles this naturally — nested depth doesn't affect identity resolution. Forward
views produce flat rows regardless of source nesting.

### Problem 2: Reverse flow — what does System A get back?

System A has no module concept. When the resolved feature has `module_name`
from System B, that information cannot flow back to System A because:
- System A's source has no `module_name` column in its features array
- The feature mapping from System A doesn't map `module_name`

**This is correct behavior** — reverse flow only updates fields the mapping
declared. System A's features get `status`, `effort` (its own fields) plus
any coalesced values from shared fields. Module info is silently omitted.

### Problem 3: Reverse flow — System B gets features it didn't have

If System A has feature "SAML" that System B doesn't have, the resolved feature
has `module_name = NULL` (only System A contributed, no module info).

When this flows back to System B via reverse:
- The feature exists in the resolved target
- System B's mapping expects features inside modules
- But this feature has no module — it can't be placed into any module array

**Options:**
- **Option A: Insert at product level** — System B gets a new feature without
  module assignment. Requires a structural change (features array on product).
- **Option B: Skip** — Features without module_name are excluded from System B's
  reverse. They exist in the resolved target but don't flow back.
- **Option C: Default module** — A `value_default` puts unassigned features into
  a "default" or "unassigned" module.

**Recommendation: Option B (skip via reverse_filter)** for the example. Use
`reverse_filter` or `reverse_required` on the module reference to only push
features back to System B that have a module assignment. Features unique to
System A simply don't exist in System B's view — which is realistic.

### Problem 4: Module entities from System B only

Modules only flow from System B. System A doesn't contribute to or receive
module data. This is straightforward — the module target has a single source
mapping, no merge needed.

## Mappings

### System A (2 levels)

```yaml
- name: a_products
  source: { dataset: system_a }
  target: product
  fields:
    - source: product_name
      target: product_name
    - source: product_owner
      target: product_owner

- name: a_features
  source:
    dataset: system_a
    path: features
    parent_fields:
      parent_product: product_name
  target: feature
  fields:
    - source: parent_product
      target: product_name
      references: a_products
    - source: name
      target: feature_name
    - source: status
      target: status
    - source: effort
      target: effort
```

### System B (3 levels)

```yaml
- name: b_products
  source: { dataset: system_b }
  target: product
  fields:
    - source: product_name
      target: product_name
    - source: roadmap_url
      target: roadmap_url

- name: b_modules
  source:
    dataset: system_b
    path: modules
    parent_fields:
      parent_product: product_name
  target: module
  fields:
    - source: parent_product
      target: product_name
      references: b_products
    - source: name
      target: module_name
    - source: module_lead
      target: module_lead

- name: b_features
  source:
    dataset: system_b
    path: modules.features
    parent_fields:
      parent_module: name
      parent_product:
        path: modules
        field: product_name    # qualified ref — goes up two levels
  target: feature
  fields:
    - source: parent_product
      target: product_name
      references: b_products
    - source: parent_module
      target: module_name
      references: b_modules
    - source: name
      target: feature_name
    - source: priority
      target: priority
```

Note: `b_features` needs `parent_product` from the grandparent level. This uses
the qualified `ParentFieldRef` form with `path: modules` to reach back to the
product-level field. (This might need verification against the current engine
implementation — the Qualified variant exists in model.rs but usage at 2-deep
nesting with cross-level references needs testing.)

## Test cases

### Test 1: Same features merge across depths — coalesce fields

**Input:**
- System A: "Acme Platform" with features SSO (done, 5h) and RBAC (active, 8h)
- System B: "Acme Platform" with Auth module containing SSO (priority 1) and
  RBAC (priority 2)

**Expected resolved state:**
- Product "Acme Platform": product_owner from A, roadmap_url from B
- Module "Auth": from B only (product_name = "Acme Platform")
- Feature "SSO": status=done + effort=5 from A, priority=1 + module=Auth from B
- Feature "RBAC": status=active + effort=8 from A, priority=2 + module=Auth from B

**Expected deltas:**
- System A: noop (its features already have status+effort, module doesn't flow back)
- System B: updates — features get status+effort from A written into nested arrays

### Test 2: System A has a feature System B doesn't

**Input:**
- System A: "Acme Platform" with feature "Audit Log" (active, 3h)
- System B: "Acme Platform" with Auth module, features SSO only

**Expected:**
- Feature "Audit Log" exists in resolved target with module_name=NULL
- System B delta: "Audit Log" does NOT appear in B's reverse (no module
  assignment → filtered by reverse_required/reverse_filter)
- System A delta: noop

## What this demonstrates

1. **Depth mismatch** — same entity (feature) at depth 1 in A, depth 2 in B.
2. **Intermediate level loss** — module info can't flow to System A (no concept).
3. **Cross-depth field coalesce** — status/effort from A + priority from B merge
   on the same feature regardless of nesting depth.
4. **Asymmetric reverse flow** — merged data flows back at each system's own
   nesting depth. Features without module assignment are excluded from B.
5. **Optional references** — `module_name` on feature is populated from B,
   NULL from A. After resolution, only B-sourced features have modules.

## Comparison to hierarchy-merge

| | Hierarchy-merge | Depth-mismatch |
|-|-----------------|----------------|
| Extra level | Ancestor (program) — above shared levels | Intermediate (module) — between shared levels |
| Merge difficulty | Easy — shared entities at same conceptual depth | Harder — shared entity at different structural depths |
| Data loss | None | Module grouping unknown to System A |
| Reverse challenge | None | Can't push intermediate grouping to simpler system |
| Identity | Natural — same depth | Must exclude intermediate from identity |
| New-entity flow | New programs don't affect shared levels | New features may lack intermediate grouping |

## Implementation

1. Create `examples/depth-mismatch/` directory
2. Write `mapping.yaml` using current syntax
3. Test cases with cross-depth merge and asymmetric reverse
4. README explaining the pattern and the hard decisions
5. May expose edge case with qualified `parent_fields` at 2-deep nesting
