# Hierarchy merge

Merging project hierarchies of different depths: 2-level simple PM and 3-level enterprise PM.

## Scenario

Two project management systems track the same projects and tasks at different structural depths.
Simple PM has a flat `project → tasks` structure; Enterprise PM adds a `program` level above projects, giving `program → project → tasks`.
Projects and tasks merge across the systems via identity resolution despite living at different nesting depths.

## Key features

- **`parent:` + `array:`** — extracts nested arrays at each hierarchy level
- **`parent_fields:`** — imports ancestor keys for reference fields
- **`strategy: identity`** — `project_name` and `task_name` match entities across systems
- **`priority: 1` / `priority: 2`** — overlapping fields (`owner`, `status`, `hours`) resolve via priority and propagate bidirectionally
- **`type: numeric`** — preserves integer types through the JSONB pipeline for `budget`, `hours`, `priority`

## How it works

1. Simple PM contributes projects (with `owner` priority 1) and tasks (with `status` priority 1, `hours` priority 2)
2. Enterprise PM contributes programs, projects (with `budget`, `lead` → `owner` priority 2), and tasks (with `status` priority 2, `hours` priority 1, `assignee`, `priority`)
3. Projects merge by `project_name` — Simple PM's `owner` wins and propagates to Enterprise PM's `lead`
4. Tasks merge by `task_name` — Simple PM's `status` wins (→ enterprise PM updated), Enterprise PM's `hours` win (→ simple PM updated)
5. Programs exist only in Enterprise PM — no cross-system merge needed

## When to use

When integrating systems that model the same domain at different hierarchy depths. The mapping layer handles the structural mismatch while identity resolution merges the shared entities.
