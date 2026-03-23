# CRDT ordering — linked list

Adjacency-pointer ordering via `order_prev` / `order_next` alongside ordinal `order: true`.

## Scenario

Two recipe databases contribute steps for the same recipes. Blog CMS owns step ordering (priority 1 on steps) while recipe DB owns recipe metadata and step durations (priority 1 on recipes). In addition to ordinal position (`order: true`), the mapping emits `prev` and `next` adjacency pointers using `order_prev` / `order_next`. These CRDT linked-list pointers let downstream consumers reconstruct sibling order without relying on a global sort key.

## Key features

- **`order_prev: true`** — auto-populates the `prev` field with a LAG window over identity fields
- **`order_next: true`** — auto-populates the `next` field with a LEAD window over identity fields
- **Composite identity in pointers** — when the target has multiple identity fields, prev/next emit a JSONB object of the neighbour's identity
- **Combined with `order: true`** — ordinal position and linked-list pointers coexist on the same mapping
- **Asymmetric step priorities** — blog CMS owns ordering (priority 1), recipe DB contributes data; duration values still flow from recipe DB via coalesce fallback

## How it works

1. Each mapping unpacks the `steps` array with `parent:` + `array:`.
2. `order: true` emits a zero-padded position key from the array index.
3. `order_prev: true` generates `LAG(jsonb_build_object(...))` over the identity fields (`recipe_name`, `instruction`), partitioned by the parent key.
4. `order_next: true` generates the corresponding `LEAD(...)` expression.
5. The identity view merges steps from both sources; `step_order` resolves via coalesce — blog CMS positions win (priority 1), giving a deterministic global order.
6. Reverse views reconstruct each source's array with the merged element set, ordered by the resolved `step_order`. Duration values from recipe DB are preserved via coalesce fallback.

## When to use

Use `order_prev` / `order_next` when downstream consumers need linked-list adjacency pointers (e.g., for CRDT-based reordering UIs) rather than — or in addition to — a simple ordinal sort key.
