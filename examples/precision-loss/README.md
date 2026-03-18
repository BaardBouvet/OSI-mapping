# Precision Loss (`normalize`)

Shows how `normalize` prevents false delta updates when a target system
has lower precision than the golden record, and how echo-aware resolution
prevents a low-precision value from degrading the golden record.

- **System A** stores decimal prices and full-length names.
- **System B** stores integer prices and names truncated to 10 characters.

The `normalize` expression on System B's field mappings reduces both sides
of the noop comparison to the system's actual resolution.  The engine
recognises "12" vs "12.50" as expected precision loss (noop) rather than a
change requiring an update.

When both systems use `last_modified` strategy, the engine also detects
that System B's rounded value is an echo of System A's and keeps the
higher-precision value as the golden record.

## Key fields

```yaml
- source: price
  target: price
  normalize: "trunc(%s::numeric, 0)::integer::text"

- source: name
  target: name
  normalize: "left(%s, 10)"
```
