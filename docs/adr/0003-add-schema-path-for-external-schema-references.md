# ADR 0003: Add `schema_path` for external schema references

- Status: Accepted
- Date: 2026-03-07

## Context

The mapping spec supports external schema files (OpenAPI, JSON Schema, etc.) as sources via `schema_file`. However, a single external schema file can define many entities — an OpenAPI spec might have dozens of component schemas and endpoint responses.

Pointing at the whole file is ambiguous: the mapping consumer has no way to know which specific schema inside the file is being mapped.

## Problem

Without a mechanism to address a specific schema within a file:

- An OpenAPI spec with `components.schemas.Company`, `components.schemas.Contact`, and `components.schemas.Address` cannot be disambiguated.
- A JSON Schema with multiple `$defs` has the same problem.
- Tooling would need out-of-band conventions or heuristics to resolve the target schema.

## Decision

Add `schema_path` to `model_ref`:

- Type: `string`
- Semantics: JSON Pointer (RFC 6901) into the `schema_file`
- Examples:
  - `#/components/schemas/Company` — OpenAPI component schema
  - `#/$defs/Order` — JSON Schema definition
  - `#/paths/~1companies~1{id}/get/responses/200/content/application~1json/schema` — specific endpoint response

JSON Pointer was chosen because:

1. It is already the referencing standard used internally by OpenAPI (`$ref`) and JSON Schema (`$ref`).
2. It is an IETF standard (RFC 6901), not a custom invention.
3. It can address any node in a JSON/YAML document.

## Consequences

### Positive

- External schema references are now precise and unambiguous.
- Reuses an existing standard — no new syntax to learn.
- Works uniformly across OpenAPI, JSON Schema, and any JSON/YAML-based schema format.

### Negative

- JSON Pointer syntax for paths with `/` or `~` requires escaping (`~1` for `/`, `~0` for `~`), which can be hard to read for deeply nested references.
- `schema_path` is only meaningful when `schema_file` is set; the relationship is implicit rather than enforced in the JSON Schema (could be added via `if/then`).

## Alternatives considered

### Use a separate `component_name` field

Rejected — too OpenAPI-specific. Wouldn't work for JSON Schema `$defs` or other formats.

### Use a dot-notation path (e.g. `components.schemas.Company`)

Rejected — not a standard; ambiguous when keys contain dots.

### Require one schema file per mapped entity

Rejected — forces users to split their OpenAPI/JSON Schema files, creating unnecessary duplication.
