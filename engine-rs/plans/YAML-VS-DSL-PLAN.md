# YAML vs custom DSL

**Status:** Design

Should the mapping format remain YAML, or should we design a custom DSL
with its own parser?

## Context

The mapping format is a YAML document with ~62 distinct properties across
6 nesting levels, validated by a JSON Schema. The format is declarative:
it describes relationships between sources and targets, field mappings,
resolution strategies, and test cases. The engine parses this YAML and
generates SQL views.

This analysis examines whether a custom DSL would be worth the investment,
or whether YAML's ecosystem advantages outweigh its verbosity.

## Prior art: how adjacent tools chose

### Terraform → HCL

HashiCorp originally used JSON for Terraform configurations. They created
HCL (HashiCorp Configuration Language) because:

- JSON lacks comments, trailing commas, and multi-line strings.
- Infrastructure-as-code needed **blocks** (resource types) and
  **references** (interpolation between resources). JSON's flat key-value
  model created deep nesting for these patterns.
- HCL added block syntax, interpolation (`${var.name}`), functions, and
  for-expressions — all impossible in standard JSON.

**Lesson:** HCL was justified because the expressiveness gap between JSON
and the domain was large. Terraform needed computed values, conditional
resources, and loops — none of which JSON or YAML can express.

### Prisma → Prisma Schema Language

Prisma replaced YAML/JSON schema definitions with a custom DSL because:

- Data modeling is inherently relational — `model User { posts Post[] }`
  reads like the domain, not like a serialization format.
- Relations (one-to-many, many-to-many) needed dedicated syntax.
- Enums, defaults, and field modifiers (`@id`, `@unique`, `@default`) are
  first-class concepts better served by annotations than YAML nesting.

**Lesson:** Prisma's domain (data models with relations) maps cleanly to a
block-and-annotation syntax. The DSL made the common case (defining a model
with relations) more concise. But Prisma also still supports a JSON Schema
representation for programmatic generation.

### dbt → SQL + Jinja + YAML

dbt uses SQL for models (the core logic), Jinja for templating (the meta-
logic), and YAML for project config and properties (schema.yml). This hybrid
approach works because:

- The core logic (transformations) is already SQL — no point wrapping it.
- Metadata (tests, descriptions, tags) is structural — YAML fits.
- Cross-model references (`{{ ref('model') }}`) need interpolation — Jinja.

**Lesson:** dbt succeeded by NOT creating a DSL. SQL is the DSL for
transformations; YAML is the DSL for metadata. The Jinja layer bridges them.
This only works because dbt's users are SQL experts.

### CUE Language

CUE is a constraint-based configuration language that unifies types, values,
and validation in one syntax. It was designed to replace YAML+JSON Schema
with a single language that is both data and schema.

- CUE schemas and data look identical — schemas are just more general values.
- Validation is built-in, not bolted on via a separate JSON Schema.
- CUE is Turing-incomplete by design — no loops, no side effects.

**Lesson:** CUE solves real YAML problems (validation, composition, defaults)
but adoption is limited because the learning curve is steep and tool support
is thin compared to YAML + JSON Schema.

### Dhall

Dhall is a programmable configuration language with types, functions, and
imports — but guaranteed to terminate (no general recursion). It's used as
a YAML/JSON replacement when configuration needs computed values.

**Lesson:** Dhall solves the "configuration needs logic" problem, but most
configuration doesn't need logic. Its adoption is niche because YAML + a
validator covers 90% of use cases.

### Summary of prior art

| Tool | Format | Why |
|------|--------|-----|
| Terraform | Custom DSL (HCL) | JSON couldn't express computed values, conditionals, loops |
| Prisma | Custom DSL | Relational modeling maps to block+annotation syntax |
| dbt | SQL + Jinja + YAML | Core logic is already SQL; metadata is structural |
| CUE | Custom language | Unifies schema + data; steep learning curve |
| Dhall | Custom language | Config needs computation; niche adoption |
| Kubernetes | YAML + JSON Schema | Structural config; massive ecosystem leverage |
| Ansible | YAML | Task sequences; familiarity > conciseness |
| GitHub Actions | YAML | Workflow definition; ecosystem wins |

**Pattern:** Custom DSLs succeed when the domain's expressiveness needs
significantly exceed what YAML can represent (computed values, loops,
relations as first-class concepts). When the format is purely declarative
and structural, YAML + JSON Schema wins on ecosystem and familiarity.

## The case for a custom DSL

### 1. Conciseness

A DSL could reduce mapping files by ~30-40%:

```
-- YAML (19 lines)
mappings:
  - name: crm_contact
    source: { dataset: crm }
    target: contact
    last_modified: updated_at
    fields:
      - { source: email, target: email }
      - source: full_name
        target: first_name
        expression: "SPLIT_PART(full_name, ' ', 1)"
      - source: full_name
        target: last_name
        expression: "SPLIT_PART(full_name, ' ', 2)"
      - source: phone
        target: phone
        direction: forward_only

-- DSL (11 lines)
mapping crm_contact: crm -> contact {
  last_modified: updated_at
  email -> email
  full_name -> first_name  = SPLIT_PART(full_name, ' ', 1)
  full_name -> last_name   = SPLIT_PART(full_name, ' ', 2)
  phone -> phone  (forward_only)
}
```

### 2. Domain-specific syntax for common patterns

```
-- References as first-class syntax
order_system_id -> purchase_order_id  references purchase_order.system_id

-- Parent-child nesting as block nesting
mapping orders: erp -> purchase_order {
  ...
  nested lines from lines {
    line_num -> line_number
    ...
  }
}

-- Resolution strategy inline
target contact {
  email: identity
  name: coalesce
  score: expression = max(score)
}
```

### 3. Better error messages

A custom parser can produce domain-specific error messages:

```
error: unknown target 'contacts' in mapping 'crm_contact'
  --> mapping.osi:12:3
   |
12 |   mapping crm_contact: crm -> contacts {
   |                                ^^^^^^^^ did you mean 'contact'?
```

vs. the current generic YAML parse error followed by a separate validation
error.

### 4. Inline expressions without quoting

In YAML, SQL expressions must be quoted strings:
```yaml
expression: "SPLIT_PART(name, ' ', 1)::text"
filter: "status = 'active'"
```

In a DSL, expressions can be first-class syntax:
```
full_name -> first_name = SPLIT_PART(full_name, ' ', 1)::text
filter: status = 'active'
```

No quoting, no escaping, no YAML string type ambiguity.

## The case for staying with YAML

### 1. Ecosystem and tooling

YAML has universal support:

- **Every editor** has YAML syntax highlighting, indentation, and folding.
- **JSON Schema** provides autocompletion, hover docs, and inline validation
  in VS Code (via the YAML extension + our `mapping-schema.json`).
- **Every language** has a YAML parser — Python, JavaScript, Ruby, Go, Java,
  C#, Rust. A custom DSL needs parsers for every language that might consume
  mappings.
- **CI/CD tools** (GitHub Actions, GitLab CI, etc.) natively understand YAML.
- **Linters** (yamllint), formatters (prettier), and diff tools work out of
  the box.
- **LLMs** generate valid YAML reliably. A custom DSL needs training data.

### 2. Familiarity

YAML is the lingua franca of DevOps, data engineering, and modern
infrastructure. Every potential user already knows it. A custom DSL has a
learning curve — no matter how intuitive the syntax, users must learn the
grammar, keywords, and conventions from scratch.

### 3. Programmatic generation

Mappings will be generated by tools — schema inference engines, UI builders,
migration scripts. These tools emit data structures, which serialize trivially
to YAML/JSON. A custom DSL requires either:

- A code-generation library (string building), or
- An AST builder + pretty-printer (significant investment).

### 4. Schema evolution

Adding a new property to YAML is trivial: add the key to the spec, update
the JSON Schema, handle it in the parser (serde does this automatically for
optional fields). Adding syntax to a custom DSL means updating the grammar,
the parser, the formatter, the validator, the language server, and the
documentation — a much higher cost per feature.

### 5. The verbosity is manageable

The YAML format averages ~30-80 lines for simple-to-medium mappings. The
most complex examples (composite-keys, nested-arrays-deep) reach ~150 lines.
This is well within the range where readability > conciseness. A DSL that
saves 30% (~20-50 lines) doesn't fundamentally change the authoring
experience.

The areas where YAML is most verbose — test cases (~40% of file size) — are
not part of the mapping language itself. Test syntax can be improved
independently (e.g., a compact test format) without changing the mapping
format.

### 6. Validation is already strong

Between JSON Schema validation and the engine's 11-pass semantic validator,
the current format catches:

- Structural errors (missing required fields, wrong types)
- Duplicate names
- Invalid target references
- Strategy consistency
- Field coverage gaps
- SQL syntax errors (balanced parens/quotes)
- Prohibited keywords and internal view references
- Unknown column references (warnings)
- Parent mapping consistency

A custom parser would need to replicate all of this validation from scratch.

## What about a dual format?

Some tools offer both: a concise DSL for humans and a structured format
(YAML/JSON) for machines. Prisma does this: the Prisma Schema Language
is the primary format, but a JSON representation exists for programmatic
use.

This sounds appealing but has real costs:

- **Two parsers, two serializers, two test suites.** Every feature must work
  in both formats. Every edge case must be verified twice.
- **Canonicalization.** Which format is authoritative? If both are valid,
  what happens when they diverge?
- **Documentation burden.** Every example must be shown in both formats.

The dual-format approach only makes sense if the DSL is significantly more
usable than YAML — enough to justify maintaining two parallel stacks.

## Hybrid option: YAML with a compact shorthand layer

Instead of replacing YAML, add a preprocessing layer that expands shorthands:

```yaml
# Short field mapping: source -> target
fields:
  - email -> email
  - full_name -> first_name = SPLIT_PART(full_name, ' ', 1)

# Expands to:
fields:
  - { source: email, target: email }
  - { source: full_name, target: first_name, expression: "SPLIT_PART(full_name, ' ', 1)" }
```

This keeps YAML as the base format but adds domain-specific compact syntax
for the most common patterns. The parser recognizes shorthand strings and
expands them during deserialization.

**Pros:**
- Keep the entire YAML ecosystem (editors, schemas, linters, LLMs).
- Reduce verbosity for the 80% case (simple field mappings).
- No new parser — just string pattern matching in the serde deserializer.
- Schema stays YAML; compact forms are syntactic sugar.

**Cons:**
- JSON Schema can't validate the shorthand strings (they're opaque strings
  to the schema).
- Mixing shorthand and full-form syntax in one file can be confusing.
- The shorthand language is still a mini-DSL; it just lives inside YAML
  strings.

## Decision matrix

| Criterion | YAML | Custom DSL | Hybrid |
|-----------|------|-----------|--------|
| **Familiarity** | High — universal | Low — must learn | High — YAML base |
| **Tooling** | Excellent (editors, schemas, LLMs) | Must build from scratch | Good (some schema gaps) |
| **Conciseness** | Verbose (~30% overhead) | Compact | Medium improvement |
| **Error messages** | Generic YAML + validation pass | Domain-specific | YAML + validation pass |
| **Programmatic generation** | Trivial (serialize to YAML) | Needs codegen library | Trivial (full-form YAML) |
| **Schema evolution** | Low cost (add key + schema) | High cost (grammar + parser) | Low cost |
| **Expression quoting** | Quoted strings | First-class syntax | Still quoted |
| **Ecosystem investment** | Zero | Parser + formatter + LSP + docs | Low |
| **Migration cost** | Zero | High (rewrite all examples + docs) | Low (backward-compatible) |

## Recommendation

**Stay with YAML.** The mapping format is purely declarative — it has no
computed values, no conditionals, no loops, no imports. This is exactly the
domain where YAML excels and where a custom DSL's costs exceed its benefits.

The conciseness gains (~30%) are real but don't cross the threshold that
justified HCL (Terraform needed computation) or Prisma Schema (data
modeling needed relations as first-class syntax). Our format is closer to
Kubernetes manifests or GitHub Actions workflows — structured declarative
documents where YAML's ecosystem leverage dominates.

### Concrete improvements within YAML

Instead of a DSL, address pain points directly:

1. **Compact field mapping strings** (hybrid shorthand, optional):
   `- email -> email` alongside `- { source: email, target: email }`. The
   parser accepts both; the schema validates the full form. This is purely
   additive and backward-compatible.

2. **Test format improvement**: Tests consume ~40% of file size. A separate
   test format or compact syntax would reduce bulk without touching the
   mapping format.

3. **Implicit source derivation**: When a mapping has `source: { dataset: x }`
   and `sources:` doesn't declare `x`, auto-create it with defaults. Removes
   the mandatory `sources:` boilerplate for simple cases (already done — the
   parser handles missing sources).

4. **JSON Schema improvements**: Better `description` fields, `examples`,
   and `markdownDescription` for hover docs in VS Code.

### When to reconsider

Revisit this decision if:

- The format needs **computed values** (e.g., field names derived from
  patterns, conditional mappings based on source schema).
- The format needs **imports/composition** (e.g., shared mapping fragments
  reused across files).
- **User feedback** consistently reports YAML as a barrier to adoption.
- The number of properties exceeds ~100, making YAML nesting unmanageable.

None of these conditions are met today.
