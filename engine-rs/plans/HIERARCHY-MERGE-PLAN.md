# HIERARCHY-MERGE EXAMPLE PLAN

Demonstrate merging project hierarchies of different depths: a simple PM system
with 2 levels (project → task) and an enterprise PM system with 3 levels
(program → project → task). Both contribute to the same target entities, with
projects and tasks merging across systems via identity resolution.

## Scenario

**Simple PM** — flat table, each row is a project with a nested `tasks` array:
```
┌──────────────────────────────────────┐
│ simple_pm                            │
│  id: "S1"                            │
│  project_name: "Website Redesign"    │
│  owner: "Alice"                      │
│  tasks: [                            │
│    { name: "Design mockups",         │
│      status: "done",  hours: 8 },    │
│    { name: "Build frontend",         │
│      status: "active", hours: 20 }   │
│  ]                                   │
└──────────────────────────────────────┘
```

**Enterprise PM** — each row is a program with nested `projects`, each
containing nested `tasks` (3 levels):
```
┌──────────────────────────────────────────┐
│ enterprise_pm                            │
│  id: "E1"                                │
│  program_name: "Digital Transformation"  │
│  sponsor: "VP Engineering"               │
│  projects: [                             │
│    { name: "Website Redesign",           │
│      budget: 50000,                      │
│      tasks: [                            │
│        { name: "Design mockups",         │
│          assignee: "Bob", priority: 1 }, │
│        { name: "Build frontend",         │
│          assignee: "Carol", priority: 2} │
│      ]                                   │
│    }                                     │
│  ]                                       │
└──────────────────────────────────────────┘
```

## Target model

Three entities — tasks and projects merge across systems, programs only come
from the enterprise system:

```
program               project                task
───────               ───────                ────
program_name (id)     project_name (id)      task_name (id)
sponsor               owner (coalesce)       project_name (id)
                      budget (coalesce)      status (coalesce)
                      program_name (ref)     hours (coalesce)
                                             assignee (coalesce)
                                             priority (coalesce)
```

**Identity matching:**
- Programs: by `program_name`
- Projects: by `project_name`
- Tasks: by `(task_name, project_name)` — compound identity scoped to project

## Mappings

### Simple PM (2 levels)

```yaml
# Level 1: project rows (flat)
- name: simple_projects
  source: { dataset: simple_pm }
  target: project
  fields:
    - source: project_name
      target: project_name
    - source: owner
      target: owner

# Level 2: tasks (nested array)
- name: simple_tasks
  source:
    dataset: simple_pm
    path: tasks
    parent_fields:
      parent_project: project_name
  target: task
  fields:
    - source: parent_project
      target: project_name
      references: simple_projects
    - source: name
      target: task_name
    - source: status
      target: status
    - source: hours
      target: hours
```

### Enterprise PM (3 levels)

```yaml
# Level 1: program rows (flat)
- name: enterprise_programs
  source: { dataset: enterprise_pm }
  target: program
  fields:
    - source: program_name
      target: program_name
    - source: sponsor
      target: sponsor

# Level 2: projects (nested array — 1 deep)
- name: enterprise_projects
  source:
    dataset: enterprise_pm
    path: projects
    parent_fields:
      parent_program: program_name
  target: project
  fields:
    - source: name
      target: project_name
    - source: budget
      target: budget
    - source: parent_program
      target: program_name
      references: enterprise_programs

# Level 3: tasks (nested array — 2 deep)
- name: enterprise_tasks
  source:
    dataset: enterprise_pm
    path: projects.tasks
    parent_fields:
      parent_project: name
  target: task
  fields:
    - source: parent_project
      target: project_name
      references: enterprise_projects
    - source: name
      target: task_name
    - source: assignee
      target: assignee
    - source: priority
      target: priority
```

### After PARENT-MAPPING-PLAN (future syntax)

```yaml
- name: simple_tasks
  parent: simple_projects
  array: tasks
  parent_fields:
    parent_project: project_name
  target: task
  fields: [...]

- name: enterprise_projects
  parent: enterprise_programs
  array: projects
  parent_fields:
    parent_program: program_name
  target: project
  fields: [...]

- name: enterprise_tasks
  parent: enterprise_projects
  array: tasks
  parent_fields:
    parent_project: name
  target: task
  fields: [...]
```

## What this demonstrates

1. **Depth mismatch merge** — Simple PM has no "program" concept; Enterprise PM
   has all three levels. Projects and tasks merge via identity despite living at
   different structural depths in their source systems.

2. **Cross-depth identity resolution** — `simple_tasks` (1-deep nested array)
   and `enterprise_tasks` (2-deep nested array) resolve to the same task
   entities via compound identity `(task_name, project_name)`.

3. **Coalesce from different depths** — Task `status` and `hours` come from
   Simple PM; `assignee` and `priority` come from Enterprise PM. The resolved
   task has all four fields despite them originating at different nesting depths.

4. **Reference preservation across depths** — Enterprise projects reference
   programs (`program_name`); tasks from both systems reference projects.
   The relationship graph is consistent regardless of source depth.

5. **Asymmetric contribution** — Programs only flow from Enterprise PM. Projects
   merge from both. Tasks merge from both. Each entity accumulates whatever
   fields each source provides.

## Test cases

### Test 1: Both systems contribute — fields merge across depths

Input: Simple PM has "Website Redesign" with 2 tasks. Enterprise PM has
"Digital Transformation" program containing the same project with the same tasks
but different fields (budget, assignee, priority).

Expected:
- Program "Digital Transformation" exists (from enterprise only)
- Project "Website Redesign" has owner from simple, budget from enterprise,
  program_name reference from enterprise
- Task "Design mockups" has status+hours from simple, assignee+priority from
  enterprise
- Simple PM delta: noop (no changes flow back)
- Enterprise PM delta: updates with status+hours from simple tasks flowing back

### Test 2: Source-only entities (no overlap)

Input: Simple PM has "Internal Tool" project with tasks. Enterprise PM has
"Cloud Migration" program with "Infrastructure" project and tasks.

Expected:
- No merging occurs — all entities are unique to their source
- Simple PM delta: noop
- Enterprise PM delta: noop

## Implementation

1. Create `examples/hierarchy-merge/` directory
2. Write `mapping.yaml` using current syntax (`source.path` + `parent_fields`)
3. Write test data exercising both test cases
4. Write `README.md` explaining the depth-mismatch pattern
5. Verify all tests pass

## Complexity notes

- Uses existing nested array features — no engine changes needed
- Compound identity on task `(task_name, project_name)` uses `group:` if needed,
  or two identity fields
- The 2-deep path `projects.tasks` is already supported
- Reverse reconstruction rebuilds JSONB arrays at each depth
