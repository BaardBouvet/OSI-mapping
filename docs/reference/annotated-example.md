# Annotated Example

A complete v2 mapping file with inline comments explaining every part.
This is the `hello-world` example: two systems sharing a contact, synced by email.

→ Runnable version: [`examples/hello-world/mapping.yaml`](../../examples/hello-world/mapping.yaml)

---

```yaml
# ─── Schema version ────────────────────────────────────────────────
# Required. Always the string "2.0".
version: "2.0"

# ─── Human-readable summary ────────────────────────────────────────
# Optional. Describes what this mapping file does.
description: >
  Two systems, one shared contact, synced by email.
  CRM is the authority on names (priority 1).

# ─── Source datasets ───────────────────────────────────────────────
# One entry per physical source. The key is the source name;
# it must match the table or dataset name in your system.
# primary_key is the column the engine uses to identify rows.
sources:
  crm:
    primary_key: id
  erp:
    primary_key: id

# ─── Target entities ───────────────────────────────────────────────
# Defines what the unified model looks like.
# Keys are entity names. Each entity declares:
#   identity: which field(s) to use as the merge key
#   fields:   what properties the entity has and how conflicts resolve
targets:
  contact:

    # IDENTITY: the merge key. Two rows from different sources with
    # the same email value will be merged into one canonical contact.
    identity:
      - email

    # FIELDS: the canonical properties of this entity.
    # Each field declares a resolution strategy.
    fields:
      # COALESCE: picks the highest-priority non-null value.
      # Priority is set on the field mapping below (lower wins).
      email: { strategy: coalesce }
      name:  { strategy: coalesce }

# ─── Mappings ──────────────────────────────────────────────────────
# Each mapping connects one source to one target.
# Fields are listed as source column → target field pairs.
mappings:

  # ── CRM mapping ──────────────────────────────────────────────────
  - name: crm               # unique mapping name — lowercase, underscores
    source: crm             # must match a key in sources:
    target: contact         # must match a key in targets:
    fields:
      # source: the column name in the CRM source table
      # target: the field name in the contact target entity
      - { source: email, target: email }

      # priority: 1 means "CRM wins" when both sources have a name
      # (lower priority number wins in coalesce)
      - { source: name, target: name, priority: 1 }

  # ── ERP mapping ──────────────────────────────────────────────────
  - name: erp
    source: erp
    target: contact
    fields:
      # ERP uses different column names for the same logical fields
      - { source: contact_email, target: email }

      # priority: 2 means ERP's name is used only when CRM has no name
      - { source: contact_name, target: name, priority: 2 }

# ─── Tests ─────────────────────────────────────────────────────────
# Inline test cases. Each test specifies input rows and the expected
# delta output. The engine verifies both PG and SPARQL backends
# produce exactly these deltas.
tests:
  # ── Test 1: shared contact, CRM name wins ────────────────────────
  - description: "Shared contact — CRM name wins (priority 1), ERP gets updated"
    input:
      # Current state of the CRM source
      crm:
        - { id: "1", email: "alice@example.com", name: "Alice" }
      # Current state of the ERP source
      erp:
        - { id: "100", contact_email: "alice@example.com", contact_name: "A. Smith" }
    expected:
      # The engine computes: canonical name is "Alice" (CRM, priority 1).
      # ERP's current value is "A. Smith" — it should be updated.
      erp:
        updates:
          # The update row matches the source's primary key (id) and
          # shows all mapped fields at their resolved canonical values.
          - { id: "100", contact_email: "alice@example.com", contact_name: "Alice" }
      # CRM has no expected deltas (its values already match canonical).
      # Omitting a source from expected: is equivalent to asserting {}.

  # ── Test 2: CRM-only contact → insert into ERP ───────────────────
  - description: "CRM-only contact triggers insert into ERP"
    input:
      crm:
        - { id: "1", email: "alice@example.com", name: "Alice" }
        - { id: "2", email: "bob@example.com",   name: "Bob" }
      erp:
        - { id: "100", contact_email: "alice@example.com", contact_name: "A. Smith" }
    expected:
      erp:
        updates:
          - { id: "100", contact_email: "alice@example.com", contact_name: "Alice" }
        inserts:
          # Bob exists in CRM but not in ERP — ERP needs an insert.
          # id is null because no ERP row exists yet.
          # _canonical_id identifies the source entity that owns the canonical:
          # "crm:2" = source "crm", pk value "2".
          - { _canonical_id: "crm:2", id: null, contact_email: "bob@example.com", contact_name: "Bob" }

  # ── Test 3: ERP-only contact → insert into CRM ───────────────────
  - description: "ERP-only contact triggers insert into CRM"
    input:
      crm:
        - { id: "1", email: "alice@example.com", name: "Alice" }
      erp:
        - { id: "100", contact_email: "alice@example.com", contact_name: "A. Smith" }
        - { id: "200", contact_email: "carol@example.com", contact_name: "Carol" }
    expected:
      crm:
        inserts:
          # Carol is ERP-only — CRM needs an insert.
          - { _canonical_id: "erp:200", id: null, email: "carol@example.com", name: "Carol" }
      erp:
        updates:
          - { id: "100", contact_email: "alice@example.com", contact_name: "Alice" }
```

---

## How it works

**Lift.** Each source row is expanded into the RDF/relational model using
the mapping's field list. ERP's `contact_email` becomes the canonical
`email` field; `contact_name` becomes `name`.

**Identity closure.** Rows from CRM and ERP with the same `email` value
are assigned the same canonical IRI (`contact/<sha256(email)>`). This
is the merge step — no pre-shared IDs required.

**Forward resolution.** For each canonical entity, the `name` field is
resolved using `coalesce`: the highest-priority non-null value wins.
CRM has `priority: 1` so "Alice" beats "A. Smith".

**Reverse projection.** Each source gets a reverse view of the canonical
state in its own shape. ERP's reverse row looks like an ERP row, but
with `contact_name` filled from the canonical `name`.

**Delta computation.** The reverse view is compared against the source's
current rows. Differences become `updates`, missing entities become
`inserts`, and source rows with no matching canonical entity become
`deletes`.

---

## See also

- [Schema reference](schema-reference.md) — complete property reference
- [`examples/composite-identity/`](../../examples/composite-identity/) — AND-tuple identity
- [`examples/last-modified/`](../../examples/last-modified/) — timestamp-based resolution
- [`examples/nested-arrays-shallow/`](../../examples/nested-arrays-shallow/) — one-to-many nested arrays
