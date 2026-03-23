# External identity links

Cross-reference table merges records across systems via `links` and `link_key`.

## Scenario

Two CRM systems (CRM A and CRM B) each have their own customer tables with
no shared identity field. An MDM (Master Data Management) system produces a
cross-reference table mapping CRM A IDs to CRM B IDs, with a pre-computed
cluster ID from a matching algorithm.

## Key features

- **`links`** — declares which columns in the linking table reference which
  source mappings, creating identity edges
- **`link_key: cluster_id`** — uses the pre-computed cluster ID from the xref
  table instead of computing it from pairwise edges (IVM-safe path)
- Linkage-only mapping (no `fields`) — contributes identity edges without
  business data

## How it works

1. CRM A and CRM B each map their customer fields normally
2. The mdm_links mapping declares no fields — only `links` and `link_key`
3. The `links` entries create identity edges: `crm_a_id → crm_a_customers`
   and `crm_b_id → crm_b_customers`
4. The `link_key: cluster_id` column provides the pre-computed cluster
   membership, so the engine doesn't need to compute connected components
5. Records from CRM A and CRM B that share a cluster are merged — CRM A's
   name wins via priority 1

## When to use

- An external matching service (ML model, MDM platform, manual curation)
  produces cross-references between systems
- Systems have no shared natural key (like email) for identity matching
- Pre-computed cluster IDs are available for IVM-safe resolution
