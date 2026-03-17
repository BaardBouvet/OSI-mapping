# Property-based testing

**Status:** Done

Property-based testing harness using the `proptest` crate to fuzz the engine
with randomly generated mapping documents and verify structural invariants.

## Goal

Complement the example-driven integration tests (37 curated mapping files)
with randomised inputs that explore corner cases humans wouldn't think to
write. The harness should catch:

- Panics / unwraps in parser, validator, DAG builder, or renderers
- Malformed SQL output (syntax errors, unbalanced parens, missing commas)
- Internal invariant violations (duplicate view names, dangling references)
- Regressions in edge-case handling (empty mappings, single-field targets,
  deeply nested paths, unusual characters in names)

## What to generate

### Tier 1 — Structure fuzzing (no Postgres)

Generate random `MappingDocument` values and verify the pipeline doesn't
panic or produce structurally invalid SQL.

**Generated elements:**

| Element | Strategy | Constraints |
|---------|----------|-------------|
| Target names | `[a-z][a-z0-9_]{0,15}` | Unique within document |
| Field names | `[a-z][a-z0-9_]{0,15}` | Unique within target |
| Strategy | Uniform over enum variants | — |
| Field type | Optional `text`, `integer`, `numeric`, `boolean`, `date` | — |
| Mapping names | `[a-z][a-z0-9_]{0,20}` | Unique within document |
| Source dataset | `[a-z][a-z0-9_]{0,15}` | — |
| Source path | Optional 1-3 depth `[a-z]+` segments | — |
| Field mappings | 1-8 per mapping | source + target from available names |
| Priority | Optional `1..100` | — |
| Direction | Uniform over `bidirectional`, `forward_only`, `reverse_only` | — |
| Expression | Optional simple literal (`'text'`, `true`, `42`) | — |
| Filter | Optional simple comparison (`col = 'val'`) | — |
| References | Optional FK to another target | Target must exist |
| Number of targets | 1-4 | — |
| Number of mappings | 1-8 | — |

**Constraints enforced during generation:**
- Every target has at least one `identity` field
- Every mapping's `target` refers to a declared target
- Every field mapping's `target` refers to a field in the mapping's target
- At least one mapping per target (field coverage)

### Tier 2 — SQL execution fuzzing (with Postgres)

Take Tier 1 generated documents, generate corresponding fake source tables
with random data, execute the full SQL pipeline against a real Postgres
instance, and verify:

- All views create without error
- Delta views produce valid `action` values (`'update'`, `'insert'`,
  `'delete'`, `'noop'`)
- Analytics views are queryable and contain expected columns
- No unexpected NULLs in identity columns

This tier reuses the existing `testcontainers` infrastructure from
`tests/integration.rs`.

## Invariants to check

### Structural (Tier 1, no Postgres)

1. **No panics.** `parse_file()`, `validate()`, `build_dag()`, `render_sql()`
   must not panic on any valid-schema input.
2. **SQL well-formedness.** Output contains balanced parentheses, balanced
   quotes, no empty `SELECT` lists, no trailing commas.
3. **View name uniqueness.** Every `CREATE OR REPLACE VIEW` name appears
   exactly once.
4. **DAG completeness.** Every target produces at least: `_fwd_*` (one per
   mapping), `_id_{target}`, `_resolved_{target}`, `{target}` (analytics).
5. **No dangling references.** Every `FROM` / `JOIN` clause references a
   view that is created earlier in the output.

### Execution (Tier 2, with Postgres)

6. **All views execute.** Zero SQL errors when running the full output.
7. **Delta actions valid.** `SELECT DISTINCT _action FROM _delta_{source}`
   returns only `{update, insert, delete, noop}`.
8. **Identity reflexivity.** A source row always appears in its own cluster
   (`_id_{target}` has a row for every `_fwd_*` contribution).
9. **Analytics columns match target.** `SELECT * FROM {target} LIMIT 0`
   returns `_cluster_id` + all declared target field names.
10. **Idempotence.** Running the same SQL twice produces the same result
    (views are `CREATE OR REPLACE`).

## Implementation

### File structure

```
engine-rs/
  tests/
    integration.rs          # existing
    proptest_structural.rs  # Tier 1 (new)
    proptest_execution.rs   # Tier 2 (new, optional)
  src/
    ...
```

### Dependencies

```toml
[dev-dependencies]
proptest = "1"
```

### Generator sketch

```rust
use proptest::prelude::*;

/// Generate a valid target name
fn target_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,15}".prop_filter("not empty", |s| !s.is_empty())
}

/// Generate a complete MappingDocument
fn arb_mapping_doc() -> impl Strategy<Value = MappingDocument> {
    // 1. Generate 1-4 targets with 1-6 fields each
    // 2. Ensure at least one identity field per target
    // 3. Generate 1-8 mappings referencing those targets
    // 4. Generate field mappings referencing declared fields
    // 5. Optionally add references between targets
    prop::collection::vec(arb_target(), 1..=4)
        .prop_flat_map(|targets| {
            let target_names: Vec<String> = targets.iter().map(|t| t.0.clone()).collect();
            (Just(targets), arb_mappings(target_names))
        })
        .prop_map(|(targets, mappings)| {
            MappingDocument {
                version: "1.0".into(),
                targets: targets.into_iter().collect(),
                mappings,
                ..Default::default()
            }
        })
}
```

### SQL validation helpers

```rust
fn check_balanced_parens(sql: &str) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    for ch in sql.chars() {
        match ch {
            '\'' if !in_string => in_string = true,
            '\'' if in_string => in_string = false,
            '(' if !in_string => depth += 1,
            ')' if !in_string => depth -= 1,
            _ => {}
        }
        if depth < 0 { return false; }
    }
    depth == 0
}

fn check_no_empty_select(sql: &str) -> bool {
    // SELECT followed immediately by FROM (nothing between)
    !sql.contains("SELECT\nFROM") && !sql.contains("SELECT FROM")
}
```

### Proptest configuration

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn no_panics_on_random_docs(doc in arb_mapping_doc()) {
        // Validation may reject some generated docs — that's fine,
        // the point is it doesn't panic.
        let _ = osi_engine::validate::validate(&doc);
        let dag = osi_engine::dag::build_dag(&doc);
        let result = osi_engine::render::render_sql(&doc, &dag, false, false);
        if let Ok(sql) = &result {
            prop_assert!(check_balanced_parens(sql));
            prop_assert!(check_no_empty_select(sql));
        }
    }
}
```

## Phasing

### Phase 1 — Structural fuzzing
- Add `proptest` dependency
- Implement `arb_mapping_doc()` generator (respecting constraints)
- Implement SQL structural checks (parens, commas, view names)
- 500 cases per run, ~5 seconds

### Phase 2 — Execution fuzzing
- Extend generator to produce matching source data
- Reuse `setup_pg()` and `split_sql_statements()` from integration.rs
- Check all SQL executes + delta action values
- 50 cases per run (Postgres overhead), ~30 seconds

### Phase 3 — Shrinking & regression
- Proptest automatically shrinks failing cases to minimal reproductions
- Add a `regressions/` file to persist known-failing seeds
- CI runs with a fixed seed for reproducibility

## Scope

- 2 new test files (~400 lines total for Phase 1)
- 1 new dev-dependency (`proptest = "1"`)
- No changes to production code
- Runs alongside existing tests (`cargo test`)
