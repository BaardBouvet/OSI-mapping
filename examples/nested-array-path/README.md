# Nested array path

Extract child entities from a JSONB array nested inside another array via
`array_path`.

## Scenario

A shop stores product measurements inside a nested path: each product has a
`specs` array containing a single element with a `measurements` array inside.
A warehouse stores the same measurements as a flat top-level array.
Both map to the same `product_measurement` child target.

## Key features

- **`array_path: specs.measurements`** — navigates two nesting levels with a
  single dotted path, expanding both as LATERAL joins
- **Qualified `parent_fields`** — `{ path: specs, field: sku }` reaches back
  to the root table level past the intermediate nesting
- Cross-source merge between deeply nested and flat array structures

## How it works

1. The forward view generates two LATERAL joins:
   `jsonb_array_elements("specs") AS _nest_0` then
   `jsonb_array_elements(_nest_0.value->'measurements') AS item`
2. Each measurement element is flattened into a row with the parent product's
   SKU pulled from the root table via qualified parent_fields
3. The warehouse mapping uses a simple `array: measurements` for its flat array
4. Identity resolution matches measurements by `(product_sku, measurement_type)`
5. The warehouse delta reconstructs its `measurements` array; the shop delta
   reconstructs the full `specs[].measurements[]` path

## When to use

- Source has arrays nested inside arrays (e.g., spec containers → measurements,
  result wrappers → data items)
- You need to flatten multi-level JSON nesting into a single child target
- Best suited when each intermediate level has a single element; multiple
  intermediate elements collapse into one group in the reverse path
