# Passthrough fields

Carry source columns through the pipeline to delta output without mapping
them to target fields.

## Scenario

A CRM system requires `record_type` and `region_code` on every API write
call, but these fields are source-internal — they don't belong in the
golden record's target model. The ETL needs them in the delta output to
construct complete write operations.

## Key features

- **`passthrough: [record_type, region_code]`** — declares source columns
  that bypass resolution and appear directly in the delta output
- **Noop-safe** — changes to passthrough fields alone don't trigger updates;
  only mapped field changes cause the delta action to become `'update'`

## How it works

1. **Forward view** — passthrough columns are included in `_base` (JSONB
   snapshot) alongside mapped source fields.
2. **Identity + resolution** — passthrough columns do not participate;
   they are invisible to the golden record.
3. **Reverse view** — passthrough columns are extracted from `_base` JSONB
   and emitted as named columns.
4. **Delta view** — passthrough columns appear in the output but are
   excluded from the noop comparison.

## When to use

- The target system's API requires context fields not in the target model.
- Delta consumers need source metadata for routing, audit, or tracing.
- You want self-contained change feeds without polluting the golden record.
