# Value Maps — v2 Plan (RFC, Draft)

**Status:** committed to v2 \
**Depends on:** v2 spec (`v2-spec-draft.md`) \
**Targets:** v2 initial release \
**Origin:** ported from
[`BaardBouvet/opensync` `plans/engine/fields/PLAN_VALUE_MAP.md`](https://github.com/BaardBouvet/opensync/blob/main/plans/engine/fields/PLAN_VALUE_MAP.md)
(implemented there; tests `VM1`–`VM12`).

## § 1 Why this lands in v2

`value_map` does not exist in v1 OSI-mapping. It is a declarative,
bidirectional enum/code translation primitive that closes a real gap in
the v2 curated vocabulary: without it, every boolean-translation
(`"Y"`/`"N"` → `true`/`false`), every status-code mapping, and every
connector-specific code list forces the escape hatch — even though the
logic is paradigm-portable and bidirectional by construction.

It earns its own plan because:

1. **It is bidirectional by construction.** Forward `value_map` and (auto-
   derived or explicit) `reverse_value_map` make it one of the very few
   value-side transforms that can survive both directions cleanly. v2's
   bidirectionality filter rules out `concat`, `coalesce`, `if`/`case`, and
   most string normalisers; `value_map` and `cast` are the survivors.
2. **It is portable across both v2 backends.** `value_map` lowers cleanly
   to SQL (`CASE WHEN`), to a SPARQL `VALUES` table, and to any other
   backend trivially. There is no SQL-vs-SPARQL semantic gap to negotiate.
3. **It has its own design surface.** Bijectivity rules, fallback
   semantics, interaction with `default:` and `normalize:`, and direction
   guards all need explicit decisions that don't belong inside the v2
   transform/aggregate/predicate model.
4. **It is opt-in.** No existing v1 mapping uses it (empirical check across
   `examples/*/mapping.yaml`). Adopters get a cleaner expression for an
   existing pattern; non-adopters are unaffected.

## § 2 Problem

Different source systems use different codes for the same canonical
concept. CRM stores `status: "a"`, ERP stores `status: "1"`, both meaning
*active*. The canonical form should be a stable readable value (`"active"`)
independent of any single connector's representation.

In v2 today this is expressible only via the escape hatch:

```yaml
- source: status
  target: status
  transform:
    sql:    "CASE status WHEN 'a' THEN 'active' WHEN 'b' THEN 'inactive' WHEN 'c' THEN 'closed' END"
    sparql: "..."
```

Drawbacks:

- The reverse direction must be hand-maintained as a parallel `CASE WHEN`
  inside a `reverse_transform:` (or pair on the field).
- The structure is opaque to validation, visualisation, and tooling.
- Two backends mean two parallel `CASE WHEN` blocks that must agree.

A declarative `value_map` block solves all three.

## § 3 Proposed shape

Three field-level keys, all optional, additive on top of the v2 field
mapping schema:

```yaml
- source: status
  target: status
  value_map:
    'a': 'active'
    'b': 'inactive'
    'c': 'closed'
  reverse_value_map:        # optional; auto-derived when value_map is bijective
    'active':   'a'
    'inactive': 'b'
    'closed':   'c'
  value_map_fallback: passthrough   # passthrough (default) | null
```

### § 3.1 Forward pass

`value_map[source_value]` after the `default:` step. Null / undefined
values bypass the map entirely (consistent with v2 NULL = unknown
semantics — there is no canonical code for "unknown").

### § 3.2 Reverse pass

`reverse_value_map[canonical_value]`. Auto-derived at load time when
`value_map` is bijective. Many-to-one forward maps require an explicit
`reverse_value_map`; the loader emits a config warning if absent and uses
last-key-wins.

### § 3.3 Fallback

- `passthrough` (default): unmapped values flow through unchanged.
- `null`: unmapped values become null (treat unknown code as unknown).

### § 3.4 Mutual exclusion

`value_map` and `transform:` are mutually exclusive on the same field
mapping (loader error). This keeps "what does this field do" answerable by
inspecting one key.

### § 3.5 Direction guard

`direction: forward_only` / `reverse_only` applies before the map step,
exactly like v2's other field-level properties.

### § 3.6 Interaction with `normalize:`

`value_map` is a **value transform** and runs at mapping time on the raw
source value. `normalize:` is a **comparison adapter** and runs at diff
time on already-mapped canonical values. Both can be declared on the same
field; they don't see each other's output. This is identical to opensync's
VM12 test.

### § 3.7 Backend rendering

| Backend | Forward | Reverse |
|---|---|---|
| PG views | `CASE` expression generated from the dict | Same on the reverse view |
| SPARQL | `VALUES` block bound on `?source ?canonical` | Same with bindings flipped |

The renderer generates the `CASE` / `VALUES` form from the YAML dict; the
mapping author never writes either by hand.

## § 4 Out of scope (deferred further)

- Named / shared maps (`value_maps:` block + `value_map: <name>` lookup).
  YAML anchors handle the realistic reuse cases.
- Case-insensitive lookup. Trivially layered later.
- Numeric key coercion: keys are coerced via `String(k)` before lookup;
  this handles YAML's int parsing transparently.

## § 5 Tests (planned)

Modelled directly on opensync's VM1–VM12. Adapt to OSI's test format
(input dicts per source, expected canonical / reverse output, and the
no-op suppression cases for VM12).

## § 6 Open questions

- **Schema location.** Field-level (per opensync) is the obvious choice.
  Confirm before implementation.
- **Loader-time auto-inversion warnings.** Match opensync (`console.warn`
  + last-wins) or upgrade to a hard error and require explicit
  `reverse_value_map` on every non-bijective forward map?
- **Renderer-specific quoting.** SPARQL `VALUES` and SQL `CASE` literals
  need consistent type coercion — likely deferred to renderer
  implementation, not the spec.

## § 7 Status

- v1 OSI: not present.
- v2 OSI: not present (deliberately; v2 ships escape hatch only on the
  value-transform side).
- Post-v2: this plan.
- opensync: implemented and tested (`packages/engine/src/core/mapping.ts`
  `applyMapping()`, schema in `packages/engine/src/config/schema.ts`,
  loader in `packages/engine/src/config/loader.ts`, tests
  `packages/engine/src/core/mapping.test.ts` VM1–VM12).
