# sparql-spike

Throwaway spike validating the v2 SPARQL renderer contract from
[`docs/design/triplestore-backend.md`](../docs/design/triplestore-backend.md).

**Goal:** prove the SPARQL pipeline (lift → identity → forward → reverse →
project) reproduces the same `updates` / `inserts` / `deletes` records
that the SQL renderer produces for a small set of v2 examples. If it
does, the spec's "two backends, one schema" claim is sound and we can
implement both renderers properly. If it doesn't, we've found the spec
gap before committing to a full SQL v2 renderer.

**Scope:** `examples/hello-world` test 1 first, then `merge-threeway`,
then one nested-array example. Each example is hardcoded in Rust — no
YAML parser, no model abstraction. The point is the SPARQL pipeline,
not the engine architecture.

**When to delete:** once the spike has answered yes-or-no on the contract
question, throw it away and implement both renderers in `engine-rs`
using a backend-neutral model.

## Run

```
cd sparql-spike
cargo run
```

Each test prints `PASS` or `FAIL <reason>`. The spike exits non-zero if
any test fails.
