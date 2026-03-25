# Benchmark large

Stress-test mapping with 30 systems, 10 targets, and 300 mappings (~930 views).

## Scenario

A purely synthetic benchmark designed to exercise the engine at scale.
Thirty generic systems each contribute to all ten targets, producing 300
bidirectional mappings. The targets vary in field count (4–7), strategy mix
(coalesce, last_modified, bool_or), and building blocks.

## Key features

- **30 × 10 = 300 mappings** — every system maps to every target
- **`group:`** — atomic resolution groups on target_a
- **`normalize:`** — lossy precision on target_b
- **`parent:` / `array:` / `link_group:`** — nested array children (target_c, target_h)
- **`filter:` / `reverse_filter:`** — routing on target_d
- **`references:`** — FK to other targets (target_d, target_i, target_j) and self-ref (target_e)
- **`bool_or`** — boolean aggregation on target_e
- **`direction: forward_only`** — asymmetric sync on target_e
- **`soft_delete:` / `derive_tombstones:` / `cluster_members:`** — deletion on target_f
- **`source_path:`** — JSONB extraction on target_g
- **`type: numeric` / `jsonb` / `boolean`** — typed fields on target_b, target_g
- **`order: true`** — CRDT ordering on target_h
- **`passthrough:`** — unmapped columns on target_i
- **`reverse_required:`** — insert gates on target_j
- **`default:` / `default_expression:`** — fallback values on target_a, target_j
- **`reverse_expression:`** — reverse transform on target_d

## How it works

The generator script (`generate.py`) produces the mapping YAML
programmatically. Each system gets one mapping per target (or one child
mapping for nested-array targets). Priority is the system number (1–30),
so `sys_01` always wins coalesce resolution.

To regenerate: `python3 generate.py > mapping.yaml`

## When to use

Use this example to benchmark compilation time (YAML → SQL), DDL execution
time (view creation), and query latency at scale. Not intended as a model
for real mapping design.
