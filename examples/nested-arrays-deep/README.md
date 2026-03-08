# Nested Arrays - Deep Nesting

Demonstrates two levels of nested arrays: parent → children → grandchildren.

## How it works

1. Source records contain a `children` array, each child contains a `grandchildren` array
2. Three targets: `parent`, `child`, `grandchild` with reference chains
3. `source.path` extracts nested arrays; `parent_fields` imports ancestor keys
4. Each level maintains its own identity and relationships via `references`
