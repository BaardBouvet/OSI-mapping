# Element-level soft delete

Detect and suppress soft-deleted elements in nested JSONB arrays via
`soft_delete` on a child mapping.

## Scenario

A billing system stores invoice line items as a JSONB array. Each element
has a `voided_at` timestamp — when set, the line is considered voided.
An accounting system shares the same invoices. Voided lines should not
appear in either system's reconstructed output.

## Key features

- **`soft_delete: voided_at`** on a child mapping — detects voided elements
  by checking if `voided_at IS NOT NULL`
- Voided elements are excluded from the reconstructed array in all sources'
  delta output
- Active elements (where `voided_at` is null) flow normally

## How it works

1. The billing child mapping declares `soft_delete: voided_at`
2. In the forward view, soft-deleted elements have their non-identity fields
   nulled so they cannot win field resolution
3. The delta CTE `_del_ts_lines_0` identifies soft-deleted elements by
   checking `voided_at IS NOT NULL` in the billing reverse view
4. The `_del_lines` CTE filters those elements out from the `jsonb_agg`
   reconstruction — both billing's and accounting's reconstructed arrays
   exclude the voided line

## When to use

- Source has soft-deletable array elements (voided invoice lines, archived
  tasks, disabled options)
- The deletion signal lives inside the JSONB element, not at the parent level
