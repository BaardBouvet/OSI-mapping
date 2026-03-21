# Scalar array element deletion

Cross-source deletion of bare scalar JSONB array elements via `scalar: true` and `derive_tombstones`.

## Scenario

Two CRM systems store contact tags as bare scalar JSONB arrays (`["vip", "churned", "newsletter"]`). Each source's tags are modelled as a child mapping to a shared `tag_entry` target. When CRM A removes a tag, the engine detects the absence via the parent's `written_state` and synthesizes `is_removed = TRUE`. The deletion propagates cross-source — CRM B's delta also excludes the removed tag.

## Key features

- **`scalar: true`** — extracts the value directly from a bare JSONB array element instead of a named key. The delta reconstructs the nested array as a scalar list (`["vip", "newsletter"]`) rather than an object list.
- **`derive_tombstones: is_removed`** — detects elements that were previously written but are now absent, and synthesizes a boolean tombstone flag.
- **`written_state: true`** — records the parent mapping's last-written output so `derive_tombstones` can compare current forward output against previous state.

## How it works

1. The parent mapping (`crm_a_contacts`) has `written_state: true`, storing the full nested output including the `tags` scalar array.
2. The child mapping (`crm_a_tags`) uses `scalar: true` on the `tag` field to extract bare values from the `tags` JSONB array.
3. When CRM A's current tags (`["vip", "newsletter"]`) differ from the written state (`["vip", "churned", "newsletter"]`), the engine detects that `"churned"` is absent and emits a tombstone row with `is_removed = TRUE`.
4. The `bool_or` resolution strategy on `is_removed` means any source marking a tag as removed wins.
5. CRM B's delta view excludes the removed tag from the reconstructed array.

## When to use

Use this pattern when source systems store tags, labels, or other bare scalar arrays and you need cross-source deletion propagation without explicit delete flags in the source data.
