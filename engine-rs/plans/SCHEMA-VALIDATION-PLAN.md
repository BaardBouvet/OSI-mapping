# JSON Schema Validation (Pass 0)

**Status:** Done

## Problem

The validation pipeline is parse-then-validate: serde deserialization
fails fast on the first structural error, so users see one error at a
time. A typo in a field name stops all further validation — including
the 11 semantic passes that would report additional issues.

The JSON schema (`spec/mapping-schema.json`) already describes the full
structural contract, but it's only used for documentation. The Rust
model and the schema drifted apart (e.g. `written_noop` → `derive_noop`,
missing `derive_timestamps`). The `deny_unknown_fields` attribute on
serde catches unknown fields but still fails on the first one.

## Design

Add JSON schema validation as **Pass 0** — before serde deserialization.

### Pipeline

```
YAML text
  │
  ├─ 1. serde_yaml::from_str → serde_json::Value  (always succeeds if valid YAML)
  │
  ├─ 2. jsonschema::validate(value, schema)        (reports ALL structural errors)
  │     └─ errors collected into ValidationResult diagnostics
  │
  ├─ 3. serde_yaml::from_str → MappingDocument     (typed deserialization)
  │     └─ if this fails, report as single parse error and stop
  │
  └─ 4. Semantic passes 1–11                       (on parsed MappingDocument)
```

Schema errors are non-fatal for the serde parse step — we report them
all, then still attempt serde deserialization. If serde also fails,
that error is reported too. This way users see the complete picture.

### Schema embedding

The JSON schema is embedded at compile time via `include_str!`. The
schema file lives at `../spec/mapping-schema.json` relative to the
crate root. No runtime file loading.

### Dependency

Add `jsonschema` crate as a regular dependency. It supports JSON Schema
draft 2020-12 which our schema uses. The crate compiles the schema once
and validates against it — fast enough for CLI use.

### Integration points

1. **`validate::validate()`** — unchanged, still takes `&MappingDocument`.
2. **New `validate::validate_schema()`** — takes `&serde_json::Value`,
   returns `Vec<Diagnostic>` with schema-level errors.
3. **`main.rs` Validate command** — calls `validate_schema()` first on
   the raw YAML value, then attempts parse + `validate()`.
4. **Unit test** — `validate_schema_all_examples` ensures every example
   passes schema validation.

### What this replaces

Nothing. `deny_unknown_fields` on serde remains as a safety net. The
semantic passes remain unchanged. Schema validation is purely additive —
it gives better error messages and accumulates all structural issues.

### What this does NOT do

- Does not replace semantic passes 1–11 (cross-references, strategy
  compatibility, SQL safety, etc. are not expressible in JSON Schema).
- Does not add schema validation to the `render` command — only
  `validate`. Render still parses directly for speed.

## Changes

| File | Change |
|------|--------|
| `Cargo.toml` | Add `jsonschema` dependency |
| `src/validate.rs` | Add `validate_schema()` function |
| `src/main.rs` | Wire Pass 0 into Validate command |
| `tests/integration.rs` | Add schema validation test for all examples |
| `../spec/mapping-schema.json` | Already synced with Rust model |
