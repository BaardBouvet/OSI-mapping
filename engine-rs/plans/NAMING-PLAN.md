# Project and binary naming

**Status:** Design

Supersedes the original binary-naming discussion (engine → compiler) by broadening scope to the full project name. The "OSI" prefix no longer makes sense — this plan covers both the project identity and the CLI binary name.

## Why Rename

The current name "OSI Mapping" refers to "Open Semantic Interchange," which is no longer what the project is about. The name:
- Collides with the OSI networking model (Open Systems Interconnection) in every Google search
- Doesn't describe what the project actually does
- Sounds like a standards body, not a tool
- Has no name recognition to protect

## What the Project Actually Is

A declarative schema (YAML) that compiles to a DAG of SQL views for multi-source data integration — handling identity linking, per-field conflict resolution, bidirectional reverse mapping, cross-system FK translation, and delta computation. One file describes the full picture; the engine renders it to PostgreSQL views.

The core metaphor: **multiple streams of data converge into a single truth, then diverge back to their sources.**

## Naming Criteria

1. **Memorable and pronounceable** — you should be able to say it in conversation
2. **Evocative** — should hint at merging, resolution, or data unification
3. **Short** — ideally ≤10 characters, works as a CLI command
4. **Available** — on crates.io (must), GitHub (should), PyPI (nice to have)
5. **Not confusable** — shouldn't collide with well-known tools in the data/dev space
6. **Works as a verb or noun** — "run crossfold" / "the crossfold schema"

## Availability Key

| Symbol | Meaning |
|--------|---------|
| ✅ | Available (not found) |
| ⚠️ | Taken but stale/tiny/unrelated |
| ❌ | Taken by active/significant project |

---

## Tier 1: Recommended

### 1. Crossfold

**Tagline:** *Cross-system data, folded into truth.*

| Platform | Status |
|----------|--------|
| crates.io | ✅ Available |
| GitHub | ⚠️ 10 results, all ML cross-validation (different domain) |
| PyPI | ⚠️ Cloudflare blocked check, likely available |

**Why it works:**
- "Cross" evokes cross-system integration — the core problem domain
- "Fold" evokes folding/reducing multiple values into one — exactly what resolution does
- Functional programming connotation: a fold/reduce operation across sources
- Unique in the data tooling space — no collisions
- Good CLI feel: `crossfold render mapping.yaml`, `crossfold validate`
- 9 characters, two syllables, easy to spell and say

**Risks:** Could be confused with k-fold cross-validation in ML contexts, but context would always disambiguate.

### 2. Viewfold

**Tagline:** *Fold your data into views.*

| Platform | Status |
|----------|--------|
| crates.io | ✅ Available |
| GitHub | ✅ Not found |
| PyPI | ⚠️ Cloudflare blocked check, likely available |

**Why it works:**
- Literally describes the output: SQL views created by folding data together
- Very transparent — someone hearing the name can guess what it does
- 8 characters, two syllables
- Good CLI feel: `viewfold render mapping.yaml`

**Risks:** More "descriptive" than "memorable." Less brandable than Crossfold.

### 3. Resolvr

**Tagline:** *Declarative conflict resolution for multi-source data.*

| Platform | Status |
|----------|--------|
| crates.io | ✅ Available |
| GitHub | ⚠️ Mixed results, no dominant project |
| PyPI | ⚠️ Taken (DNS pentest tool, completely unrelated) |

**Why it works:**
- Direct reference to conflict resolution — the core differentiator
- Modern -r spelling (like Flickr, Tumblr) is distinctive
- 7 characters, two syllables
- Good CLI feel: `resolvr render mapping.yaml`

**Risks:** "Resolv" also evokes DNS resolution (resolv.conf). The PyPI name is taken by an unrelated pentest tool (could use `resolvr-data` on PyPI if ever needed).

---

## Tier 2: Strong Alternatives

### 4. Goldmeld

**Tagline:** *Meld your sources into golden records.*

| Platform | Status |
|----------|--------|
| crates.io | ✅ Available |
| GitHub | ✅ Not found |

**Why it works:** Direct reference to "golden record" (MDM concept) and "meld" (merge). Very on-the-nose for anyone in the MDM/data quality space. 8 characters.

**Risks:** "Meld" is a well-known diff tool (meldmerge.org) and a recently active crates.io project. The compound avoids direct collision but may still cause some confusion. Feels a bit literal — less brandable.

### 5. Fieldmeld

**Tagline:** *Field-level data merging, declared.*

| Platform | Status |
|----------|--------|
| crates.io | ✅ Available |
| GitHub | ✅ Not found |

**Why it works:** Emphasizes the per-field nature of the resolution (not row-level, not table-level — *field*-level). Descriptive and precise. 9 characters.

**Risks:** Same "meld" association as Goldmeld. Sounds a bit corporate/enterprise.

### 6. Syncfold

**Tagline:** *Fold multiple sources into synchronized truth.*

| Platform | Status |
|----------|--------|
| crates.io | ✅ Available |
| GitHub | ✅ Not found |

**Why it works:** Evokes bidirectional sync (a key feature) and fold/reduce. 8 characters.

**Risks:** "Sync" implies runtime synchronization, but the tool generates views, not a sync daemon. Could set wrong expectations.

---

## Harmonization Family

These names lean into the "bringing disparate sources into harmony" metaphor — treating integration as harmonization rather than mechanical merging.

### 7. Attune

**Tagline:** *Attune your data sources.*

| Platform | Status |
|----------|--------|
| crates.io | ❓ Needs verification |
| GitHub | ❓ Needs verification |
| PyPI | ❓ Needs verification |

**Why it works:**
- "Attune" means to bring into harmony or make receptive — exactly what the tool does to conflicting sources
- 6 characters, two syllables, easy to say and spell
- Works naturally as both verb and noun: "attune the mapping" / "the attune schema"
- Excellent CLI feel: `attune render mapping.yaml`, `attune validate`
- Distinctive — unlikely to collide with data tooling
- Elegant and understated, avoids the "enterprise middleware" feel

**Risks:** Could be perceived as too soft/abstract for a technical tool. Less immediately obvious what it does compared to Crossfold or Viewfold.

### 8. Concord

**Tagline:** *Bring your data into agreement.*

| Platform | Status |
|----------|--------|
| crates.io | ❓ Needs verification |
| GitHub | ❓ Needs verification |
| PyPI | ❓ Needs verification |

**Why it works:**
- Literally means "agreement, harmony" (from Latin *concordia*)
- 7 characters, two syllables
- Strong historical/literary resonance — treaties, agreements, shared truth
- Good CLI feel: `concord render mapping.yaml`
- Evokes the outcome: after resolution, all sources are in concord

**Risks:** Concord is a common place name (New Hampshire, Massachusetts, grape variety, the supersonic jet). May have crates.io/GitHub collisions due to general popularity of the word.

### 9. Harmonic

**Tagline:** *Harmonic resolution for multi-source data.*

| Platform | Status |
|----------|--------|
| crates.io | ❓ Needs verification |
| GitHub | ❓ Needs verification |
| PyPI | ❓ Needs verification |

**Why it works:**
- In music, harmonics are frequencies that naturally combine — parallel to combining data from multiple sources
- In math/physics, "harmonic" implies smooth convergence to an equilibrium
- 8 characters, three syllables
- Sounds technical and precise: `harmonic render mapping.yaml`

**Risks:** Three syllables is slightly longer to say. "Harmonic" is a common word in science/engineering — higher collision risk. May feel more like a library name than a CLI tool.

### 10. Consonance

**Tagline:** *Where your data sources agree.*

| Platform | Status |
|----------|--------|
| crates.io | ❓ Needs verification |
| GitHub | ❓ Needs verification |

**Why it works:**
- Musical term: pleasant agreement of sounds. Opposite of dissonance — which is what conflicting source data is.
- The metaphor is strong: the tool takes dissonant data and produces consonance.
- 10 characters — at the upper limit of the naming criteria.

**Risks:** 10 characters and four syllables is borderline too long. Harder to type as a CLI command. Less "punchy" than shorter alternatives.

---

## Tier 3: Considered and Rejected

| Name | Why rejected |
|------|-------------|
| **Conflux** | Heavily taken — Conflux Chain blockchain (786 GitHub repos, crates.io taken) |
| **Meld** | Taken on crates.io (active AI context tool) + famous `meld` diff tool |
| **Arbiter** | Taken on crates.io (multi-agent framework, 32K downloads) |
| **Fugue** | Taken on crates.io (binary analysis framework, 17K downloads) |
| **Tessera** | Taken on crates.io (3D tiles). Beautiful metaphor (mosaic tiles) but not available |
| **Crucible** | Taken on crates.io + Oxide Computer has a storage project called Crucible |
| **Alloy** | Massively taken (Ethereum library, 5M+ downloads) |
| **Amalgam** | Taken on crates.io (config generator) |
| **Ingot** | Taken on crates.io (Oxide's packet parser, 108K downloads) |
| **Accord** | Taken on crates.io (validation library, 12K downloads) |
| **Keel** | Taken on crates.io (Kubernetes client). Beautiful metaphor (ship's structural backbone) but stale squatted |
| **Quorum** | Taken on crates.io (very new, 11 downloads). Also too associated with consensus protocols |
| **Canon** | Too overloaded (printers, cameras, religion, literature) |
| **Datameld** | Available but feels like a B2B SaaS name from 2012 |
| **Entmeld** | Available but sounds like "ant meld" when spoken aloud |

---

## Recommendation

**Crossfold** is the strongest choice. It:

1. Is available everywhere that matters (crates.io, GitHub namespace)
2. Evokes the core concept (cross-system + fold/reduce)
3. Is short, memorable, and unique in the data tooling space
4. Works naturally as a CLI command and brand name
5. Has no significant collisions
6. Would make a strong GitHub org name (`crossfold/crossfold` or `crossfold/engine`)
7. Has a natural tagline: *"Cross-system data, folded into truth"*

**Runner-up: Viewfold** if you want something more transparently descriptive ("it folds data into views").

**Runner-up: Resolvr** if you want to emphasize the conflict resolution angle.

---

## Rename Scope (if approved)

What would need to change:

| Item | Current | New |
|------|---------|-----|
| GitHub repo | `osi-mapping` | `crossfold` |
| Root README title | "Integration Mapping Schema" | "Crossfold" |
| Engine crate name | `osi-engine` (in Cargo.toml) | `crossfold` or `crossfold-engine` |
| Engine binary | `osi-engine` | `crossfold` |
| Docs references | "OSI Mapping" / "Integration Mapping Schema" | "Crossfold" |
| Validation script | references to "osi" | references to "crossfold" |
| Internal code | any `osi_` prefixes | `crossfold_` prefixes |
| Schema version | Could stay `1.0` — the schema itself is independent of the tool name |

The mapping YAML format itself has no name dependency — `version: "1.0"` doesn't reference "OSI" anywhere. The rename is primarily the tool/project name, not the schema format.

## Binary naming (from original plan)

Separate from the project name, the binary should reflect that the tool is a **compiler** (YAML → SQL), not a runtime engine.

Once a project name is chosen, the binary should follow the `{name}c` convention (like `rustc`, `gcc`):

| Project name | Binary | Crate | Feels like |
|-------------|--------|-------|------------|
| crossfold | `crossfold` | `crossfold` | `crossfold render`, `crossfold validate` |
| viewfold | `viewfold` | `viewfold` | `viewfold render` |
| resolvr | `resolvr` | `resolvr` | `resolvr render` |

For short project names, the bare name works fine as the binary. The `-c` suffix pattern (`crossfoldc`) is unnecessary when the name itself is short enough. The CLI subcommands (`render`, `validate`, `dot`) already clarify the tool's role.
