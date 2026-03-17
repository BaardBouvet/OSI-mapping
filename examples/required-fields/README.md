# Required Fields

## Scenario

Two source systems — a CRM and a public registry — feed an `organisation`
target.  An organisation record is only meaningful when it carries at least
one business identifier: a **name** or an **organisation number**.  Records
that have *neither* should not be synced back to any source.

## Key features

| Feature | Purpose |
|---|---|
| `reverse_filter` with `OR` | Ensures at least one of several fields is present before a record is reverse-synced |
| Shared filter across mappings | Both mappings use the same business rule, keeping the constraint consistent |
| Suppression via delete | Records that violate the constraint appear as deletes in the reverse view |

## How it works

Each mapping declares a `reverse_filter`:

```yaml
reverse_filter: "name IS NOT NULL OR org_number IS NOT NULL"
```

During reverse-view generation the engine wraps this filter in the
`WHERE` clause.  Only rows that satisfy the condition are included in
the reverse result set.  Rows that fail (both fields `NULL`) are
emitted as deletes, signalling that the source record should be
removed (or flagged) because it no longer meets the minimum data
requirements.

## When to use

* **Minimum-data rules** — a record must have *at least one* of N
  identifying fields before it may flow back to sources.
* **Soft required fields** — unlike a database `NOT NULL` constraint,
  `reverse_filter` is evaluated at sync time and does not block
  inserts into the golden record.
* **Cross-source consistency** — apply the same filter to every mapping
  that touches the target so no source receives incomplete records.
