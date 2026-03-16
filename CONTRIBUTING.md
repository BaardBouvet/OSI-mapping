# Contributing

Style and formatting rules for the OSI mapping repository.

## Specification files (`docs/`, `spec/`, `examples/`)

These define the mapping language itself. They must be **engine-agnostic**:

- Never reference `engine-rs/` or any specific implementation.
- Cross-reference other spec files and examples freely.
- Use relative links (e.g., `../examples/hello-world/README.md`).

## Examples (`examples/`)

Each example lives in its own directory with `mapping.yaml` and `README.md`.

### README format

```markdown
# Title

One-line description.

## Scenario

What the example models and why.

## Key features

- **`property: value`** — what it demonstrates

## How it works

Step-by-step explanation of the generated pipeline.

## When to use

Guidance on applicability.
```

- Title: sentence case, no "example" suffix.
- Sections: Scenario, Key features, How it works, When to use. Omit sections that don't apply.
- Never reference engine internals or plan files.

### Catalog (`examples/README.md`)

The `Full Example Catalog` table must list every example directory, sorted alphabetically. When adding an example, add its row to the table.

## Plans (`engine-rs/plans/`)

Design plans and decision records for the reference engine.

### Header format

```markdown
# Title

**Status:** Value

Summary paragraph or section headings.
```

- Title: sentence case, descriptive (e.g., "Atomic resolution groups", not "ATOMIC-GROUPS-PLAN").
- Status values: `Done`, `Planned`, `Pattern`, `Design`, `Proposed`, `Superseded`, `Maybe`.
- No `**Priority:**`, `**Effort:**`, or `## Status:` headings — use the `**Status:**` format only.

### Index (`engine-rs/plans/README.md`)

The index table must list every plan file with its status and a one-line summary. Update the index whenever a plan's status changes.

## Documentation (`docs/`)

- `schema-reference.md` — authoritative property reference.
- `design-rationale.md` — why decisions were made.
- `ai-guidelines.md` — compact reference for AI agents.
- `annotated-example.md` — walkthrough of a complete mapping.

When adding a new mapping property or strategy, update all four files.
