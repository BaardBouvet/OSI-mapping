# Depth mismatch

Merging when one system has an extra intermediate level: 2-level vs 3-level nesting.

## Scenario

Two product systems track the same features at different structural depths.
System A has a flat `product → features` structure. System B adds a `module` grouping between product and features, giving `product → modules → features`.
Features merge across systems via identity despite living at different nesting depths. The intermediate module level exists only in System B.

## Key features

- **`parent:` chaining** — `b_features` parents on `b_modules` which parents on `b_products`, building `source.path: modules.features`
- **Qualified `parent_fields:`** — `b_features` uses `{ path: modules, field: product_name }` to reach back to the root table through the intermediate level
- **`strategy: identity`** — `feature_name` matches features across systems regardless of nesting depth
- **`priority: 1` / `priority: 2`** — overlapping fields (`status`, `effort`) propagate bidirectionally
- **Optional reference** — `module_name` on feature is populated from System B, NULL from System A

## How it works

1. System A contributes products (with `product_owner`) and features (with `status` priority 1, `effort` priority 2)
2. System B contributes products (with `roadmap_url`), modules (with `module_lead`), and features (with `status` priority 2, `effort` priority 1, `priority`)
3. Features merge by `feature_name` — System A's `status` wins and propagates to System B; System B's `effort` wins and propagates to System A
4. Module info from System B cannot flow to System A (no module concept) — this is correct behavior
5. New features from System B's extra modules DO flow to System A as flat entries without module grouping

## When to use

When integrating systems where one has an intermediate grouping level the other lacks. The shared entities merge regardless of depth, and each system reconstructs output at its own nesting depth.
