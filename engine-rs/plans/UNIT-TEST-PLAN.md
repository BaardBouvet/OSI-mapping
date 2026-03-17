# Unit tests for the render pipeline

**Status:** Done

Add unit tests for the SQL rendering modules to reduce reliance on the
slow integration test suite (testcontainers + PostgreSQL).  The render
functions are pure — they accept model structs and return SQL strings —
so they are ideal for fast, deterministic unit tests.

---

## Problem

The current test pyramid is bottom-heavy on integration:

| Layer | Tests | Time | What it covers |
|-------|------:|-----:|----------------|
| Unit (`--lib`) | 58 | ~2 s | Parser, validator, expression checker, DAG, one forward-view test |
| Integration (`--test integration`) | 12 | ~18 s | Parse/render/execute against Postgres via testcontainers |

The render modules — which contain the most complex logic — have almost
no unit coverage:

| Module | Lines | Unit tests |
|--------|------:|-----------:|
| `render/delta.rs` | 984 | 0 |
| `render/reverse.rs` | 319 | 0 |
| `render/resolution.rs` | 304 | 0 |
| `render/identity.rs` | 287 | 0 |
| `render/mod.rs` | 445 | 0 |
| `render/analytics.rs` | 36 | 0 |
| `render/forward.rs` | 454 | 1 |

Every bug found in these modules today requires a full integration cycle
to detect.  The parent_field FK resolution bug (returning `_src_id` vs
identity field) is a recent example: the rendered SQL was syntactically
valid but semantically wrong, and only the full E2E test caught it.

### Goals

1. **Catch SQL generation bugs without a database.** Assert on SQL string
   content — column names, JOIN conditions, CASE branches, CTE structure.
2. **Fast feedback.** Unit tests run in <3 s even as coverage grows.
3. **Document render behavior.** Each test encodes an expectation about
   what SQL a given mapping configuration produces.
4. **Complement, not replace, integration tests.** E2E tests remain the
   ground truth; unit tests provide fast first-pass coverage.

---

## Test pattern

All render functions are pure: `(model structs) → Result<String>`.
Tests construct model structs directly (or parse inline YAML) and assert
on the returned SQL string.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    /// Helper: parse inline YAML and return the parsed document.
    fn parse(yaml: &str) -> MappingDocument {
        parser::parse_str(yaml).expect("valid test YAML")
    }

    #[test]
    fn reverse_parent_field_uses_src_id_for_root_ref() {
        let doc = parse(r#"
          version: "1.0"
          sources:
            s: { primary_key: id }
          targets:
            parent: { fields: { pid: { strategy: identity } } }
            child:  { fields: { cid: { strategy: identity }, pref: { strategy: coalesce, references: parent } } }
          mappings:
            - name: s_parents
              source: { dataset: s }
              target: parent
              fields: [{ source: id, target: pid }]
            - name: s_children
              parent: s_parents
              array: items
              parent_fields: { parent_id: id }
              target: child
              fields:
                - { source: cid, target: cid }
                - { source: parent_id, target: pref, references: s_parents }
        "#);
        let sql = render_reverse_view(
            &doc.mappings[1], "child",
            doc.targets.get("child").as_ref(),
            &doc.targets, None, &doc.mappings, &doc.sources,
        ).unwrap();
        // Root-level parent → _src_id (matches delta root join)
        assert!(sql.contains("ref_local._src_id"),
                "parent_field referencing root mapping should return _src_id");
    }
}
```

### Assertion strategy

- **`contains` checks** for specific SQL fragments (column names, JOIN
  conditions, CASE branches).  Robust against whitespace changes.
- **`!contains` checks** to assert absence (e.g., no `IS NULL` where
  `IS NOT DISTINCT FROM` is expected).
- **Snapshot comparison** (`insta` crate) for full SQL output of key
  scenarios.  Opt-in — not required for Phase 1.

---

## Phase 1 — High-value render tests (no new dependencies)

Add `#[cfg(test)] mod tests` blocks to each render module.  Parse
inline YAML snippets and assert on the generated SQL.

### reverse.rs

| Test | Asserts |
|------|---------|
| `parent_field_root_ref_returns_src_id` | FK subquery for a parent_field referencing a root mapping returns `ref_local._src_id` |
| `parent_field_nested_ref_returns_identity` | FK subquery for a parent_field referencing a nested (child) mapping returns `ref_local."identity_field"` |
| `regular_ref_returns_src_id` | Non-parent_field reference returns `ref_local._src_id` (default) |
| `typed_identity_ref_returns_field` | Reference to mapping with typed PK identity returns `ref_local."typed_field"` |
| `references_field_override` | `references_field:` forces the return column |
| `pk_type_casting` | PK column with `type: integer` on source produces `::integer` cast |
| `reverse_expression_passthrough` | `reverse_expression:` appears verbatim in SELECT |
| `reverse_filter_in_where` | `reverse_filter:` appears as WHERE clause |
| `child_mapping_null_action` | Child mapping produces `WHEN p._src_id IS NULL THEN NULL` (not 'insert') |

### delta.rs

| Test | Asserts |
|------|---------|
| `simple_noop_detection` | CASE branch includes `_base->>'field' IS NOT DISTINCT FROM` |
| `nested_array_cte_structure` | `WITH _nested_` CTE present with `jsonb_agg` and `_parent_key` |
| `deep_nesting_intermediate_join` | Child CTE LEFT JOINs on `n."identity_field"::text` |
| `root_nesting_join_on_pk` | Root CTE joins `_parent_key = p."pk"::text` |
| `merged_delta_union` | Multi-source delta produces UNION ALL of reverse views |
| `multi_pk_columns` | Composite PK produces all columns in SELECT and noop check |
| `forward_only_excluded` | `direction: forward_only` mapping has no delta view |
| `delete_detection_present` | Delta includes `WHEN src_id IS NOT NULL AND resolved IS NULL THEN 'delete'` or equivalent |
| `text_norm_both_sides` | `_osi_text_norm` applied to both `_base` and reconstructed array |

### identity.rs

| Test | Asserts |
|------|---------|
| `recursive_cte_structure` | Contains `WITH RECURSIVE` and iterative closure step |
| `union_all_base` | All mappings contributing to target appear in UNION ALL base |
| `link_group_edges` | `link_group` fields produce edge rows in the recursive seed |
| `single_mapping_no_recursion` | Single-mapping identity skips recursion (trivial path) |

### resolution.rs

| Test | Asserts |
|------|---------|
| `coalesce_strategy` | `array_agg ... FILTER (WHERE ... IS NOT NULL)` pattern |
| `last_modified_strategy` | ORDER BY timestamp, priority in aggregation |
| `bool_or_strategy` | `bool_or(...)` in SELECT |
| `group_distinct_on` | `DISTINCT ON` CTE for grouped fields |
| `default_expression_fallback` | `COALESCE(resolved, default_expression)` pattern |
| `references_field_in_resolved` | Reference fields resolve entity refs to target PKs |

### forward.rs (extend existing)

| Test | Asserts |
|------|---------|
| `nested_array_lateral_join` | `CROSS JOIN LATERAL jsonb_array_elements` present |
| `source_path_extraction` | `source_path: "a.b.c"` produces chained `->` / `->>` |
| `expression_passthrough` | `expression:` used verbatim in SELECT |
| `filter_in_where` | `filter:` appears as WHERE clause |
| `parent_field_promoted` | Parent field appears as direct column reference, not from array item |
| `base_includes_parent_fields` | `_base` jsonb_build_object includes parent_field aliases |

### mod.rs

| Test | Asserts |
|------|---------|
| `create_tables_ddl` | `--create-tables` flag emits CREATE TABLE for each source |
| `annotate_comments` | `--annotate` flag emits comment blocks before views |
| `view_order_follows_dag` | Forward views appear before identity, identity before resolution, etc. |
| `osi_text_norm_function` | Helper function DDL appears at top of output |

### analytics.rs

| Test | Asserts |
|------|---------|
| `analytics_selects_from_resolved` | View selects from `_resolved_{target}` |
| `only_user_fields` | No internal columns (`_src_id`, `_mapping`, etc.) in output |

**Estimated count:** ~40 tests.

---

## Phase 2 — Snapshot tests with `insta`

Add `insta` as a dev-dependency and snapshot the full SQL output for a
small set of canonical mappings.

### Why snapshots

- Catch unintended SQL changes across refactors.
- Reviewers see exact SQL diffs in PRs.
- Complement fragment-based `contains` checks with full-output coverage.

### Snapshot targets

| Mapping | Why |
|---------|-----|
| `hello-world` | Minimal baseline — catches regressions in the simplest path |
| `nested-arrays-deep` | Deep nesting, parent_fields, delta CTE tree |
| `merge-groups` | Atomic groups, DISTINCT ON, multi-source resolution |
| `references` | FK resolution, multi-target identity |
| `multi-value` | Cardinality mismatch, expression, forward_only |
| `propagated-delete` | `bool_or`, `reverse_filter` |

### Workflow

```bash
# Generate/update snapshots:
cargo insta review

# CI: reject unapproved changes:
cargo insta test --check
```

**Estimated count:** ~6 snapshots (one per canonical mapping, covering
all pipeline stages).

---

## Phase 3 — Helpers unit

Extract and test small helper functions that are currently inline:

| Function | Location | Test focus |
|----------|----------|------------|
| `qi()` | `lib.rs` | Quoting edge cases (reserved words, special chars) |
| `sql_escape()` | `render/mod.rs` | Escaping single quotes in strings |
| `pk_base_expr_map()` | `render/reverse.rs` | Type resolution priority |
| `build_nesting_tree()` | `render/delta.rs` | Tree construction from flat path segments |
| `find_node_mut()` | `render/delta.rs` | Path traversal in nesting tree |
| `json_path_expr()` | `render/forward.rs` | Dotted path → PostgreSQL `->` / `->>` chain |

These are pure functions with narrow input/output — ideal for
table-driven tests.

**Estimated count:** ~15 tests.

---

## Implementation notes

### Test YAML construction

For most tests, parse inline YAML via `parser::parse_str()`.  This
exercises the parser as a side effect and keeps tests self-contained.
Only use direct struct construction when testing a render function in
isolation from the parser.

### No new dev-dependencies in Phase 1

Phase 1 uses only `assert!` / `assert_eq!` / `contains` checks — no
extra crates.  `insta` is introduced in Phase 2 as an explicit choice.

### CI integration

Unit tests already run via `cargo test --lib`.  No CI changes needed
for Phase 1.  Phase 2 adds `cargo insta test --check` to CI.

### Naming convention

Test names use the pattern `{module}_{scenario}_{expectation}`, e.g.
`reverse_parent_field_root_ref_returns_src_id`.  This reads well in
`cargo test` output and makes it obvious which render behavior broke.

---

## Exit criteria

- Phase 1: 30+ render unit tests, all passing.  `cargo test --lib`
  stays under 3 s.
- Phase 2: Snapshot tests for 6 canonical mappings.  `insta` in
  dev-dependencies.
- Phase 3: Helper function tests cover edge cases.  Total unit test
  count ≥ 100.
