# Element-level deletion (deletion-wins)

When one source removes a nested array element, the removal "wins"
over other sources that still contribute the element. The engine
detects this by comparing each source's current forward view against
previously-written JSONB from any parent mapping with `written_state`.
The deletion propagates across all sources' delta outputs.

## Scenario

Two sources (`recipe_db` and `blog_cms`) both contribute recipe steps.
The engine merges them by identity (instruction name) and resolves
duration via coalesce. Between sync cycles, `blog_cms` removes a step
but `recipe_db` still has it.

`blog_cms_recipes` declares `written_state: true` and `derive_noop: true`.
The ETL writes the full resolved object (including nested arrays) to the
`_written` table after each sync.

Previously written for blog_cms: Preheat(10), Mix(5), Bake(30), Sift flour(7).
Current blog_cms: Preheat(10), Mix(5), Bake(30) — dropped "Sift flour".
Current recipe_db: Preheat(10), Mix(5), Bake(30), Sift flour(7) — unchanged.

Without deletion-wins, the resolved array still includes "Sift flour"
(contributed by recipe_db) and recipe_db's delta says `noop`. With
deletion-wins, the engine sees that blog_cms previously had "Sift flour"
(in the written state) but now doesn't (not in forward view), so it
excludes the element from **all** sources' reconstructed arrays. The
recipe_db delta says `update` with only 3 steps.

## Key features

- **`written_state: true`** on a parent mapping — stores the complete
  object including arrays after each ETL sync cycle
- **Cross-source deletion-wins** — if any source removes an element
  that was in its written state, the removal propagates to all deltas
- **No new views or surface** — works through the regular `_delta_` view
- **Derived tombstones** — deletions are inferred from absence, not
  explicit markers

## How it works

1. A parent mapping declares `written_state: true`.
2. The ETL writes the full resolved object (including nested arrays) to
   `_written_{parent}` after each sync.
3. On the next engine run, for each child array segment, the delta
   scans ALL parent mappings with `written_state`:
   - `_del_prev_{segment}_N`: extracts elements from that parent's
     written JSONB array
   - `_del_curr_{segment}_N`: gets current elements from that source's
     forward view
   - `_del_src_{segment}_N`: elements in prev but not in curr = deletions
4. Per-source deletions are UNIONed into `_del_{segment}`.
5. The nested `jsonb_agg` CTE LEFT JOINs the combined deletion CTE and
   excludes deleted elements from the reconstructed array.
6. For the source that declared `derive_noop`, the noop comparison
   (both `_base` and `_written`) detects the resulting array change.

## Footguns

### Asymmetric element contributions (false deletions)

The written state stores the **resolved** output, not per-source contributions.
If source A contributes element X but source B never did, element X appears
in the written JSONB for both sources (since it's the merged result). When
comparing source B's forward view against the written state, X is "absent" —
it looks like source B deleted it, when in fact B never had it.

**Mitigation**: this only causes problems when sources contribute **different**
elements to the same array. When all sources contribute the same elements
(the typical case for synchronized data), the detection is accurate.

**Future**: per-source contribution tracking (storing each source's forward
view output separately) would eliminate false positives at the cost of a
more complex written-state contract.

### First sync (no written state yet)

Before the ETL has written the first `_written` row, there is no previous
state to compare. No deletion CTEs fire — the delta produces the full
resolved array. This is correct: there can be no "deletions" on the first
sync since nothing was written before.

### Stale written state

If the ETL fails to update the written state after a sync, the next run
compares against outdated data. Previously-written elements that the ETL
actually synced but forgot to record might be flagged as deletions again.
The ETL must reliably write the `_written` row after each sync.

## When to use

- Multiple sources contribute to the same nested array elements.
- Sources silently drop array elements (no tombstone record).
- You want a single source's removal to propagate even when other sources
  still contribute the element.
- All sources contribute roughly the same set of elements (avoids the
  asymmetric-contributions footgun).
