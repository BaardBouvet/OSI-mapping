# Annotated Example

A complete mapping file with comments explaining every part. This example syncs contacts between a CRM, an ERP, and a shared phone directory — covering identity matching, multiple resolution strategies, expressions, and tests.

```yaml
# ─── Schema version (required, always "1.0") ───────────────────────
version: "1.0"

# ─── Human-readable summary (optional) ─────────────────────────────
description: >
  Three systems share a unified contact record.
  CRM is the authority on names. ERP is the authority on job titles.
  Phone directory provides formatted phone numbers.

# ─── Target entities ───────────────────────────────────────────────
# Defines WHAT the unified record looks like and HOW conflicts resolve.
# Keys are entity names referenced by mappings below.
targets:
  contact:
    fields:
      # IDENTITY — match key. Records from different sources with
      # the same email are merged into one unified contact.
      # Every target needs at least one identity field.
      email: identity

      # COALESCE — pick the best non-null value by priority.
      # Priority is set on individual field mappings (lower wins).
      name: coalesce

      # LAST_MODIFIED — most recently changed value wins.
      # Requires a timestamp on the mapping or field mapping.
      title: last_modified

      # EXPRESSION — custom SQL aggregation over all contributed values.
      # Must use the object form to provide the SQL expression.
      phone:
        strategy: expression
        expression: "max(phone)"    # picks lexicographically highest

# ─── Mappings ──────────────────────────────────────────────────────
# Each mapping connects ONE source dataset to ONE target entity.
mappings:

  # ── CRM mapping ──────────────────────────────────────────────────
  - name: crm                       # unique identifier (lowercase, underscores)
    source: { dataset: crm }        # source dataset name
    target: contact                  # references the target defined above

    # Mapping-level timestamp — applies to all fields using last_modified
    # strategy unless a field specifies its own.
    last_modified: updated_at

    fields:
      # Simple field copy: source field → target field.
      # No transform, no extra config.
      - source: email
        target: email

      # Coalesce field with priority 1 (highest — lower number wins).
      # When CRM and ERP both have a name, CRM's value is chosen.
      - source: full_name
        target: name
        priority: 1

      # Last_modified field — uses the mapping-level timestamp (updated_at).
      - source: job_title
        target: title

      # Expression with forward transform.
      # Normalizes phone format before contributing to the target.
      - source: phone
        target: phone
        expression: "regexp_replace(phone, '[^0-9+]', '')"

  # ── ERP mapping ──────────────────────────────────────────────────
  - name: erp
    source: { dataset: erp }
    target: contact
    last_modified: modified_date

    fields:
      - source: contact_email
        target: email

      # Lower priority (2) — only used when CRM doesn't have a name.
      - source: contact_name
        target: name
        priority: 2

      - source: position
        target: title

      - source: work_phone
        target: phone
        expression: "regexp_replace(work_phone, '[^0-9+]', '')"

  # ── Phone directory mapping ──────────────────────────────────────
  - name: phonebook
    source: { dataset: phonebook }
    target: contact

    fields:
      - source: email
        target: email

      # Forward-only computed field — no source field, just a constant.
      # Omitting source makes direction default to forward_only.
      # This contributes to "name" resolution but only as a last resort.
      - target: name
        expression: "'(from phonebook)'"
        priority: 99

      - source: formatted_phone
        target: phone

# ─── Tests ─────────────────────────────────────────────────────────
# Each test defines input data and expected output AFTER the full
# pipeline: forward transform → resolution → reverse transform.
tests:

  # Test 1: conflict resolution across all sources
  - description: "Alice exists in all three systems. CRM name wins (priority 1). ERP title wins (more recent). Phone resolved by max()."

    # Input: one array of rows per source dataset.
    # Keys must match mapping source.dataset names.
    input:
      crm:
        - id: "C1"
          email: "alice@example.com"
          full_name: "Alice Anderson"
          job_title: "Engineer"
          phone: "(555) 100-1000"
          updated_at: "2025-01-01T00:00:00Z"
      erp:
        - id: "E1"
          contact_email: "alice@example.com"
          contact_name: "A. Anderson"
          position: "Senior Engineer"
          work_phone: "555.200.2000"
          modified_date: "2025-06-15T00:00:00Z"
      phonebook:
        - id: "P1"
          email: "alice@example.com"
          formatted_phone: "+15553003000"

    # Expected: output per source dataset AFTER resolution.
    # Always an object with updates/inserts/deletes — never a bare array.
    expected:
      crm:
        updates:
          # CRM gets back its own row, but with the resolved title
          # from ERP (more recent) and resolved phone (max).
          - id: "C1"
            email: "alice@example.com"
            full_name: "Alice Anderson"     # kept (priority 1 winner)
            job_title: "Senior Engineer"    # updated from ERP
            phone: "+15553003000"           # max() across all sources
            updated_at: "2025-01-01T00:00:00Z"
      erp:
        updates:
          # ERP gets resolved name from CRM and resolved phone.
          - id: "E1"
            contact_email: "alice@example.com"
            contact_name: "Alice Anderson"  # updated from CRM
            position: "Senior Engineer"     # kept (most recent)
            work_phone: "+15553003000"      # resolved phone
            modified_date: "2025-06-15T00:00:00Z"
      phonebook:
        updates:
          - id: "P1"
            email: "alice@example.com"
            formatted_phone: "+15553003000" # resolved phone

  # Test 2: single-source pass-through + inserts to other systems
  - description: "Bob exists only in CRM, so values pass through unchanged except for phone normalization."
    input:
      crm:
        - id: "C2"
          email: "bob@example.com"
          full_name: "Bob Brown"
          job_title: "Manager"
          phone: "555-4000"
          updated_at: "2025-03-01T00:00:00Z"
      erp: []
      phonebook: []
    expected:
      crm:
        updates:
          - id: "C2"
            email: "bob@example.com"
            full_name: "Bob Brown"
            job_title: "Manager"
            phone: "5554000"
            updated_at: "2025-03-01T00:00:00Z"
      # ERP and phonebook get inserts — Bob is new to them.
      erp:
        inserts:
          - contact_email: "bob@example.com"
            contact_name: "Bob Brown"
            position: "Manager"
            work_phone: "5554000"
      phonebook:
        inserts:
          - email: "bob@example.com"
            formatted_phone: "5554000"
```

## What to notice

1. **Targets define the shape, mappings fill it in.** The `contact` target declares four fields with four different strategies. Each mapping contributes values to those fields independently.

2. **Priority is per-field, not per-source.** CRM has `priority: 1` on `name` but no priority on `phone`. Priority only matters for `coalesce` fields.

3. **Timestamp cascades.** `last_modified: updated_at` on the CRM mapping applies to all fields using the `last_modified` strategy. Per-field `last_modified` would override it if needed.

4. **Expressions are SQL.** The `regexp_replace` on phone fields is a forward transform. The `max(phone)` on the target is an aggregation across all contributed values.

5. **Tests are explicit.** Expected output is an object containing one or more of `updates`, `inserts`, and `deletes` (never a bare array). Omit keys when empty.

6. **Forward-only fields.** The phonebook's `name` mapping has no `source` — it's a constant contributed only during forward processing.
