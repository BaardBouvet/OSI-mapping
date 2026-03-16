# Target path (dotted target properties)

**Status:** Design

Analysis of supporting dotted notation on the target side (`target_path`),
where multiple field mappings compose into a single JSONB column on the target.

## Concept

Symmetric counterpart to `source_path`. Where `source_path: metadata.tier`
**extracts** a sub-field from a source JSONB column, `target_path: address.city`
would **compose** multiple scalar resolutions into a target JSONB column.

```yaml
targets:
  customer:
    fields:
      name: { strategy: coalesce }
      address:               # JSONB column on the target
        strategy: coalesce    # ... but what does coalesce mean for a composite?

mappings:
  - name: crm
    source: { dataset: crm }
    target: customer
    fields:
      - source: name
        target: name
      - source: street
        target_path: address.street
      - source: city
        target_path: address.city
      - source: zip
        target_path: address.zip
```

Desired output: `address` column contains `{"street": "...", "city": "...", "zip": "..."}`.

## How source_path works today (for comparison)

```
Source JSONB column → source_path extraction → scalar target fields → resolution → reverse → delta JSONB reconstruction
```

1. **Forward:** `source_path: metadata.tier` → `"metadata"->>'tier'` (JSONB → scalar)
2. **Resolution:** operates on the scalar `tier` column — standard strategy
3. **Reverse:** column aliased as `"metadata.tier"` (the dotted source name)
4. **Delta:** `JsonNode` tree reconstructs `jsonb_build_object('tier', ...)` back to JSONB

Each scalar travels independently through resolution. Reconstruction is a
post-resolution formatting step.

## The fundamental question

With `target_path: address.city`, at which pipeline stage does JSONB
composition happen?

### Option A — Late composition (in delta/analytics views)

Target fields remain scalar throughout forward → resolution → reverse.
Only the **output** layer composes them into JSONB.

```
forward: street, city, zip (scalars)
resolution: street, city, zip (each resolves independently)
reverse: street, city, zip (each mapped back independently)
delta: jsonb_build_object('street', street, 'city', city, 'zip', zip) AS address
```

**This is essentially what we can do today** without any engine changes —
just declare flat target fields and add a SQL view on top that wraps them.

### Option B — Early composition (in forward/resolution views)

The forward view emits `address` as a single JSONB column. Resolution
operates on the composite.

```
forward: jsonb_build_object('street', street, 'city', city, 'zip', zip) AS address
resolution: ??? (coalesce on JSONB?)
reverse: address->>'street' AS street, address->>'city' AS city, ...
```

**Problem:** Resolution strategies don't make sense for composite values.
What does `coalesce` mean for a JSONB object? Take the whole object from the
highest-priority source? That loses per-field granularity — CRM might have
the best street but ERP the best city.

## Analysis

### Pro: target_path

| Benefit | Detail |
|---------|--------|
| Symmetric with source_path | Consistent mental model — dotted paths on both sides |
| Clean target schema | Consumer sees `address` JSONB column, not 3 flat fields |
| Self-documenting | YAML shows the intended structure |

### Con: target_path

| Issue | Detail |
|---------|--------|
| Resolution granularity conflict | Strategies operate per-field. If `address` is one field, you lose per-sub-field resolution control. If sub-fields resolve independently, `target_path` is just cosmetic output formatting. |
| Noop detection complexity | Comparing `address` JSONB values requires deep equality or decomposition back to scalars |
| Reverse mapping ambiguity | When reversing `address` JSONB back to source, need to decompose — same problem as source_path but in reverse |
| Direction mismatch | `source_path` decomposes (JSONB → scalars) at the START of the pipeline where it's natural. `target_path` composes (scalars → JSONB) — but at which stage? If at the end, it's just output formatting. |
| Group semantics overlap | `group:` already handles "these fields must resolve together." Adding target_path-as-group creates two ways to express the same thing. |
| Partial updates | If only `city` changes, does the delta emit the full `address` JSONB or just the changed path? Consumers may expect either. |

### The "just formatting" observation

If each sub-field resolves independently (which is the natural and desirable
behavior), then `target_path` does exactly one thing: in the final output view,
wrap related scalar columns into a `jsonb_build_object`. This is:

1. **~20 lines of engine code** (mirror the `delta_output_exprs` JsonNode tree
   but for target output)
2. **Achievable today with a SQL view** on top of the delta/analytics output
3. **Not a pipeline concern** — it doesn't affect forward, identity, resolution,
   or reverse

### What the user probably wants

The likely motivation is consuming the resolved output via an API or BI tool
that expects structured JSON. The current flat-column output requires
post-processing. This is valid but may be better solved by:

1. **Analytics view layer** (already planned in ANALYTICS-PROVENANCE-PLAN) —
   add JSONB composition in the analytics/output view
2. **Output schema declaration** — a separate `output:` section that declares
   how flat target fields map to a consumer-facing structured schema

### Comparison with alternatives

| Approach | Engine changes | Resolution | Noop | Reverse |
|----------|---------------|------------|------|---------|
| Flat fields + SQL view on top | None | Per-field ✓ | Simple ✓ | Simple ✓ |
| `target_path` (late, output only) | ~20 lines | Per-field ✓ | Must decompose | Must decompose |
| `target_path` (early, composite) | ~100+ lines | Broken ✗ | Complex ✗ | Complex ✗ |
| `group:` + output formatting | Formatting only | Per-field ✓ | Simple ✓ | Simple ✓ |

## Recommendation

**Don't add `target_path` to the core pipeline.** The resolution/reverse/noop
machinery works because target fields are scalars. Compositing into JSONB is
an output concern.

Two cleaner paths:

### Path 1 — Do nothing (recommend a view)

Document that consumers wanting structured JSON should create a SQL view:

```sql
CREATE VIEW customer_structured AS
SELECT
  name,
  jsonb_build_object(
    'street', street,
    'city', city,
    'zip', zip
  ) AS address
FROM _delta_customer;
```

Zero engine changes. Fully flexible. Consumer controls the shape.

### Path 2 — Output formatting in analytics view (future)

When the analytics view layer is built (ANALYTICS-PROVENANCE-PLAN), add an
optional `output:` section to target definitions:

```yaml
targets:
  customer:
    fields:
      name: { strategy: coalesce }
      street: { strategy: coalesce }
      city: { strategy: coalesce }
      zip: { strategy: coalesce }
    output:
      address:
        street: street
        city: city
        zip: zip
```

This separates concerns: `fields:` defines resolution (always scalar),
`output:` defines consumer-facing shape (can be structured). The analytics
view emits `jsonb_build_object(...)` based on `output:` declarations.

This gives the "dotted target" UX without corrupting the resolution pipeline.
