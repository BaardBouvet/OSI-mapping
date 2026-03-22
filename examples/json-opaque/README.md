# JSON opaque

Whole JSON values mapped as atomic blobs — no sub-field extraction.

## Scenario

Two product catalog systems track tags (a JSON array) and attributes (a JSON
object) per product. Neither system needs the engine to inspect or transform
the JSON content — it flows through as an opaque value. When both catalogs
have data for the same product, coalesce priority picks one source's JSON
wholesale.

## Key features

- **`type: jsonb`** — declares the target field as JSONB so values are cast
  and compared correctly
- **No `source_path`** — the field maps directly, treating the entire JSON
  blob as one atomic value
- **`strategy: coalesce`** — higher-priority source's JSON wins entirely;
  there is no per-element merge

## How it works

1. Each source's `tags` and `attributes` columns are read as JSONB.
2. The forward view casts them to `jsonb` on the target field.
3. Coalesce resolution picks catalog A (priority 1) when both sources
   contribute.
4. The reverse delta writes the winning JSON back to catalog B unchanged.

## When to use

Use this pattern when a source field already contains the correct JSON
structure and you only need source-wins resolution — no transformation, no
per-element merge. Common cases: tag arrays, preference objects, opaque
metadata blobs, configuration JSON.

If you need per-field extraction from JSONB, see
[`json-fields`](../json-fields/README.md) instead.
