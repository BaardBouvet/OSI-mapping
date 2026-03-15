# JSON Fields

Demonstrates `source_path` for extracting sub-fields from JSONB source columns.

## Scenario

A CRM system stores customer metadata as a single JSONB column:
```json
{"tier": "gold", "language": "en", "address": {"city": "Oslo"}}
```

An ERP system stores the same data in flat columns. Both map to a shared
`customer` target. The ERP has higher priority.

## Key Features

- **`source_path: metadata.tier`** — extracts a single JSON key
- **`source_path: metadata.address.city`** — deep path navigation
- **Automatic reverse reconstruction** — the delta rebuilds the `metadata`
  JSONB column from resolved sub-fields via `jsonb_build_object`
- **Per-sub-field noop detection** — each sub-field is compared individually
