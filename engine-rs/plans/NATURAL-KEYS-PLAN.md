# Natural keys

**Status:** Done

Investigation into whether the engine needs special handling for natural keys
(email addresses, business codes, composite business identifiers) versus
surrogate keys (auto-increment IDs, UUIDs).

**Conclusion:** No engine changes needed. Natural keys work correctly today.

---

## Key separation: row identity vs entity identity

The engine already separates two concerns that natural keys conflate in
traditional database design:

### 1. Row identification (`primary_key` → `_src_id`)

"Which row in this source table are we talking about?"

Always converted to text and flows through the pipeline as an opaque row
identifier. Could be `"42"`, `"alice@example.com"`, or
`'{"order_id":"ORD-100","line_no":1}'`. The engine doesn't interpret the
value — it's a stable address for the source row.

- **Single PK:** `id::text` → `"42"`
- **Composite PK:** `jsonb_build_object('line_no', line_no, 'order_id', order_id)::text` → `'{"line_no":1,"order_id":"ORD-100"}'`

### 2. Entity matching (`strategy: identity`)

"Which real-world thing does this row represent?"

Identity fields drive the transitive closure algorithm that merges rows across
systems into a single resolved entity. This is completely independent of the PK.

---

## Natural key patterns

Both patterns work identically in the engine:

### Pattern A: Natural key as PK + identity field

```yaml
sources:
  crm:
    primary_key: email          # row identification

targets:
  contact:
    fields:
      email:
        strategy: identity      # entity matching
      name:
        strategy: last_modified

mappings:
  - name: crm
    source: { dataset: crm }
    target: contact
    fields:
      - source: email
        target: email
      - source: name
        target: name
        last_modified: updated_at
```

Here `email` serves double duty: it's both the PK (`_src_id`) and the identity
field that drives entity matching.

### Pattern B: Surrogate PK, natural key as identity only

```yaml
sources:
  crm:
    primary_key: id             # row identification (surrogate)

targets:
  contact:
    fields:
      email:
        strategy: identity      # entity matching
      name:
        strategy: last_modified

mappings:
  - name: crm
    source: { dataset: crm }
    target: contact
    fields:
      - source: email
        target: email
      - source: name
        target: name
        last_modified: updated_at
```

Here `id` is the PK and `email` is only an identity field. The engine treats
both patterns uniformly — the PK is an opaque row reference, identity fields
drive matching.

---

## The one assumption: PK stability

The engine assumes `_src_id` is stable for the lifetime of a source row. If a
PK value changes, the engine sees it as a delete (old `_src_id` disappears)
plus an insert (new `_src_id` appears).

### When this matters

If `email` is the PK and Alice changes her email:

| Before | After |
|--------|-------|
| `_src_id = "alice@co.com"` | Row gone → looks like delete |
| — | `_src_id = "alice.new@co.com"` → looks like insert |

Since the email is also the identity field, the new row won't match the old
entity (different identity value), so this is genuinely a different entity from
the engine's perspective — correct behavior. The engine can't distinguish
"email changed" from "old user left, new user arrived" without CDC or a stable
surrogate.

### When this doesn't matter

If the source has a stable surrogate PK (`id = 42`) and the email is only an
identity field:

| Before | After |
|--------|-------|
| `_src_id = "42"`, email = `alice@co.com` | `_src_id = "42"`, email = `alice.new@co.com` |

The row identity is preserved (`_src_id` unchanged). The identity field change
causes the entity to split or re-link through transitive closure as appropriate.

---

## Existing examples using natural keys

| Example | PK | Natural? | Notes |
|---------|-----|----------|-------|
| `vocabulary-standard` | `name` ("Norway") | Yes | Country names as PK |
| `vocabulary-custom` | `crm_code` (integer) | Semi | Business code, stable |
| `composite-keys` | `[order_id, line_no]` | Yes | Business composite key |
| `merge-partials` | `invoice_id` ("INV1") | Yes | Business identifier |
| `hello-world` | `id` (integer) | No | Surrogate, identity via `email` |
| `references` | `id` (integer) | No | Surrogate, identity via `email`/`name` |

All work correctly without any natural-key-specific handling.

---

## Pipeline flow for natural keys

| Stage | `_src_id` role | Natural key behavior |
|-------|---------------|---------------------|
| **Forward** | Normalized to TEXT | `email::text` or `jsonb_build_object(...)::text` |
| **Identity** | Part of `md5(_mapping ':' _src_id ':' identity_fields)` | Opaque — value doesn't matter |
| **Resolution** | Grouped by `_entity_id_resolved` | PK not involved |
| **Reverse** | Extracted back to source columns | `id._src_id` or `(id._src_id::jsonb->>'col')` |
| **Delta** | `IS NULL` → insert, else compare `_base` | PK excluded from noop comparison |

Key detail: the delta **excludes PK columns from noop detection** (see
`action_case` in `delta.rs`). Only non-PK reverse-mapped fields are compared
against `_base`. This means a PK value can never trigger a spurious update.

---

## Recommendation

**Best practice:** If a natural key can change, use a stable surrogate as
`primary_key` and declare the natural key as a `strategy: identity` field.
This is standard database design advice, not an engine limitation.

The engine enforces no policy here — it works correctly with either approach.
