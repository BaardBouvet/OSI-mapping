# Scalar array extraction

Bare scalar JSONB arrays mapped via `scalar: true` on a child target field.

## Scenario

A CRM and an ERP both store customer tags as bare JSON arrays like
`["vip", "newsletter"]` — no object wrappers, just strings. Both map to the
same `customer_tag` child target. The engine merges tag sets across sources and
reconstructs bare scalar arrays in each source's delta.

## Key features

- **`scalar: true`** — extracts the bare value directly from each array element
  instead of a named key
- **`link_group`** — composite identity ensures tags match on both customer
  email and tag value
- Cross-source merge between two scalar array sources

## How it works

1. Both mappings declare `array: tags` with a single `scalar: true` field
2. The forward view unpacks each scalar element and uses the value as both the
   extracted data and the element identity
3. Identity resolution matches tags across sources by `(email, tag)` pairs
4. The delta reconstructs each source's array as bare scalars `["newsletter", "premium", "vip"]`

## When to use

- Source stores tags, labels, or categories as bare JSON arrays
- Source has a text[] or simple `["a", "b"]` column that needs per-element
  identity and cross-source merging
