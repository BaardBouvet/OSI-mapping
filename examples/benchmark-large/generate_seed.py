#!/usr/bin/env python3
"""Generate seed SQL data for the benchmark-large example.

Produces INSERT statements for all 240 source tables (30 systems × 8 root
targets).  A handful of systems get real rows; the rest stay empty (tables
still need to exist so the views compile).

Usage:
    python3 generate_seed.py > seed.sql
"""

import json
import random
from datetime import datetime, timedelta, timezone

SYSTEMS = [f"sys_{i:02d}" for i in range(1, 31)]

# Systems that get real data rows.  The rest get empty CREATE TABLE only.
ACTIVE = {"sys_01", "sys_02", "sys_03", "sys_05", "sys_08", "sys_13"}

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

random.seed(42)
lines: list[str] = []


def w(line: str = "") -> None:
    lines.append(line)


def ts(days_ago: int = 0) -> str:
    dt = datetime(2025, 6, 1, tzinfo=timezone.utc) - timedelta(days=days_ago)
    return dt.strftime("%Y-%m-%dT%H:%M:%SZ")


def sql_val(v) -> str:
    if v is None:
        return "NULL"
    if isinstance(v, bool):
        return "TRUE" if v else "FALSE"
    if isinstance(v, (int, float)):
        return str(v)
    if isinstance(v, (dict, list)):
        return f"'{json.dumps(v, separators=(',', ':'))}'::jsonb"
    # string — single-quote escape
    return "'" + str(v).replace("'", "''") + "'"


def insert(table: str, columns: list[str], rows: list[list]) -> None:
    if not rows:
        return
    cols = ", ".join(columns)
    for row in rows:
        vals = ", ".join(sql_val(v) for v in row)
        w(f"INSERT INTO {table} ({cols}) VALUES ({vals});")


# ── Shared identity keys (entities that appear in multiple systems) ──────
# target_a entities
A_KEYS = ["customer-001", "customer-002", "customer-003", "order-100", "order-101"]
# target_b entities
B_KEYS = ["product-A", "product-B", "product-C"]
# target_d entities (filtered by dtype='d')
D_KEYS = ["doc-10", "doc-11"]
# target_e entities (self-ref: e2 -> e1)
E_KEYS = ["emp-1", "emp-2", "emp-3"]
# target_f entities (soft-delete)
F_KEYS = ["proj-X", "proj-Y", "proj-Z"]
# target_g entities (JSONB source_path)
G_KEYS = ["asset-01", "asset-02"]
# target_i entities
I_KEYS = ["txn-500", "txn-501"]
# target_j entities
J_KEYS = ["case-A", "case-B", "case-C"]


def gen_target_a(sys: str) -> None:
    table = f"{sys}_target_a"
    cols = ["id", "key_a", "fa1", "fa2", "fa3", "fa4", "updated_at", "items"]
    rows = []
    for idx, k in enumerate(A_KEYS, 1):
        # Vary fa3 per system so coalesce is interesting
        fa3 = f"{k}-{sys}-addr"
        items = [
            {"elem_key": f"item-{idx}-1", "fc1": f"color-{idx}", "fc2": f"size-{idx}"},
            {"elem_key": f"item-{idx}-2", "fc1": "red", "fc2": "XL"},
        ]
        rows.append([
            str(idx), k,
            f"name-{idx}", f"street-{idx}", fa3, f"city-{idx}",
            ts(idx * 10), items,
        ])
    insert(table, cols, rows)


def gen_target_b(sys: str) -> None:
    table = f"{sys}_target_b"
    cols = ["id", "key_b", "fb1", "fb2", "fb3", "fb4", "updated_at"]
    rows = []
    for idx, k in enumerate(B_KEYS, 1):
        price = round(random.uniform(5, 500), 4)
        rows.append([
            str(idx), k,
            f"desc-{idx}", price, f"cat-{idx}", f"brand-{sys}",
            ts(idx * 5),
        ])
    insert(table, cols, rows)


def gen_target_d(sys: str) -> None:
    table = f"{sys}_target_d"
    cols = ["id", "key_d", "fd1", "fd2", "fd3", "fd4", "fd5", "fd6", "dtype", "updated_at"]
    rows = []
    for idx, k in enumerate(D_KEYS, 1):
        # fd1 references target_a identity
        rows.append([
            str(idx), k,
            A_KEYS[idx % len(A_KEYS)], f"summary-{idx}",
            f"body-{idx}", f"tag-{idx}", f"status-{idx}", f"note-{idx}",
            "d", ts(idx * 3),
        ])
    insert(table, cols, rows)


def gen_target_e(sys: str) -> None:
    table = f"{sys}_target_e"
    cols = [
        "id", "key_e", "fe1", "fe2", "fe3", "fe4",
        "fe5", "fe6", "fe7", "updated_at", "elements",
    ]
    rows = []
    for idx, k in enumerate(E_KEYS, 1):
        # fe1 = self-ref (manager); first employee has no manager
        manager = E_KEYS[0] if idx > 1 else None
        elements = [
            {"elem_id": f"skill-{idx}-1", "val": f"python-{idx}"},
            {"elem_id": f"skill-{idx}-2", "val": f"rust-{idx}"},
        ]
        rows.append([
            str(idx), k, manager,
            f"title-{idx}", f"dept-{idx}", idx > 1,  # fe4 boolean
            f"office-{idx}", f"team-{sys}", f"rank-{idx}",
            ts(idx * 7), elements,
        ])
    insert(table, cols, rows)


def gen_target_f(sys: str, sys_idx: int) -> None:
    table = f"{sys}_target_f"
    # Odd systems use soft_delete (deleted_at), even use derive_tombstones
    cols = ["id", "key_f", "ff1", "ff2", "ff3", "deleted_at", "updated_at"]
    rows = []
    for idx, k in enumerate(F_KEYS, 1):
        # Soft-delete the last entity in some systems
        deleted = ts(1) if (k == "proj-Z" and sys in ("sys_02", "sys_08")) else None
        rows.append([
            str(idx), k,
            f"name-{idx}", f"budget-{idx}", f"owner-{idx}",
            deleted, ts(idx * 4),
        ])
    insert(table, cols, rows)


def gen_target_g(sys: str) -> None:
    table = f"{sys}_target_g"
    cols = ["id", "key_g", "meta", "fg3", "fg4", "fg5", "updated_at"]
    rows = []
    for idx, k in enumerate(G_KEYS, 1):
        meta = {
            "tier": f"tier-{idx}",
            "score": str(round(random.uniform(1, 100), 2)),
            "tags": [f"tag-{idx}-a", f"tag-{idx}-b"],
        }
        fg3 = {"nested": f"data-{idx}"}
        rows.append([
            str(idx), k, meta, fg3, idx % 2 == 0, f"loc-{idx}", ts(idx * 6),
        ])
    insert(table, cols, rows)


def gen_target_i(sys: str) -> None:
    table = f"{sys}_target_i"
    cols = ["id", "key_i", "fi1", "fi2", "fi3", "fi4", "fi5", "extra_col", "updated_at"]
    rows = []
    for idx, k in enumerate(I_KEYS, 1):
        rows.append([
            str(idx), k,
            D_KEYS[idx % len(D_KEYS)],  # fi1 refs target_d
            B_KEYS[idx % len(B_KEYS)],  # fi2 refs target_b
            f"amount-{idx}", f"currency-{idx}", f"memo-{idx}",
            f"extra-{sys}-{idx}",  # passthrough
            ts(idx * 2),
        ])
    insert(table, cols, rows)


def gen_target_j(sys: str) -> None:
    table = f"{sys}_target_j"
    cols = ["id", "key_j", "fj1", "fj2", "fj3", "fj4", "fj5", "updated_at"]
    rows = []
    for idx, k in enumerate(J_KEYS, 1):
        rows.append([
            str(idx), k,
            A_KEYS[idx % len(A_KEYS)],  # fj1 refs target_a
            f"subject-{idx}", f"detail-{idx}",
            f"priority-{idx}" if idx != 2 else None,  # test default_expression
            f"assignee-{idx}",
            ts(idx * 8),
        ])
    insert(table, cols, rows)


def main() -> None:
    w("-- Seed data for benchmark-large example")
    w("-- Generated by generate_seed.py")
    w(f"-- {len(SYSTEMS)} systems × {len(ROOT_TARGETS)} root targets = {len(SYSTEMS) * len(ROOT_TARGETS)} tables")
    w(f"-- Active systems with data rows: {', '.join(sorted(ACTIVE))}")
    w()

    for i, sys in enumerate(SYSTEMS, 1):
        if sys not in ACTIVE:
            continue
        w(f"-- ── {sys} ──")
        gen_target_a(sys)
        gen_target_b(sys)
        gen_target_d(sys)
        gen_target_e(sys)
        gen_target_f(sys, i)
        gen_target_g(sys)
        gen_target_i(sys)
        gen_target_j(sys)
        w()

    # Skeleton rows for inactive systems.
    # Tables that the views reference must have the right column types even
    # when empty.  A single row per table ensures PG infers JSONB / BOOLEAN
    # columns correctly.  Targets with only plain TEXT columns (b, d, i, j)
    # need a row too so the table exists with all expected columns.
    w("-- ── skeleton rows for inactive systems ──")
    for i, sys in enumerate(SYSTEMS, 1):
        if sys in ACTIVE:
            continue
        # target_a: items must be JSONB (used by child target_c)
        insert(f"{sys}_target_a",
               ["id", "key_a", "fa1", "fa2", "fa3", "fa4", "updated_at", "items"],
               [["0", f"_skel_{sys}_a", "", "", "", "", ts(), []]])
        # target_b
        insert(f"{sys}_target_b",
               ["id", "key_b", "fb1", "fb2", "fb3", "fb4", "updated_at"],
               [["0", f"_skel_{sys}_b", "", 0, "", "", ts()]])
        # target_d: needs dtype column
        insert(f"{sys}_target_d",
               ["id", "key_d", "fd1", "fd2", "fd3", "fd4", "fd5", "fd6", "dtype", "updated_at"],
               [["0", f"_skel_{sys}_d", "", "", "", "", "", "", "x", ts()]])
        # target_e: elements must be JSONB (used by child target_h)
        insert(f"{sys}_target_e",
               ["id", "key_e", "fe1", "fe2", "fe3", "fe4", "fe5", "fe6", "fe7", "updated_at", "elements"],
               [["0", f"_skel_{sys}_e", None, "", "", False, "", "", "", ts(), []]])
        # target_f: deleted_at for soft_delete
        insert(f"{sys}_target_f",
               ["id", "key_f", "ff1", "ff2", "ff3", "deleted_at", "updated_at"],
               [["0", f"_skel_{sys}_f", "", "", "", None, ts()]])
        # target_g: meta must be JSONB, fg3 JSONB, fg4 BOOLEAN
        insert(f"{sys}_target_g",
               ["id", "key_g", "meta", "fg3", "fg4", "fg5", "updated_at"],
               [["0", f"_skel_{sys}_g", {}, None, False, "", ts()]])
        # target_i: needs extra_col for passthrough
        insert(f"{sys}_target_i",
               ["id", "key_i", "fi1", "fi2", "fi3", "fi4", "fi5", "extra_col", "updated_at"],
               [["0", f"_skel_{sys}_i", "", "", "", "", "", "", ts()]])
        # target_j
        insert(f"{sys}_target_j",
               ["id", "key_j", "fj1", "fj2", "fj3", "fj4", "fj5", "updated_at"],
               [["0", f"_skel_{sys}_j", "", "", "", None, "", ts()]])
    w()

    total = len(SYSTEMS) * len(ROOT_TARGETS)
    active_count = len(ACTIVE)
    w(f"-- {active_count * len(ROOT_TARGETS)} tables with real data, {total - active_count * len(ROOT_TARGETS)} with skeleton rows")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
