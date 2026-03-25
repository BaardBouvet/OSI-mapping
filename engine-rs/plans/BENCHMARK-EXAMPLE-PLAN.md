# Benchmark example

**Status:** Planned

Large-scale mapping example for benchmarking engine compilation, SQL
generation, and runtime query performance. 30 systems × 10 targets = 300
mappings producing ~930 views.

## Goal

Provide a single `examples/benchmark-large/mapping.yaml` that exercises the
engine at scale. Use cases:

- Measure compilation time (YAML → DAG → SQL)
- Measure PostgreSQL view creation time (DDL execution)
- Measure query latency on analytics and delta views
- Stress-test the DAG builder and renderer with hundreds of nodes
- Provide a baseline for optimisation work

## Scale

| Dimension | Count |
|-----------|-------|
| Systems (sources) | 30 (`sys_01` … `sys_30`) |
| Targets (globals) | 10 (`target_a` … `target_j`) |
| Datatypes per system | 10 (one per target) |
| Mappings | 300 (30 × 10) |
| Fields per target | 4–9 (varies) |
| Total field mappings | ~1,830 |

## View count estimate

Formula: `Total = M + 3T + R + S_r`

| View type | Formula | Count |
|-----------|---------|-------|
| Forward (`_fwd_*`) | 1 per mapping | 300 |
| Identity (`_id_*`) | 1 per target | 10 |
| Resolved (`_resolved_*`) | 1 per target | 10 |
| Analytics | 1 per target | 10 |
| Reverse (`_rev_*`) | 1 per bidirectional mapping | 300 |
| Delta (`_delta_*`) | 1 per unique source dataset | 300 |
| **Total** | | **~930** |

## Target schemas

Each target exercises a different combination of building blocks. Targets
are named `target_a` through `target_j`.

### target_a — flat, coalesce-heavy, groups

Exercises: `coalesce`, `group` (atomic resolution), `default`.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_a | identity | |
| fa1 | coalesce | `group: "addr"` |
| fa2 | coalesce | `group: "addr"`, `default: "unknown"` |
| fa3 | coalesce | |
| fa4 | last_modified | |

### target_b — last_modified-heavy, expressions, normalize

Exercises: `last_modified`, `expression`, `normalize`, `type: numeric`.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_b | identity | |
| fb1 | last_modified | |
| fb2 | last_modified | `type: numeric`, `normalize: "round($value, 2)"` |
| fb3 | last_modified | |
| fb4 | coalesce | `expression: "upper(fb4)"` |

### target_c — nested array child of target_a

Exercises: `parent`, `array`, `parent_fields`, `link_group`, `scalar`.

This target is a child entity. Mappings for `target_c` use `parent:` to
reference a `target_a` mapping and `array:` to expand a JSONB column.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_c1 | identity | `link_group: "ck"` |
| key_c2 | identity | `link_group: "ck"` (composite key) |
| fc1 | coalesce | |
| fc2 | last_modified | |
| fc3 | coalesce | `scalar: true` |

### target_d — references, filter-routed

Exercises: `references`, `filter` (routing), `reverse_filter`.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_d | identity | |
| fd1 | coalesce | `references: target_a` |
| fd2 | coalesce | |
| fd3 | last_modified | |
| fd4 | last_modified | |
| fd5 | coalesce | |
| fd6 | coalesce | |

Mappings use `filter: "dtype = 'd'"` to route a subset of rows from a
shared source table, and `reverse_filter` to gate reverse sync.

### target_e — wide, mixed, self-reference

Exercises: `references` (self-ref), `bool_or`, `direction: forward_only`.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_e | identity | |
| fe1 | coalesce | `references: target_e` (self-ref) |
| fe2 | last_modified | |
| fe3 | coalesce | |
| fe4 | bool_or | |
| fe5 | last_modified | |
| fe6 | coalesce | `direction: forward_only` |
| fe7 | last_modified | |

### target_f — soft-delete, tombstones

Exercises: `soft_delete`, `derive_tombstones`, `cluster_members`.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_f | identity | |
| ff1 | coalesce | |
| ff2 | last_modified | |
| ff3 | coalesce | |

Mappings use `soft_delete: "deleted_at"`, `derive_tombstones: "_deleted"`,
and `cluster_members: true`.

### target_g — JSONB source_path extraction

Exercises: `source_path` (nested JSON), `type: jsonb`, `type: boolean`.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_g | identity | |
| fg1 | coalesce | `source_path: "meta.tier"` |
| fg2 | last_modified | `source_path: "meta.score"`, `type: numeric` |
| fg3 | coalesce | `type: jsonb` (opaque blob) |
| fg4 | coalesce | `type: boolean` |
| fg5 | last_modified | |
| fg6 | coalesce | `source_path: "meta.tags[0]"` |

### target_h — CRDT ordering, element resolution

Exercises: `order`, `elements: last_modified`, child array with ordering.

This target is a child entity resolved per-element across sources.

`elements: last_modified` on the target definition.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_h1 | identity | `link_group: "hk"` |
| key_h2 | identity | `link_group: "hk"` |
| fh1 | coalesce | |
| fh2 | last_modified | `order: true` |

### target_i — two references, passthrough, written_state

Exercises: `references` (×2), `passthrough`, `written_state`,
`derive_timestamps`, `reverse_expression`.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_i | identity | |
| fi1 | coalesce | `references: target_d` |
| fi2 | coalesce | `references: target_b` |
| fi3 | last_modified | |
| fi4 | coalesce | `reverse_expression: "lower(fi4)"` |
| fi5 | last_modified | |

Mappings use `passthrough: [extra_col]`, `written_state: true`,
`derive_timestamps: true`.

### target_j — kitchen sink, reverse_required, defaults

Exercises: `reverse_required`, `default_expression`,
`references_field`, `direction: reverse_only`.

| Field | Strategy | Extras |
|-------|----------|--------|
| key_j | identity | |
| fj1 | coalesce | `references: target_a`, `references_field: "fa3"` |
| fj2 | coalesce | `reverse_required: true` |
| fj3 | last_modified | |
| fj4 | coalesce | `default_expression: "'N/A'"` |
| fj5 | coalesce | `direction: reverse_only` |
| fj6 | last_modified | |

### Feature coverage summary

| Building block | Target(s) |
|----------------|-----------|
| `coalesce` | a, b, c, d, e, f, g, h, i, j |
| `last_modified` | a, b, c, d, e, f, g, h, i, j |
| `references` | d, e (self), i (×2), j |
| `references_field` | j |
| `group` (atomic) | a |
| `link_group` (composite key) | c, h |
| `parent` / `array` | c, h |
| `parent_fields` | c |
| `source_path` | g |
| `type: numeric` | b, g |
| `type: jsonb` | g |
| `type: boolean` | g |
| `expression` | b |
| `reverse_expression` | i |
| `normalize` | b |
| `default` | a |
| `default_expression` | j |
| `direction: forward_only` | e |
| `direction: reverse_only` | j |
| `filter` | d |
| `reverse_filter` | d |
| `reverse_required` | j |
| `soft_delete` | f |
| `derive_tombstones` | f |
| `cluster_members` | f |
| `bool_or` | e |
| `scalar` | c |
| `order` (CRDT) | h |
| `elements: last_modified` | h |
| `passthrough` | i |
| `written_state` | i |
| `derive_timestamps` | i |

## Source and mapping patterns

### Source naming

```
sys_{NN}_{target}     →  e.g. sys_01_target_a, sys_17_target_j
```

Primary key per source: `id` (text). Source fields map 1:1 to target fields
in most cases. Every source has `updated_at` for `last_modified`. Sources
feeding `target_g` have a `meta` JSONB column for `source_path` extraction.

### Mapping allocation

Most targets get one mapping per system (30 mappings). Exceptions:

| Target | Mappings per system | Notes |
|--------|-------------------|-------|
| target_a | 1 (root) | 30 |
| target_c | 1 (child) | `parent:` references sys's target_a mapping, `array: "items"` |
| target_d | 1 | Some via `filter:` on shared source with target_j |
| target_h | 1 (child) | `parent:` references sys's target_e mapping |
| All others | 1 | Standard flat mappings |

Total: 300 mappings (30 systems × 10 targets).

### Mapping properties

Each mapping has `priority: {system_number}` (1–30) and
`last_modified: updated_at`. Most fields are `direction: bidirectional`
unless the target schema specifies otherwise. This maximises view count
(every mapping produces forward + reverse, every source gets a delta).

## Tests

Minimal tests to prove the mapping compiles and propagates correctly.
Two systems (`sys_01`, `sys_02`) contributing to `target_a`:

1. **Cross-system propagation** — `sys_01` has row with key `"x"`,
   `sys_02` has row with same key. Verify coalesce picks `sys_01` (lower
   priority). Verify `sys_02` gets an update with the resolved value.

2. **Insert propagation** — `sys_01` has a row not in `sys_02`. Verify
   `sys_02` gets an insert.

Only `target_a` is tested to keep the test section small. The mapping file
itself is the benchmark payload — tests just prove validity.

## Files to create

1. `examples/benchmark-large/mapping.yaml` — the mapping (~5,000+ lines)
2. `examples/benchmark-large/README.md` — standard example README
3. Update `examples/README.md` — add row to catalog table

## Non-goals

- Realistic system or column names
- Comprehensive test coverage of all 10 targets
