#!/usr/bin/env python3
"""Generate benchmark-large mapping.yaml."""

SYSTEMS = [f"sys_{i:02d}" for i in range(1, 31)]
ROOT_TARGETS = [
    "target_a",
    "target_b",
    "target_d",
    "target_e",
    "target_f",
    "target_g",
    "target_i",
    "target_j",
]

lines = []


def w(line=""):
    lines.append(line)


def gen_header():
    w('version: "1.0"')
    w("description: >")
    w("  Large-scale benchmark mapping. 30 systems, 10 targets, 300 mappings.")
    w("  Exercises coalesce, last_modified, groups, defaults, references,")
    w("  self-references, filters, soft_delete, derive_tombstones, source_path,")
    w("  CRDT ordering, link_group, parent/array, normalize, bool_or,")
    w("  passthrough, reverse_required, default_expression, and type variants.")
    w()


def gen_sources():
    w("sources:")
    for sys in SYSTEMS:
        for tgt in ROOT_TARGETS:
            w(f"  {sys}_{tgt}:")
            w("    primary_key: id")
    w()


def gen_targets():
    w("targets:")

    # target_a — coalesce, group, default
    w("  target_a:")
    w("    fields:")
    w("      key_a:")
    w("        strategy: identity")
    w("      fa1:")
    w("        strategy: coalesce")
    w("        group: addr")
    w("      fa2:")
    w("        strategy: coalesce")
    w("        group: addr")
    w('        default: "unknown"')
    w("      fa3:")
    w("        strategy: coalesce")
    w("      fa4:")
    w("        strategy: last_modified")
    w()

    # target_b — last_modified, type:numeric, normalize
    w("  target_b:")
    w("    fields:")
    w("      key_b:")
    w("        strategy: identity")
    w("      fb1:")
    w("        strategy: last_modified")
    w("      fb2:")
    w("        strategy: last_modified")
    w("        type: numeric")
    w("      fb3:")
    w("        strategy: last_modified")
    w("      fb4:")
    w("        strategy: coalesce")
    w()

    # target_c — nested array child (link_group composite key)
    w("  target_c:")
    w("    fields:")
    w("      key_c1:")
    w("        strategy: identity")
    w("        link_group: ck")
    w("      key_c2:")
    w("        strategy: identity")
    w("        link_group: ck")
    w("      fc1:")
    w("        strategy: coalesce")
    w("      fc2:")
    w("        strategy: coalesce")
    w()

    # target_d — references, filter, reverse_filter
    w("  target_d:")
    w("    fields:")
    w("      key_d:")
    w("        strategy: identity")
    w("      fd1:")
    w("        strategy: coalesce")
    w("        references: target_a")
    w("      fd2:")
    w("        strategy: coalesce")
    w("      fd3:")
    w("        strategy: last_modified")
    w("      fd4:")
    w("        strategy: last_modified")
    w("      fd5:")
    w("        strategy: coalesce")
    w("      fd6:")
    w("        strategy: coalesce")
    w()

    # target_e — self-ref, bool_or, forward_only
    w("  target_e:")
    w("    fields:")
    w("      key_e:")
    w("        strategy: identity")
    w("      fe1:")
    w("        strategy: coalesce")
    w("        references: target_e")
    w("      fe2:")
    w("        strategy: last_modified")
    w("      fe3:")
    w("        strategy: coalesce")
    w("      fe4:")
    w("        strategy: bool_or")
    w("        type: boolean")
    w("      fe5:")
    w("        strategy: last_modified")
    w("      fe6:")
    w("        strategy: coalesce")
    w("      fe7:")
    w("        strategy: last_modified")
    w()

    # target_f — soft_delete, derive_tombstones, cluster_members
    w("  target_f:")
    w("    fields:")
    w("      key_f:")
    w("        strategy: identity")
    w("      ff1:")
    w("        strategy: coalesce")
    w("      ff2:")
    w("        strategy: last_modified")
    w("      ff3:")
    w("        strategy: coalesce")
    w("      _deleted:")
    w("        strategy: bool_or")
    w("        type: boolean")
    w()

    # target_g — source_path, type:jsonb, type:boolean
    w("  target_g:")
    w("    fields:")
    w("      key_g:")
    w("        strategy: identity")
    w("      fg1:")
    w("        strategy: coalesce")
    w("      fg2:")
    w("        strategy: last_modified")
    w("      fg3:")
    w("        strategy: coalesce")
    w("        type: jsonb")
    w("      fg4:")
    w("        strategy: coalesce")
    w("        type: boolean")
    w("      fg5:")
    w("        strategy: last_modified")
    w("      fg6:")
    w("        strategy: coalesce")
    w()

    # target_h — CRDT ordering child (link_group)
    w("  target_h:")
    w("    fields:")
    w("      key_h1:")
    w("        strategy: identity")
    w("        link_group: hk")
    w("      key_h2:")
    w("        strategy: identity")
    w("        link_group: hk")
    w("      fh1:")
    w("        strategy: coalesce")
    w("      fh2:")
    w("        strategy: coalesce")
    w()

    # target_i — two references, passthrough
    w("  target_i:")
    w("    fields:")
    w("      key_i:")
    w("        strategy: identity")
    w("      fi1:")
    w("        strategy: coalesce")
    w("        references: target_d")
    w("      fi2:")
    w("        strategy: coalesce")
    w("        references: target_b")
    w("      fi3:")
    w("        strategy: last_modified")
    w("      fi4:")
    w("        strategy: coalesce")
    w("      fi5:")
    w("        strategy: last_modified")
    w()

    # target_j — reverse_required, default_expression
    w("  target_j:")
    w("    fields:")
    w("      key_j:")
    w("        strategy: identity")
    w("      fj1:")
    w("        strategy: coalesce")
    w("        references: target_a")
    w("      fj2:")
    w("        strategy: coalesce")
    w("      fj3:")
    w("        strategy: last_modified")
    w("      fj4:")
    w("        strategy: coalesce")
    w("        default_expression: \"'N/A'\"")
    w("      fj5:")
    w("        strategy: last_modified")
    w()


def gen_mappings():
    w("mappings:")
    for i, sys in enumerate(SYSTEMS, 1):
        sep = "\u2500" * 60
        w(f"  # \u2500\u2500 {sys} {sep}")
        w()

        # target_a
        w(f"  - name: {sys}_target_a")
        w(f"    source: {sys}_target_a")
        w("    target: target_a")
        w(f"    priority: {i}")
        w("    last_modified: updated_at")
        w("    fields:")
        w("      - source: key_a")
        w("        target: key_a")
        w("      - source: fa1")
        w("        target: fa1")
        w("      - source: fa2")
        w("        target: fa2")
        w("      - source: fa3")
        w("        target: fa3")
        w("      - source: fa4")
        w("        target: fa4")
        w()

        # target_b
        w(f"  - name: {sys}_target_b")
        w(f"    source: {sys}_target_b")
        w("    target: target_b")
        w(f"    priority: {i}")
        w("    last_modified: updated_at")
        w("    fields:")
        w("      - source: key_b")
        w("        target: key_b")
        w("      - source: fb1")
        w("        target: fb1")
        w("      - source: fb2")
        w("        target: fb2")
        w('        normalize: "round(%s::numeric, 2)"')
        w("      - source: fb3")
        w("        target: fb3")
        w("      - source: fb4")
        w("        target: fb4")
        w()

        # target_c (child of target_a)
        w(f"  - name: {sys}_target_c")
        w(f"    parent: {sys}_target_a")
        w("    array: items")
        w("    parent_fields:")
        w("      parent_key: key_a")
        w("    target: target_c")
        w(f"    priority: {i}")
        w("    fields:")
        w("      - source: parent_key")
        w("        target: key_c1")
        w(f"        references: {sys}_target_a")
        w("      - source: elem_key")
        w("        target: key_c2")
        w("      - source: fc1")
        w("        target: fc1")
        w("      - source: fc2")
        w("        target: fc2")
        w()

        # target_d
        w(f"  - name: {sys}_target_d")
        w(f"    source: {sys}_target_d")
        w("    target: target_d")
        w(f"    priority: {i}")
        w("    last_modified: updated_at")
        w("    filter: \"dtype = 'd'\"")
        w('    reverse_filter: "fd5 IS NOT NULL"')
        w("    fields:")
        w("      - source: key_d")
        w("        target: key_d")
        w("      - source: fd1")
        w("        target: fd1")
        w(f"        references: {sys}_target_a")
        w("      - source: fd2")
        w("        target: fd2")
        w("      - source: fd3")
        w("        target: fd3")
        w("      - source: fd4")
        w("        target: fd4")
        w("      - source: fd5")
        w("        target: fd5")
        w("      - source: fd6")
        w("        target: fd6")
        w("      - source: dtype")
        w("        direction: reverse_only")
        w("        reverse_expression: \"'d'\"")
        w()

        # target_e
        w(f"  - name: {sys}_target_e")
        w(f"    source: {sys}_target_e")
        w("    target: target_e")
        w(f"    priority: {i}")
        w("    last_modified: updated_at")
        w("    fields:")
        w("      - source: key_e")
        w("        target: key_e")
        w("      - source: fe1")
        w("        target: fe1")
        w(f"        references: {sys}_target_e")
        w("      - source: fe2")
        w("        target: fe2")
        w("      - source: fe3")
        w("        target: fe3")
        w("      - source: fe4")
        w("        target: fe4")
        w("      - source: fe5")
        w("        target: fe5")
        w("      - source: fe6")
        w("        target: fe6")
        w("        direction: forward_only")
        w("      - source: fe7")
        w("        target: fe7")
        w()

        # target_f
        w(f"  - name: {sys}_target_f")
        w(f"    source: {sys}_target_f")
        w("    target: target_f")
        w(f"    priority: {i}")
        w("    last_modified: updated_at")
        w("    cluster_members: true")
        w('    reverse_filter: "_deleted IS NOT TRUE"')
        if i % 2 == 0:
            w("    derive_tombstones: _deleted")
        else:
            w("    soft_delete: deleted_at")
        w("    fields:")
        w("      - source: key_f")
        w("        target: key_f")
        w("      - source: ff1")
        w("        target: ff1")
        w("      - source: ff2")
        w("        target: ff2")
        w("      - source: ff3")
        w("        target: ff3")
        w()

        # target_g
        w(f"  - name: {sys}_target_g")
        w(f"    source: {sys}_target_g")
        w("    target: target_g")
        w(f"    priority: {i}")
        w("    last_modified: updated_at")
        w("    fields:")
        w("      - source: key_g")
        w("        target: key_g")
        w("      - source_path: meta.tier")
        w("        target: fg1")
        w("      - source_path: meta.score")
        w("        target: fg2")
        w("      - source: fg3")
        w("        target: fg3")
        w("      - source: fg4")
        w("        target: fg4")
        w("      - source: fg5")
        w("        target: fg5")
        w('      - source_path: "meta.tags[0]"')
        w("        target: fg6")
        w()

        # target_h (child of target_e)
        w(f"  - name: {sys}_target_h")
        w(f"    parent: {sys}_target_e")
        w("    array: elements")
        w("    parent_fields:")
        w("      parent_key: key_e")
        w("    target: target_h")
        w(f"    priority: {i}")
        w("    fields:")
        w("      - source: parent_key")
        w("        target: key_h1")
        w(f"        references: {sys}_target_e")
        w("      - source: elem_id")
        w("        target: key_h2")
        w("      - source: val")
        w("        target: fh1")
        w("      - target: fh2")
        w("        order: true")
        w()

        # target_i
        w(f"  - name: {sys}_target_i")
        w(f"    source: {sys}_target_i")
        w("    target: target_i")
        w(f"    priority: {i}")
        w("    last_modified: updated_at")
        w("    passthrough:")
        w("      - extra_col")
        w("    fields:")
        w("      - source: key_i")
        w("        target: key_i")
        w("      - source: fi1")
        w("        target: fi1")
        w(f"        references: {sys}_target_d")
        w("      - source: fi2")
        w("        target: fi2")
        w(f"        references: {sys}_target_b")
        w("      - source: fi3")
        w("        target: fi3")
        w("      - source: fi4")
        w("        target: fi4")
        w("      - source: fi5")
        w("        target: fi5")
        w()

        # target_j
        w(f"  - name: {sys}_target_j")
        w(f"    source: {sys}_target_j")
        w("    target: target_j")
        w(f"    priority: {i}")
        w("    last_modified: updated_at")
        w("    fields:")
        w("      - source: key_j")
        w("        target: key_j")
        w("      - source: fj1")
        w("        target: fj1")
        w(f"        references: {sys}_target_a")
        w("      - source: fj2")
        w("        target: fj2")
        w("        reverse_required: true")
        w("      - source: fj3")
        w("        target: fj3")
        w("      - source: fj4")
        w("        target: fj4")
        w("      - source: fj5")
        w("        target: fj5")
        w()


def gen_test_input(test_a_keys):
    """Generate skeleton rows for all sources.

    Sources with parent/array (target_c, target_h) get their data from the
    parent source's JSONB column, so they don't need their own input entries.

    Sources that need special PG types (JSONB meta, boolean fg4, JSONB
    items/elements) must have at least one row so the test harness infers the
    correct PG type.  All systems use the SAME identity key with identical
    values so they merge into one cluster and produce only noop deltas.

    test_a_keys: set of source keys already listed by the caller for target_a.
    """
    for sys in SYSTEMS:
        key = f"{sys}_target_a"
        if key in test_a_keys:
            continue
        w(f"      {key}:")
        w(
            '        - { id: "1", key_a: "shared_a", fa1: "v1", fa2: "v2", fa3: "winner", fa4: "v4", updated_at: "2025-01-01T00:00:00Z", items: [] }'
        )

    for sys in SYSTEMS:
        w(f"      {sys}_target_b: []")

    for sys in SYSTEMS:
        w(f"      {sys}_target_d: []")

    for sys in SYSTEMS:
        # target_e: needs `elements` JSONB column for child target_h
        w(f"      {sys}_target_e:")
        w(
            '        - { id: "1", key_e: "shared_e", fe1: null, fe2: "v2", fe3: "v3", fe4: false, fe5: "v5", fe6: "v6", fe7: "v7", updated_at: "2025-01-01T00:00:00Z", elements: [] }'
        )

    for sys in SYSTEMS:
        # target_f: needs `deleted_at` for soft_delete on odd systems
        w(f"      {sys}_target_f:")
        w(
            '        - { id: "1", key_f: "shared_f", ff1: "v", ff2: "v", ff3: "v", deleted_at: null, updated_at: "2025-01-01T00:00:00Z" }'
        )

    for sys in SYSTEMS:
        # target_g: needs `meta` as JSONB for source_path extraction
        w(f"      {sys}_target_g:")
        w(
            '        - { id: "1", key_g: "shared_g", meta: {}, fg3: null, fg4: false, fg5: "v", updated_at: "2025-01-01T00:00:00Z" }'
        )

    for sys in SYSTEMS:
        w(f"      {sys}_target_i: []")

    for sys in SYSTEMS:
        w(f"      {sys}_target_j: []")


def gen_tests():
    w("tests:")

    # Test 1: coalesce resolution on target_a
    # sys_01 (priority 1) has fa3="winner", sys_02 (priority 2) has fa3="loser".
    # All other systems have shared_a with fa3="v3".
    # sys_01 wins coalesce. sys_02 gets an update.
    w(
        '  - description: "Coalesce resolution — sys_01 wins, sys_02 gets updated"'
    )
    w("    input:")
    w("      sys_01_target_a:")
    w(
        '        - { id: "1", key_a: "shared_a", fa1: "v1", fa2: "v2", fa3: "winner", fa4: "v4", updated_at: "2025-01-01T00:00:00Z", items: [] }'
    )
    w("      sys_02_target_a:")
    w(
        '        - { id: "1", key_a: "shared_a", fa1: "v1", fa2: "v2", fa3: "loser", fa4: "v4", updated_at: "2025-01-01T00:00:00Z", items: [] }'
    )
    gen_test_input({"sys_01_target_a", "sys_02_target_a"})
    w("    expected:")
    w("      sys_02_target_a:")
    w("        updates:")
    w(
        '          - { id: "1", key_a: "shared_a", fa1: "v1", fa2: "v2", fa3: "winner", fa4: "v4", items: [] }'
    )


# Generate
gen_header()
gen_sources()
gen_targets()
gen_mappings()
gen_tests()

print("\n".join(lines))
