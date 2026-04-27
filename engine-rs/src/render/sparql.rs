//! SPARQL/RDF backend (slice 1).
//!
//! Pipeline (real SPARQL, no Rust-side aggregation):
//!
//! 1. **Lift.** Per mapping, rows are serialised to a JSON-LD document
//!    whose `@context` maps source field names to source-property
//!    predicates and whose `@id` is the source-row IRI. The JSON-LD doc
//!    is parsed by Oxigraph into the named graph `<base>/sourcegraph/<M>`.
//!
//! 2. **Identity closure.** A SPARQL `INSERT { ... } WHERE { ... }`
//!    update materialises `<row> osi:canonical <cid>` triples in
//!    `<base>/identity/<T>`, where `<cid>` is computed inside SPARQL via
//!    `IRI(CONCAT(prefix, SHA256(STR(?val))))`. Equal identity values
//!    therefore route to the same canonical IRI without any union-find.
//!
//! 3. **Forward resolution.** Per (target, target-field), a SPARQL
//!    `INSERT WHERE` picks the winning value via a `FILTER NOT EXISTS`
//!    pattern that says "no candidate has strictly better
//!    `(priority, decl_order)`". Compile-time priorities/decl-orders are
//!    embedded as `VALUES` rows.
//!
//! 4. **Reverse projection.** Per mapping, a SPARQL `CONSTRUCT` query
//!    projects canonical values back into source-property predicates,
//!    once for existing source rows (joined via `osi:canonical`) and
//!    once for canonicals that have no source row in this mapping
//!    (inserts). The resulting triples are grouped per subject into
//!    `Row`s in Rust.
//!
//!    For parent mappings (those that have a child mapping with
//!    `parent:` + `array:` pointing back at them), each existing /
//!    insert row is enriched with a synthetic `<child.array>` column
//!    via **JSON-LD framing**. The pipeline:
//!
//!      1. CONSTRUCT a graph linking each child canonical entity to
//!         its parent linkage value via a synthetic `osi:embedFor/<M>`
//!         predicate, with all child canonical fields attached.
//!      2. Apply a [`crate::render::framing::Frame`] describing the
//!         child shape (`@type` = child target, scalar properties for
//!         each non-linkage field) via [`crate::render::framing::apply_frame_grouped_by`].
//!      3. Sort each group by the child target's identity, encode as
//!         canonical JSON via [`canonical_json_string`].
//!
//!    The resulting bytes match PG's matching
//!    `jsonb_agg(jsonb_build_object(...) ORDER BY ...)` output exactly,
//!    so the round-trip diff in `<M>_updates` / `<M>_inserts` /
//!    `<M>_deletes` agrees across both backends.
//!
//!    The framer is a focused subset of full JSON-LD framing
//!    semantics; see `framing.rs` for what is and isn't supported and
//!    the path to swapping in the `json-ld` crate later.
//!
//! Slice 1 supports: single-field identity, `coalesce` strategy, flat
//! source rows, no `references:` / nesting / written-state.

use crate::model::{Doc, IdentityGroup, Strategy};
use anyhow::{anyhow, Context, Result};
use indexmap::IndexMap;
use oxigraph::io::{JsonLdProfileSet, RdfFormat, RdfParser};
use oxigraph::model::{GraphName, NamedNode, NamedNodeRef, Term};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

const DEFAULT_BASE_IRI: &str = "https://osi.test/";

thread_local! {
    /// The base IRI used by all `*_iri` / `*_graph_iri` helpers and by
    /// `render_sparql`.  Set for the duration of a single `render_sparql*`
    /// call via `BaseGuard`; defaults to `DEFAULT_BASE_IRI` outside that
    /// scope.  Thread-local so concurrent renders on different threads
    /// don't trample each other.
    static BASE_IRI: std::cell::RefCell<String> = std::cell::RefCell::new(DEFAULT_BASE_IRI.to_string());
}

/// RAII guard that swaps in a custom base IRI for the duration of a
/// render call and restores the previous value on drop.
struct BaseGuard {
    prev: String,
}

impl BaseGuard {
    fn new(base: &str) -> Self {
        let prev = BASE_IRI.with(|b| {
            let prev = b.borrow().clone();
            *b.borrow_mut() = base.to_string();
            prev
        });
        Self { prev }
    }
}

impl Drop for BaseGuard {
    fn drop(&mut self) {
        BASE_IRI.with(|b| *b.borrow_mut() = std::mem::take(&mut self.prev));
    }
}

fn base() -> String {
    BASE_IRI.with(|b| b.borrow().clone())
}

fn osi_canonical() -> String {
    format!("{}vocab/canonical", base())
}

// ---------------------------------------------------------------------------
// IRI helpers
// ---------------------------------------------------------------------------

fn encode_pk(s: &str) -> String {
    s.replace('%', "%25").replace('/', "%2F")
}

fn decode_pk(s: &str) -> String {
    s.replace("%2F", "/").replace("%25", "%")
}

fn source_iri(mapping: &str, pk: &str) -> String {
    format!("{}source/{}/{}", base(), mapping, encode_pk(pk))
}

fn source_prop_iri(mapping: &str, field: &str) -> String {
    format!("{}sourceprop/{}/{}", base(), mapping, field)
}

fn canonical_prop_iri(field: &str) -> String {
    format!("{}prop/{}", base(), field)
}

fn source_graph_iri(mapping: &str) -> String {
    format!("{}sourcegraph/{}", base(), mapping)
}

fn canonical_graph_iri(target: &str) -> String {
    format!("{}canonical/{}", base(), target)
}

fn identity_graph_iri(target: &str) -> String {
    format!("{}identity/{}", base(), target)
}

fn canonical_iri_prefix(target: &str) -> String {
    format!("{}canonical/{}/", base(), target)
}

/// Per-parent-target graph holding materialised RDF lists
/// (`osi:hasChild/<array>` + `rdf:first`/`rdf:rest` chains) for every
/// child mapping that embeds into this target. Kept separate from
/// `canonical/<target>` so derived ordering data stays cleanly
/// distinguishable from canonical scalar properties.
fn lists_graph_iri(parent_target: &str) -> String {
    format!("{}lists/{}", base(), parent_target)
}

/// Graph holding the reverse-projected rows for a mapping: the canonical
/// state as it should appear in the source system.  Analogous to the SQL
/// `<mapping>_reverse` view — inspectable at any time with:
///   SELECT * WHERE { GRAPH <base>/reverse/<mapping> { ?s ?p ?o } }
fn reverse_graph_iri(mapping: &str) -> String {
    format!("{}reverse/{}", base(), mapping)
}

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Per-source-row, keyed by source column name. Mirrors the conformance
/// harness's representation.
pub type Row = IndexMap<String, serde_yaml::Value>;

#[derive(Debug, Default)]
pub struct Deltas {
    pub updates: HashMap<String, Vec<Row>>,
    pub inserts: HashMap<String, Vec<Row>>,
    pub deletes: HashMap<String, Vec<Row>>,
}

/// Compiled SPARQL/RDF plan for a `Doc`.
///
/// Every field holds a SPARQL CONSTRUCT query or JSON-LD document.
/// These are the artifacts deployed to a triplestore and the exact
/// queries the in-process test executor runs — no separate UPDATE form.
#[derive(Debug, Clone)]
pub struct SparqlPlan {
    pub base_iri: String,
    /// Mapping name → JSON-LD `@context` document used to lift its source rows.
    pub contexts: IndexMap<String, serde_json::Value>,
    /// Target name → SPARQL CONSTRUCT that defines `<base>/identity/<T>`.
    pub identity_constructs: IndexMap<String, String>,
    /// `"<target>.<field>"` → SPARQL CONSTRUCT that defines one column of
    /// `<base>/canonical/<T>`.
    pub forward_constructs: IndexMap<String, String>,
    /// Mapping name → SPARQL CONSTRUCT query that projects existing
    /// source rows back from the canonical graph.
    pub reverse_existing: IndexMap<String, String>,
    /// Mapping name → SPARQL CONSTRUCT query that projects insert rows
    /// (canonicals with no existing source row in this mapping).
    pub reverse_inserts: IndexMap<String, String>,
    /// Child mapping name → JSON-LD frame document.
    pub frame_documents: IndexMap<String, serde_json::Value>,
    /// Child mapping name → SPARQL CONSTRUCT fed into the JSON-LD framer.
    pub frame_constructs: IndexMap<String, String>,
    /// Child mapping name → two SPARQL CONSTRUCTs (cells + heads) that
    /// define `<base>/lists/<parent_target>`.
    pub list_constructs: IndexMap<String, String>,
    // Captured for executor convenience; not part of the published artifact.
    doc: Doc,
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// Renders a `SparqlPlan` using the default base IRI
/// (`https://osi.test/`). Equivalent to
/// `render_sparql_with_base(doc, "https://osi.test/")`.
pub fn render_sparql(doc: &Doc) -> Result<SparqlPlan> {
    render_sparql_with_base(doc, DEFAULT_BASE_IRI)
}

/// Renders a `SparqlPlan` whose every IRI is rooted at `base`.
///
/// `base` is used verbatim — it should already end with `/` (e.g.
/// `"https://example.org/osi/"`).  All named-graph IRIs become
/// `<base>identity/<T>`, `<base>canonical/<T>`, etc., and all property
/// IRIs become `<base>prop/<f>` and `<base>sourceprop/<M>/<f>`.
pub fn render_sparql_with_base(doc: &Doc, base: &str) -> Result<SparqlPlan> {
    let _guard = BaseGuard::new(base);
    render_sparql_inner(doc)
}

fn render_sparql_inner(doc: &Doc) -> Result<SparqlPlan> {
    // Slice 4 not yet rendered: cross-entity references on field maps.
    for m in &doc.mappings {
        for fm in &m.fields {
            if fm.references.is_some() {
                return Err(anyhow!(
                    "sparql: mapping `{}` field `{}` uses references: — slice 4 not yet implemented",
                    m.name,
                    fm.target
                ));
            }
        }
        if m.parent.is_some() != m.array.is_some() {
            return Err(anyhow!(
                "sparql: mapping `{}` must declare both parent: and array:, or neither",
                m.name
            ));
        }
        if let Some(parent_name) = &m.parent {
            let parent = doc
                .mappings
                .iter()
                .find(|p| p.name == *parent_name)
                .ok_or_else(|| {
                    anyhow!("mapping `{}` parent `{}` not found", m.name, parent_name)
                })?;
            if parent.parent.is_some() {
                return Err(anyhow!(
                    "sparql: mapping `{}` parent `{}` is itself nested; deep nesting (slice 3c) not yet implemented",
                    m.name,
                    parent_name
                ));
            }
        }
    }

    // Slice 2/5a constraints: single-field OR composite (AND-tuple)
    // identity, exactly one OR-group; strategies `coalesce` and
    // `last_modified` are supported.
    for (target_name, t) in &doc.targets {
        match t.identity.as_slice() {
            [IdentityGroup::Single(_)] | [IdentityGroup::Tuple(_)] => {}
            _ => {
                return Err(anyhow!(
                    "sparql: target `{}` must have exactly one identity group (single field or AND-tuple)",
                    target_name
                ));
            }
        }
        for (fname, f) in &t.fields {
            match f.strategy {
                Strategy::Coalesce | Strategy::LastModified => {}
            }
            let _ = (fname, f);
        }
    }

    let mut plan = SparqlPlan {
        base_iri: base(),
        contexts: IndexMap::new(),
        identity_constructs: IndexMap::new(),
        forward_constructs: IndexMap::new(),
        reverse_existing: IndexMap::new(),
        reverse_inserts: IndexMap::new(),
        frame_documents: IndexMap::new(),
        frame_constructs: IndexMap::new(),
        list_constructs: IndexMap::new(),
        doc: doc.clone(),
    };

    // JSON-LD context per mapping.
    for m in &doc.mappings {
        let mut ctx = serde_json::Map::new();
        for fm in &m.fields {
            ctx.insert(
                fm.source.clone(),
                serde_json::Value::String(source_prop_iri(&m.name, &fm.source)),
            );
        }
        // Also lift the mapping's `last_modified:` source column (if any)
        // so timestamp-aware resolution can read it from the source graph.
        if let Some(lm) = &m.last_modified {
            ctx.entry(lm.clone())
                .or_insert_with(|| serde_json::Value::String(source_prop_iri(&m.name, lm)));
        }
        let mut doc_obj = serde_json::Map::new();
        doc_obj.insert("@context".to_string(), serde_json::Value::Object(ctx));
        plan.contexts
            .insert(m.name.clone(), serde_json::Value::Object(doc_obj));
    }

    // Identity CONSTRUCT per target.
    for (target_name, t) in &doc.targets {
        let ident_fields: Vec<String> = match t.identity.as_slice() {
            [IdentityGroup::Single(f)] => vec![f.clone()],
            [IdentityGroup::Tuple(fs)] => fs.clone(),
            _ => unreachable!("validated above"),
        };
        plan.identity_constructs.insert(
            target_name.clone(),
            build_identity_construct(doc, target_name, &ident_fields),
        );
    }

    // Forward CONSTRUCT per (target, field).
    for (target_name, t) in &doc.targets {
        for tfield in t.fields.keys() {
            let key = format!("{}.{}", target_name, tfield);
            let c = build_forward_construct(doc, target_name, tfield);
            plan.forward_constructs.insert(key, c);
        }
    }

    // Reverse CONSTRUCTs per mapping (existing + insert variants).
    for m in &doc.mappings {
        plan.reverse_existing
            .insert(m.name.clone(), build_reverse_existing_construct(doc, m));
        plan.reverse_inserts
            .insert(m.name.clone(), build_reverse_inserts_construct(doc, m));
    }

    // Per-child framing and list CONSTRUCTs.
    for child in &doc.mappings {
        if child.parent.is_none() {
            continue;
        }
        let (frame, construct) = build_child_frame(doc, child)?;
        plan.frame_documents
            .insert(child.name.clone(), frame.to_json());
        plan.frame_constructs.insert(child.name.clone(), construct);
        plan.list_constructs
            .insert(child.name.clone(), build_list_construct(doc, child)?);
    }

    Ok(plan)
}

// ---------------------------------------------------------------------------
// Identity CONSTRUCT.
//
// Defines `<base>/identity/<T>`: for each source row whose identity field(s)
// are bound, maps the values to a canonical IRI via SHA256.
// Annotated with `# Maintains: GRAPH <IRI>` so the caller knows which named
// graph the rule targets.
// ---------------------------------------------------------------------------

fn build_identity_construct(doc: &Doc, target_name: &str, ident_fields: &[String]) -> String {
    let id_graph = identity_graph_iri(target_name);
    let cid_prefix = canonical_iri_prefix(target_name);

    let mut union_branches: Vec<String> = Vec::new();
    for m in doc.mappings.iter().filter(|m| m.target == target_name) {
        let mut tpls: Vec<(String, String)> = Vec::new();
        let mut all_present = true;
        for (i, ident_f) in ident_fields.iter().enumerate() {
            let Some(fm) = m.fields.iter().find(|fm| fm.target == *ident_f) else {
                all_present = false;
                break;
            };
            let p = source_prop_iri(&m.name, &fm.source);
            tpls.push((format!("?val{i}"), p));
        }
        if !all_present {
            continue;
        }
        let g = source_graph_iri(&m.name);
        let triples = tpls
            .iter()
            .map(|(var, pred)| format!("?row <{pred}> {var}"))
            .collect::<Vec<_>>()
            .join(" . ");
        union_branches.push(format!("{{ GRAPH <{g}> {{ {triples} }} }}"));
    }

    if union_branches.is_empty() {
        return format!("# no mapping covers identity for {target_name}\n# Maintains: GRAPH <{id_graph}>\nCONSTRUCT {{}} WHERE {{ FILTER false }}");
    }
    let where_pattern = union_branches.join("\n    UNION\n    ");

    let bind_expr = if ident_fields.len() == 1 {
        format!("BIND(IRI(CONCAT(\"{cid_prefix}\", SHA256(STR(?val0)))) AS ?cid)")
    } else {
        let mut parts: Vec<String> = Vec::new();
        for i in 0..ident_fields.len() {
            if i > 0 {
                parts.push("\"\\u001F\"".to_string());
            }
            parts.push(format!("STR(?val{i})"));
        }
        let concat = parts.join(", ");
        format!("BIND(IRI(CONCAT(\"{cid_prefix}\", SHA256(CONCAT({concat})))) AS ?cid)")
    };

    let osi_canon = osi_canonical();
    format!(
        "# Maintains: GRAPH <{id_graph}>\nCONSTRUCT {{\n  ?row <{osi_canon}> ?cid\n}}\nWHERE {{\n    {where_pattern}\n    {bind_expr}\n}}"
    )
}

// ---------------------------------------------------------------------------
// Forward CONSTRUCT.
//
// One CONSTRUCT per (target, field). Winner picking depends on the field's
// strategy:
//   - Coalesce      → pick the candidate with no strictly-better
//                     (priority, decl_order) competitor.
//   - LastModified  → pick the candidate with no strictly-better
//                     (last_modified DESC, decl_order ASC) competitor.
// Compile-time per-mapping metadata (priority, decl_order, last_modified
// source predicate) is embedded as `VALUES` rows.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Candidate {
    source_graph: String,
    source_pred: String,
    priority: i64,
    decl_order: i64,
    /// Predicate IRI of the mapping's last_modified column on the source
    /// row, if any.
    last_modified_pred: Option<String>,
}

fn collect_candidates(doc: &Doc, target_name: &str, tfield: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (idx, m) in doc.mappings.iter().enumerate() {
        if m.target != target_name {
            continue;
        }
        let Some(fm) = m.fields.iter().find(|fm| fm.target == tfield) else {
            continue;
        };
        let lm_pred = m
            .last_modified
            .as_ref()
            .map(|col| source_prop_iri(&m.name, col));
        out.push(Candidate {
            source_graph: source_graph_iri(&m.name),
            source_pred: source_prop_iri(&m.name, &fm.source),
            priority: fm.priority.map(i64::from).unwrap_or(i64::MAX),
            decl_order: idx as i64,
            last_modified_pred: lm_pred,
        });
    }
    out
}

fn build_forward_construct(doc: &Doc, target_name: &str, tfield: &str) -> String {
    let target = doc
        .targets
        .get(target_name)
        .expect("target known to caller");
    let strategy = target.fields[tfield].strategy;
    match strategy {
        Strategy::Coalesce => build_forward_coalesce(doc, target_name, tfield),
        Strategy::LastModified => build_forward_last_modified(doc, target_name, tfield),
    }
}

fn build_forward_coalesce(doc: &Doc, target_name: &str, tfield: &str) -> String {
    let canonical_graph = canonical_graph_iri(target_name);
    let id_graph = identity_graph_iri(target_name);
    let canon_pred = canonical_prop_iri(tfield);
    let candidates = collect_candidates(doc, target_name, tfield);

    if candidates.is_empty() {
        return format!("# no candidates for {target_name}.{tfield}\n# Maintains: GRAPH <{canonical_graph}>\nCONSTRUCT {{}} WHERE {{ FILTER false }}");
    }

    let values_rows: Vec<String> = candidates
        .iter()
        .map(|c| {
            format!(
                "(<{}> <{}> {} {})",
                c.source_graph, c.source_pred, c.priority, c.decl_order
            )
        })
        .collect();
    let values_rows_str = values_rows.join("\n        ");
    let values_block =
        format!("VALUES (?mg ?p ?prio ?decl) {{\n        {values_rows_str}\n      }}");
    let values_block_alt =
        format!("VALUES (?mg2 ?p2 ?prio2 ?decl2) {{\n        {values_rows_str}\n      }}");

    let osi_canon = osi_canonical();
    format!(
        "# Maintains: GRAPH <{canonical_graph}>\nCONSTRUCT {{\n  ?cid <{canon_pred}> ?val\n}}\nWHERE {{\n  {{\n    SELECT ?cid ?val ?prio ?decl WHERE {{\n      {values_block}\n      GRAPH ?mg {{ ?row ?p ?val }}\n      GRAPH <{id_graph}> {{ ?row <{osi_canon}> ?cid }}\n    }}\n  }}\n  FILTER NOT EXISTS {{\n    {values_block_alt}\n    GRAPH ?mg2 {{ ?row2 ?p2 ?val2 }}\n    GRAPH <{id_graph}> {{ ?row2 <{osi_canon}> ?cid }}\n    FILTER ((?prio2 < ?prio) || (?prio2 = ?prio && ?decl2 < ?decl))\n  }}\n}}"
    )
}

/// `last_modified` resolution: a candidate wins when no other candidate
/// for the same canonical entity has a strictly-newer timestamp (or, on
/// timestamp tie, earlier decl_order). Mappings without a configured
/// `last_modified:` source field have no timestamp triple in the source
/// graph and lose to any timestamped competitor; if no candidate has a
/// timestamp, decl_order is the sole tie-breaker.
fn build_forward_last_modified(doc: &Doc, target_name: &str, tfield: &str) -> String {
    let canonical_graph = canonical_graph_iri(target_name);
    let id_graph = identity_graph_iri(target_name);
    let canon_pred = canonical_prop_iri(tfield);
    let candidates = collect_candidates(doc, target_name, tfield);

    if candidates.is_empty() {
        return format!("# no candidates for {target_name}.{tfield}\n# Maintains: GRAPH <{canonical_graph}>\nCONSTRUCT {{}} WHERE {{ FILTER false }}");
    }

    let values_rows: Vec<String> = candidates
        .iter()
        .map(|c| {
            let lm = c
                .last_modified_pred
                .as_ref()
                .map(|p| format!("<{p}>"))
                .unwrap_or_else(|| "UNDEF".to_string());
            format!(
                "(<{}> <{}> {} {})",
                c.source_graph, c.source_pred, lm, c.decl_order
            )
        })
        .collect();
    let values_rows_str = values_rows.join("\n        ");
    let values_block =
        format!("VALUES (?mg ?p ?lm_pred ?decl) {{\n        {values_rows_str}\n      }}");
    let values_block_alt =
        format!("VALUES (?mg2 ?p2 ?lm_pred2 ?decl2) {{\n        {values_rows_str}\n      }}");

    let osi_canon = osi_canonical();
    format!(
        "# Maintains: GRAPH <{canonical_graph}>\nCONSTRUCT {{\n  ?cid <{canon_pred}> ?val\n}}\nWHERE {{\n  {{\n    SELECT ?cid ?val ?lm ?decl WHERE {{\n      {values_block}\n      GRAPH ?mg {{ ?row ?p ?val }}\n      OPTIONAL {{ GRAPH ?mg {{ ?row ?lm_pred ?lm_raw }} }}\n      BIND(COALESCE(STR(?lm_raw), \"\") AS ?lm)\n      GRAPH <{id_graph}> {{ ?row <{osi_canon}> ?cid }}\n    }}\n  }}\n  FILTER NOT EXISTS {{\n    {values_block_alt}\n    GRAPH ?mg2 {{ ?row2 ?p2 ?val2 }}\n    OPTIONAL {{ GRAPH ?mg2 {{ ?row2 ?lm_pred2 ?lm_raw2 }} }}\n    BIND(COALESCE(STR(?lm_raw2), \"\") AS ?lm2)\n    GRAPH <{id_graph}> {{ ?row2 <{osi_canon}> ?cid }}\n    FILTER ((?lm2 > ?lm) || (?lm2 = ?lm && ?decl2 < ?decl))\n  }}\n}}"
    )
}

// ---------------------------------------------------------------------------
// Reverse CONSTRUCT queries.
//
// `reverse_existing` projects values for source rows that exist in this
// mapping (joined via osi:canonical → canonical graph property).
//
// `reverse_inserts` projects values for canonicals NOT linked to a source
// row in this mapping; the constructed subject is the canonical IRI itself,
// distinguishing inserts from updates.
// ---------------------------------------------------------------------------

fn build_reverse_existing_construct(_doc: &Doc, m: &crate::model::Mapping) -> String {
    let id_graph = identity_graph_iri(&m.target);
    let canonical_graph = canonical_graph_iri(&m.target);
    let source_graph = source_graph_iri(&m.name);

    let mut construct_lines = Vec::<String>::new();
    let mut where_lines = Vec::<String>::new();

    for fm in &m.fields {
        let src_pred = source_prop_iri(&m.name, &fm.source);
        let canon_pred = canonical_prop_iri(&fm.target);
        let var = sanitize_var(&fm.source);
        construct_lines.push(format!("  ?row <{src_pred}> ?{var} ."));
        where_lines.push(format!(
            "  OPTIONAL {{ GRAPH <{canonical_graph}> {{ ?cid <{canon_pred}> ?{var} }} }}"
        ));
    }

    let osi_canon = osi_canonical();
    format!(
        "CONSTRUCT {{\n{}\n}}\nWHERE {{\n  GRAPH <{source_graph}> {{ ?row ?_p ?_o }}\n  GRAPH <{id_graph}> {{ ?row <{osi_canon}> ?cid }}\n{}\n}}",
        construct_lines.join("\n"),
        where_lines.join("\n"),
    )
}

fn build_reverse_inserts_construct(doc: &Doc, m: &crate::model::Mapping) -> String {
    let id_graph = identity_graph_iri(&m.target);
    let canonical_graph = canonical_graph_iri(&m.target);
    let source_graph = source_graph_iri(&m.name);

    let mut construct_lines = Vec::<String>::new();
    let mut where_lines = Vec::<String>::new();

    for fm in &m.fields {
        let src_pred = source_prop_iri(&m.name, &fm.source);
        let canon_pred = canonical_prop_iri(&fm.target);
        let var = sanitize_var(&fm.source);
        construct_lines.push(format!("  ?cid <{src_pred}> ?{var} ."));
        where_lines.push(format!(
            "  OPTIONAL {{ GRAPH <{canonical_graph}> {{ ?cid <{canon_pred}> ?{var} }} }}"
        ));
    }

    // "Canonicals with no existing source row in this mapping": pick all
    // canonicals appearing in <identity/T> that are NOT linked to any row
    // in this mapping's source graph.
    let _ = doc; // currently unused; kept for symmetry / future use.
    let osi_canon = osi_canonical();
    format!(
        "CONSTRUCT {{\n{}\n}}\nWHERE {{\n  GRAPH <{id_graph}> {{ ?_any <{osi_canon}> ?cid }}\n  FILTER NOT EXISTS {{\n    GRAPH <{id_graph}> {{ ?row2 <{osi_canon}> ?cid }}\n    GRAPH <{source_graph}> {{ ?row2 ?_p ?_o }}\n  }}\n{}\n}}",
        construct_lines.join("\n"),
        where_lines.join("\n"),
    )
}

fn sanitize_var(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 1);
    out.push('v');
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Reverse materialisation UPDATE
// ---------------------------------------------------------------------------
// Deployable equivalent of the two ephemeral CONSTRUCTs above.
// Writes into <base>/reverse/<mapping> so the result is persistently
// queryable on any SPARQL 1.1 endpoint — analogous to the SQL
// `<mapping>_reverse` view.
//
// Shape:
//   Step 1: DELETE old reverse triples (idempotent re-run)
//   Step 2: INSERT from existing source rows  (≈ reverse_existing)
//   Step 3: INSERT from insert candidates     (≈ reverse_inserts)
// ---------------------------------------------------------------------------
// Executor.
// ---------------------------------------------------------------------------

impl SparqlPlan {
    pub fn execute(&self, inputs: &HashMap<String, Vec<Row>>) -> Result<Deltas> {
        let store = Store::new()?;
        self.lift(&store, inputs)?;
        // Identity: run each CONSTRUCT and insert results into identity/<T>.
        for (target, construct) in &self.identity_constructs {
            let graph = identity_graph_iri(target);
            run_construct_into_graph(&store, construct, &graph).with_context(|| {
                format!("identity CONSTRUCT failed for `{target}`:\n{construct}")
            })?;
        }
        // Forward: run each CONSTRUCT and insert results into canonical/<T>.
        for (key, construct) in &self.forward_constructs {
            let target = key.split('.').next().unwrap_or(key.as_str());
            let graph = canonical_graph_iri(target);
            run_construct_into_graph(&store, construct, &graph)
                .with_context(|| format!("forward CONSTRUCT failed for `{key}`:\n{construct}"))?;
        }
        // Lists: run each child's two CONSTRUCTs into lists/<parent_target>.
        self.materialise_child_lists(&store)?;
        self.compute_deltas(&store, inputs)
    }

    /// For each embedded child mapping, run its list CONSTRUCTs (cells +
    /// heads) and insert the results into `<base>/lists/<parent_target>`.
    fn materialise_child_lists(&self, store: &Store) -> Result<()> {
        for child in &self.doc.mappings {
            if child.parent.is_none() {
                continue;
            }
            let construct = self
                .list_constructs
                .get(&child.name)
                .ok_or_else(|| anyhow!("missing list_construct for `{}`", child.name))?;
            let parent_target = child
                .parent
                .as_ref()
                .and_then(|pn| self.doc.mappings.iter().find(|pm| pm.name == *pn))
                .map(|pm| pm.target.as_str())
                .ok_or_else(|| anyhow!("list parent target not found for `{}`", child.name))?;
            let graph = lists_graph_iri(parent_target);
            // The field stores two CONSTRUCTs separated by a blank line;
            // split on "\n\n" and run each one.
            for part in construct.split("\n\n") {
                let part = part.trim();
                if part.is_empty() || part.starts_with('#') && !part.contains("CONSTRUCT") {
                    continue;
                }
                run_construct_into_graph(store, part, &graph)
                    .with_context(|| format!("list CONSTRUCT failed for `{}`", child.name))?;
            }
        }
        Ok(())
    }

    fn lift(&self, store: &Store, inputs: &HashMap<String, Vec<Row>>) -> Result<()> {
        for m in &self.doc.mappings {
            let lifted = expand_lifted_rows(m, &self.doc, inputs)?;
            if lifted.is_empty() {
                continue;
            }
            let pk_col_opt = mapping_pk_col(m, &self.doc)?;

            // Build a JSON-LD doc: { "@context": ..., "@graph": [ {row}, ... ] }
            let context = self
                .contexts
                .get(&m.name)
                .and_then(|v| v.get("@context"))
                .ok_or_else(|| anyhow!("missing context for mapping {}", m.name))?
                .clone();

            let mut graph = Vec::<serde_json::Value>::new();
            for (pk_val, row) in &lifted {
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "@id".to_string(),
                    serde_json::Value::String(source_iri(&m.name, pk_val)),
                );
                for fm in &m.fields {
                    if let Some(pk_col) = pk_col_opt {
                        if fm.source == pk_col {
                            // PK already encoded in @id — don't emit as a
                            // source-prop too.
                            continue;
                        }
                    }
                    let Some(v) = row.get(&fm.source) else {
                        continue;
                    };
                    if matches!(v, serde_yaml::Value::Null) {
                        continue;
                    }
                    let json_v = yaml_to_json(v);
                    obj.insert(fm.source.clone(), json_v);
                }
                // Lift the mapping's last_modified column too, if declared
                // and not already present as a mapped target field.
                if let Some(lm) = &m.last_modified {
                    let is_pk = pk_col_opt.is_some_and(|c| c == lm);
                    if !obj.contains_key(lm) && !is_pk {
                        if let Some(v) = row.get(lm) {
                            if !matches!(v, serde_yaml::Value::Null) {
                                obj.insert(lm.clone(), yaml_to_json(v));
                            }
                        }
                    }
                }
                graph.push(serde_json::Value::Object(obj));
            }

            let mut doc_obj = serde_json::Map::new();
            doc_obj.insert("@context".to_string(), context);
            doc_obj.insert("@graph".to_string(), serde_json::Value::Array(graph));
            let doc_str = serde_json::to_string(&serde_json::Value::Object(doc_obj))?;

            let g = NamedNode::new(source_graph_iri(&m.name))?;
            let parser = RdfParser::from_format(RdfFormat::JsonLd {
                profile: JsonLdProfileSet::empty(),
            })
            .without_named_graphs()
            .with_default_graph(GraphName::NamedNode(g));
            store
                .load_from_reader(parser, doc_str.as_bytes())
                .with_context(|| format!("JSON-LD lift failed for mapping {}", m.name))?;
        }
        Ok(())
    }

    fn compute_deltas(&self, store: &Store, inputs: &HashMap<String, Vec<Row>>) -> Result<Deltas> {
        let mut deltas = Deltas::default();
        for m in &self.doc.mappings {
            let pk_col_opt = mapping_pk_col(m, &self.doc)?;
            let lifted = expand_lifted_rows(m, &self.doc, inputs)?;

            // 1. Existing source rows: run the reverse-existing CONSTRUCT and
            //    group resulting triples by subject.
            let existing_q = self
                .reverse_existing
                .get(&m.name)
                .ok_or_else(|| anyhow!("missing reverse_existing for {}", m.name))?;
            let existing_triples = run_construct(store, existing_q)?;
            let mut existing_rows = group_by_subject(&existing_triples, m, pk_col_opt, true)?;

            // 2. Insert rows: run the reverse-inserts CONSTRUCT.
            let inserts_q = self
                .reverse_inserts
                .get(&m.name)
                .ok_or_else(|| anyhow!("missing reverse_inserts for {}", m.name))?;
            let inserts_triples = run_construct(store, inserts_q)?;
            let mut insert_grouped = group_by_subject(&inserts_triples, m, pk_col_opt, false)?;

            // 2b. Slice 3b: enrich both `existing_rows` and `insert_grouped`
            //     with child-array agg columns (one per child mapping whose
            //     parent: is this mapping).
            let children: Vec<&crate::model::Mapping> = self
                .doc
                .mappings
                .iter()
                .filter(|c| c.parent.as_deref() == Some(m.name.as_str()) && c.array.is_some())
                .collect();
            for child in &children {
                let array_col = child.array.clone().unwrap();
                let agg = run_child_agg(store, m, child, &self.doc)?;
                // Linkage column on the parent side: parent_fields value
                // points at a parent source column; locate the parent's
                // `fields` entry for that source column to find the
                // canonical projection key in `proj` rows.
                let parent_link_col = child
                    .parent_fields
                    .values()
                    .next()
                    .ok_or_else(|| anyhow!("child {} has no parent_fields", child.name))?
                    .clone();
                for (_, row) in existing_rows.iter_mut() {
                    let pkey = row
                        .get(&parent_link_col)
                        .and_then(yaml_to_text)
                        .unwrap_or_default();
                    let json_str = agg.get(&pkey).cloned().unwrap_or_else(|| "[]".to_string());
                    row.insert(array_col.clone(), serde_yaml::Value::String(json_str));
                }
                for (_, row) in insert_grouped.iter_mut() {
                    let pkey = row
                        .get(&parent_link_col)
                        .and_then(yaml_to_text)
                        .unwrap_or_default();
                    let json_str = agg.get(&pkey).cloned().unwrap_or_else(|| "[]".to_string());
                    row.insert(array_col.clone(), serde_yaml::Value::String(json_str));
                }
            }

            // Round-trip column set: mapped fields + child-array columns.
            let mut roundtrip_cols: Vec<String> =
                m.fields.iter().map(|fm| fm.source.clone()).collect();
            for child in &children {
                if let Some(arr) = &child.array {
                    if !roundtrip_cols.iter().any(|c| c == arr) {
                        roundtrip_cols.push(arr.clone());
                    }
                }
            }

            let insert_rows: Vec<Row> = insert_grouped.into_iter().map(|(_, r)| r).collect();

            // 3. Diff lifted rows vs. existing canonical projection.
            let by_pk: HashMap<String, Row> = existing_rows
                .iter()
                .map(|(p, r)| (p.clone(), r.clone()))
                .collect();

            let mut updates: Vec<Row> = Vec::new();
            let mut deletes: Vec<Row> = Vec::new();
            for (pk_val, lifted_row) in &lifted {
                let Some(proj) = by_pk.get(pk_val) else {
                    deletes.push(strip_internal_columns(lifted_row));
                    continue;
                };
                if row_differs_cols(proj, lifted_row, &roundtrip_cols) {
                    updates.push(proj.clone());
                }
            }

            deltas.updates.insert(m.name.clone(), updates);
            deltas.inserts.insert(m.name.clone(), insert_rows);
            deltas.deletes.insert(m.name.clone(), deletes);
        }
        Ok(deltas)
    }

    // -----------------------------------------------------------------------
    // Artifact writing
    // -----------------------------------------------------------------------

    /// Returns a single SPARQL UPDATE script containing all pipeline UPDATEs
    /// in execution order, separated by `;`.  This is the SPARQL equivalent
    /// of the SQL DDL file: POST it to `$ENDPOINT/update` and the full
    /// pipeline runs in one request.
    /// Writes one CONSTRUCT file per artifact into `dir`, creating it if needed.
    ///
    /// File naming mirrors the named graph IRI path:
    ///
    /// ```text
    /// <dir>/
    ///   context_<mapping>.jsonld       -- JSON-LD @context (used during LIFT)
    ///   identity_<target>.sparql       -- CONSTRUCT defining identity/<target>
    ///   canonical_<target>.sparql      -- CONSTRUCTs defining canonical/<target>
    ///   lists_<parent>.sparql          -- CONSTRUCTs defining lists/<parent>
    ///   reverse_<mapping>.sparql       -- CONSTRUCTs defining reverse/<mapping>
    ///   framing_<child>.sparql         -- CONSTRUCT (on-demand framing query)
    ///   frame_<child>.jsonld           -- JSON-LD frame document
    /// ```
    ///
    /// Deploy: register each `*.sparql` CONSTRUCT with the triplestore's rule
    /// API.  Then LIFT is the only imperative step — all derived graphs update
    /// automatically.
    pub fn write_artifacts(&self, dir: &std::path::Path) -> anyhow::Result<()> {
        use std::fs;
        fs::create_dir_all(dir)?;

        for (mapping, ctx) in &self.contexts {
            fs::write(
                dir.join(format!("context_{mapping}.jsonld")),
                serde_json::to_string_pretty(ctx)?,
            )?;
        }

        for (target, c) in &self.identity_constructs {
            fs::write(dir.join(format!("identity_{target}.sparql")), c)?;
        }

        let mut canonical_by_target: IndexMap<String, Vec<&str>> = IndexMap::new();
        for (key, c) in &self.forward_constructs {
            let target = key.split('.').next().unwrap_or(key.as_str());
            canonical_by_target
                .entry(target.to_string())
                .or_default()
                .push(c.as_str());
        }
        for (target, constructs) in &canonical_by_target {
            fs::write(
                dir.join(format!("canonical_{target}.sparql")),
                constructs.join("\n\n"),
            )?;
        }

        for (child_mapping, c) in &self.list_constructs {
            let parent_target = self
                .doc
                .mappings
                .iter()
                .find(|m| m.name == *child_mapping)
                .and_then(|m| m.parent.as_deref())
                .and_then(|pn| self.doc.mappings.iter().find(|pm| pm.name == pn))
                .map(|pm| pm.target.as_str())
                .unwrap_or(child_mapping.as_str());
            fs::write(dir.join(format!("lists_{parent_target}.sparql")), c)?;
        }

        for (mapping, q_existing) in &self.reverse_existing {
            let q_inserts = &self.reverse_inserts[mapping];
            let rev_iri = reverse_graph_iri(mapping);
            let combined = format!(
                "# Maintains: GRAPH <{rev_iri}>\n# Existing source rows\n{q_existing}\n\n# Insert candidates (in canonical, no source row)\n{q_inserts}"
            );
            fs::write(dir.join(format!("reverse_{mapping}.sparql")), combined)?;
        }

        for (child_mapping, q) in &self.frame_constructs {
            fs::write(dir.join(format!("framing_{child_mapping}.sparql")), q)?;
        }

        for (child_mapping, frame) in &self.frame_documents {
            fs::write(
                dir.join(format!("frame_{child_mapping}.jsonld")),
                serde_json::to_string_pretty(frame)?,
            )?;
        }

        Ok(())
    }
}

fn run_construct(store: &Store, q: &str) -> Result<Vec<(NamedNode, NamedNode, Term)>> {
    let res = store
        .query(q)
        .with_context(|| format!("CONSTRUCT failed:\n{q}"))?;
    let QueryResults::Graph(it) = res else {
        anyhow::bail!("expected graph results from CONSTRUCT");
    };
    let mut out = Vec::new();
    for triple in it {
        let triple = triple?;
        let oxigraph::model::Subject::NamedNode(s) = triple.subject else {
            continue; // skip blank/triple subjects in slice 1
        };
        out.push((s, triple.predicate, triple.object));
    }
    Ok(out)
}

/// Run a CONSTRUCT query and insert all resulting triples into `graph_iri`
/// in the store.  This is how the in-process executor simulates an
/// incrementally maintained triplestore: it applies each CONSTRUCT rule
/// once, writing results to the same named graph the rule declares.
fn run_construct_into_graph(store: &Store, construct: &str, graph_iri: &str) -> Result<()> {
    // Strip the `# Maintains: GRAPH <IRI>` annotation line if present so
    // Oxigraph's query parser sees pure SPARQL.
    let q = construct
        .lines()
        .filter(|l| !l.trim_start().starts_with("# "))
        .collect::<Vec<_>>()
        .join("\n");
    let triples = run_construct(store, &q)?;
    let g =
        NamedNodeRef::new(graph_iri).with_context(|| format!("invalid graph IRI: {graph_iri}"))?;
    for (s, p, o) in triples {
        store.insert(oxigraph::model::QuadRef::new(
            s.as_ref(),
            p.as_ref(),
            o.as_ref(),
            g,
        ))?;
    }
    Ok(())
}

fn group_by_subject(
    triples: &[(NamedNode, NamedNode, Term)],
    m: &crate::model::Mapping,
    pk_col: Option<&str>,
    has_existing_pk: bool,
) -> Result<Vec<(String, Row)>> {
    // Build pred-IRI → source-column-name map for this mapping's fields.
    let pred_to_col: HashMap<String, String> = m
        .fields
        .iter()
        .map(|fm| (source_prop_iri(&m.name, &fm.source), fm.source.clone()))
        .collect();

    let mut by_subj: BTreeMap<String, Row> = BTreeMap::new();
    for (s, p, o) in triples {
        let subj = s.as_str().to_string();
        let pred = p.as_str().to_string();
        let Some(col) = pred_to_col.get(&pred) else {
            continue;
        };
        let val = match o {
            Term::Literal(l) => serde_yaml::Value::String(l.value().to_string()),
            Term::NamedNode(n) => serde_yaml::Value::String(n.as_str().to_string()),
            _ => serde_yaml::Value::Null,
        };
        by_subj.entry(subj).or_default().insert(col.clone(), val);
    }

    let mut rows = Vec::new();
    for (subj, mut row) in by_subj {
        // Make sure every mapped field appears (null if absent).
        for fm in &m.fields {
            row.entry(fm.source.clone())
                .or_insert(serde_yaml::Value::Null);
        }
        let synth_pk = if has_existing_pk {
            let prefix = format!("{}source/{}/", base(), m.name);
            let suffix = subj.strip_prefix(&prefix).unwrap_or("");
            decode_pk(suffix)
        } else {
            // Insert: subject is the canonical IRI itself; use it as the
            // matching key (no diff against inputs is performed).
            subj.clone()
        };
        if has_existing_pk {
            if let Some(c) = pk_col {
                row.insert(c.to_string(), serde_yaml::Value::String(synth_pk.clone()));
            }
        } else {
            if let Some(c) = pk_col {
                row.insert(c.to_string(), serde_yaml::Value::Null);
            }
            row.insert(
                "_canonical_id".to_string(),
                serde_yaml::Value::String(subj.clone()),
            );
        }
        rows.push((synth_pk, row));
    }
    Ok(rows)
}

/// Returns the PK column name for a top-level mapping, or `None` for
/// child mappings (which use the synthetic `_row_pk` not exposed to
/// users).
fn mapping_pk_col<'a>(m: &crate::model::Mapping, doc: &'a Doc) -> Result<Option<&'a str>> {
    if m.parent.is_some() {
        return Ok(None);
    }
    let src = doc
        .sources
        .get(&m.source)
        .ok_or_else(|| anyhow!("mapping {} → unknown source {}", m.name, m.source))?;
    Ok(Some(src.primary_key.as_str()))
}

/// Expand a mapping's input rows into the per-subject lifted rows used
/// by both JSON-LD lift and reverse-projection diffing.
///
/// - Top-level mapping → returns `(real_pk_text, original_row)` for
///   every input row in `inputs[m.source]`.
/// - Child mapping → looks up parent input rows in
///   `inputs[parent.source]`, expands the array column with ordinal `i`,
///   and synthesises rows containing parent_fields aliases + element
///   fields + the parent's last_modified column (if declared on the
///   child mapping). Synthetic PK is `<parent_pk>:<i>`.
fn expand_lifted_rows(
    m: &crate::model::Mapping,
    doc: &Doc,
    inputs: &HashMap<String, Vec<Row>>,
) -> Result<Vec<(String, Row)>> {
    if let Some(parent_name) = &m.parent {
        let array_col = m
            .array
            .as_ref()
            .ok_or_else(|| anyhow!("mapping {} has parent: but no array:", m.name))?;
        let parent = doc
            .mappings
            .iter()
            .find(|p| p.name == *parent_name)
            .ok_or_else(|| anyhow!("mapping {} parent `{}` not found", m.name, parent_name))?;
        let parent_src = doc.sources.get(&parent.source).ok_or_else(|| {
            anyhow!(
                "mapping {} parent source `{}` unknown",
                m.name,
                parent.source
            )
        })?;
        let Some(parent_rows) = inputs.get(&parent.source) else {
            return Ok(vec![]);
        };
        let mut out: Vec<(String, Row)> = Vec::new();
        for parent_row in parent_rows {
            let parent_pk = parent_row
                .get(&parent_src.primary_key)
                .and_then(yaml_to_text)
                .ok_or_else(|| {
                    anyhow!(
                        "row in source {} missing PK {}",
                        parent.source,
                        parent_src.primary_key
                    )
                })?;
            let elements = match parent_row.get(array_col) {
                Some(serde_yaml::Value::Sequence(s)) => s.clone(),
                Some(serde_yaml::Value::Null) | None => continue,
                Some(other) => {
                    anyhow::bail!(
                        "mapping {}: expected array at {}.{}, got {:?}",
                        m.name,
                        parent.source,
                        array_col,
                        other
                    );
                }
            };
            for (i, elem) in elements.iter().enumerate() {
                let synth_pk = format!("{parent_pk}:{i}");
                let mut row: Row = IndexMap::new();
                // parent_fields aliases first.
                for (alias, parent_col) in &m.parent_fields {
                    if let Some(v) = parent_row.get(parent_col) {
                        row.insert(alias.clone(), v.clone());
                    }
                }
                // Element fields (override aliases with same key).
                if let serde_yaml::Value::Mapping(elem_map) = elem {
                    for (k, v) in elem_map {
                        if let serde_yaml::Value::String(name) = k {
                            row.insert(name.clone(), v.clone());
                        }
                    }
                }
                // last_modified from parent if mapping declares it and not
                // already overridden.
                if let Some(lm) = &m.last_modified {
                    if !row.contains_key(lm) {
                        if let Some(v) = parent_row.get(lm) {
                            row.insert(lm.clone(), v.clone());
                        }
                    }
                }
                out.push((synth_pk, row));
            }
        }
        Ok(out)
    } else {
        let src = doc
            .sources
            .get(&m.source)
            .ok_or_else(|| anyhow!("mapping {} → unknown source {}", m.name, m.source))?;
        let Some(rows) = inputs.get(&m.source) else {
            return Ok(vec![]);
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let pk = row
                .get(&src.primary_key)
                .and_then(yaml_to_text)
                .with_context(|| {
                    format!("row in source {} missing PK {}", m.source, src.primary_key)
                })?;
            out.push((pk, row.clone()));
        }
        Ok(out)
    }
}

/// Drop columns that callers shouldn't see (currently `_canonical_id`).
fn strip_internal_columns(row: &Row) -> Row {
    let mut out = row.clone();
    out.shift_remove("_canonical_id");
    out
}

/// Slice 3b-proper: SPARQL-side child aggregation via JSON-LD framing.
///
/// 1. Issue a CONSTRUCT producing an RDF graph with:
///      - one synthesised `rdf:type` triple per child canonical entity,
///      - the child's mapped canonical properties,
///      - and a synthetic `osi:embedFor/<parent>/<child>` predicate
///        from each child to the literal parent linkage value.
/// 2. Convert the CONSTRUCT result into [`Triple`]s.
/// 3. Build a [`Frame`] describing the child shape — `@type` matches the
///    child target IRI, scalar props for each non-linkage child field,
///    sort by the child target's identity components.
/// 4. Apply the frame, grouped by the linkage scalar.
/// 5. Encode each group's array via [`canonical_json_string`] so the
///    bytes match PG's `jsonb_agg(jsonb_build_object(...) ORDER BY ...)`
///    output exactly.
///
/// Slice 3b-proper: SPARQL-side child aggregation via JSON-LD framing
/// over a triplestore-native RDF list.
///
/// 1. Build the parent-rooted frame + feeder CONSTRUCT via
///    [`build_child_frame`]. The frame uses `@container: "@list"`;
///    the CONSTRUCT consumes the materialised `osi:hasChild/<array>`
///    + `rdf:first`/`rdf:rest` chain from the lists graph.
/// 2. Run the CONSTRUCT, convert results to [`framing::Triple`]s.
/// 3. Apply the frame — one framed parent object per parent canonical
///    IRI, with children already in RDF-list order.
/// 4. Group by the synthetic `_p_link` scalar (parent's source-column
///    linkage value), strip framing metadata from children, and
///    canonical-JSON-encode for the round-trip diff.
///
/// Order is determined entirely by the triplestore's RDF list — the
/// engine does no post-framing sort. The ordering rule lives in
/// [`build_list_update`] and runs as published SPARQL UPDATE.
fn run_child_agg(
    store: &Store,
    parent: &crate::model::Mapping,
    child: &crate::model::Mapping,
    doc: &Doc,
) -> Result<HashMap<String, String>> {
    use crate::render::framing::{apply_frame, Triple, TripleObject};

    let _ = parent;
    let array_key = child
        .array
        .clone()
        .ok_or_else(|| anyhow!("child `{}` has no array:", child.name))?;
    let (frame, q) = build_child_frame(doc, child)?;

    let res = store
        .query(&q)
        .with_context(|| format!("child framing CONSTRUCT failed:\n{q}"))?;
    let QueryResults::Graph(it) = res else {
        anyhow::bail!("expected graph results from child framing CONSTRUCT");
    };

    let mut triples: Vec<Triple> = Vec::new();
    for triple in it {
        let triple = triple?;
        let oxigraph::model::Subject::NamedNode(s) = triple.subject else {
            continue;
        };
        let pred = triple.predicate.as_str().to_string();
        let obj = match triple.object {
            Term::NamedNode(n) => TripleObject::Iri(n.as_str().to_string()),
            Term::Literal(l) => {
                TripleObject::Literal(serde_json::Value::String(l.value().to_string()))
            }
            _ => continue,
        };
        triples.push(Triple {
            subject: s.as_str().to_string(),
            predicate: pred,
            object: obj,
        });
    }

    // Apply frame → one framed parent object per parent canonical IRI.
    let framed_parents = apply_frame(&triples, &frame);

    let mut out: HashMap<String, String> = HashMap::new();
    for parent_obj in framed_parents {
        let serde_json::Value::Object(map) = parent_obj else {
            continue;
        };
        // Group key: synthetic `_p_link` scalar.
        let p_link = match map.get("_p_link") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => continue,
        };
        // Embedded array sits under the child's `array:` JSON key.
        let arr = match map.get(&array_key) {
            Some(serde_json::Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        // Strip framing metadata from each embedded child so the bytes
        // match PG's jsonb_build_object output.
        let cleaned: Vec<serde_json::Value> = arr
            .into_iter()
            .map(|mut o| {
                if let serde_json::Value::Object(m) = &mut o {
                    m.remove("@id");
                    m.remove("@type");
                }
                o
            })
            .collect();
        out.insert(
            p_link,
            canonical_json_string(&serde_json::Value::Array(cleaned)),
        );
    }
    Ok(out)
}

/// Build the [`Frame`] + feeder CONSTRUCT for one parent×child pair.
///
/// The frame is rooted at the **parent** target type, with the child
/// elements appearing as a nested frame under the `array:` property,
/// declared as a JSON-LD `@container: "@list"`. The actual ordering
/// lives in the triplestore as an RDF list (`rdf:first`/`rdf:rest`)
/// in the `<base>/lists/<parent_target>` graph, materialised by
/// [`materialise_child_lists`] (driven by [`build_list_update`]).
/// The framer just walks the list — it does not sort.
///
/// Output frame shape (illustrative):
///
/// ```jsonc
/// {
///   "@type": "<parent_target_type_iri>",
///   "_p_link": { "@id": "<parent_link_predicate>" },
///   "<array>": {
///     "@type": "<child_target_type_iri>",
///     "@container": "@list",
///     "@id": "<base>/hasChild/<array>",
///     "<child_field_src>": { "@id": "<child_field_predicate>" },
///     ...
///   }
/// }
/// ```
///
/// `_p_link` is an execution sentinel scalar carrying the parent's
/// linkage value (the source-column key); the executor uses it to
/// key the framed parent objects against the parent's reverse rows.
/// It's stripped from the published frame document and from emitted
/// child objects.
fn build_child_frame(
    doc: &Doc,
    child: &crate::model::Mapping,
) -> Result<(crate::render::framing::Frame, String)> {
    use crate::render::framing::{Frame, FrameProp};

    let parent_name = child.parent.as_ref().ok_or_else(|| {
        anyhow!(
            "build_child_frame called on non-child mapping `{}`",
            child.name
        )
    })?;
    let parent = doc
        .mappings
        .iter()
        .find(|m| m.name == *parent_name)
        .ok_or_else(|| anyhow!("child `{}` parent `{}` not found", child.name, parent_name))?;
    let array_key = child
        .array
        .clone()
        .ok_or_else(|| anyhow!("child `{}` has no array:", child.name))?;
    let _ = doc
        .targets
        .get(&child.target)
        .ok_or_else(|| anyhow!("mapping {} → unknown target {}", child.name, child.target))?;

    let (_child_alias_src, parent_src_col) = child
        .parent_fields
        .iter()
        .next()
        .ok_or_else(|| anyhow!("child `{}` has no parent_fields", child.name))?;
    let parent_link_target = parent
        .fields
        .iter()
        .find(|fm| fm.source == *parent_src_col)
        .ok_or_else(|| {
            anyhow!(
                "parent `{}`: linkage source column `{}` (referenced by child `{}`) not declared in fields",
                parent.name,
                parent_src_col,
                child.name
            )
        })?
        .target
        .clone();

    let alias_keys: std::collections::HashSet<&str> =
        child.parent_fields.keys().map(|s| s.as_str()).collect();
    let elem_fields: Vec<(String, String)> = child
        .fields
        .iter()
        .filter(|fm| !alias_keys.contains(fm.source.as_str()))
        .map(|fm| (fm.source.clone(), fm.target.clone()))
        .collect();

    let parent_type_iri = format!("{}target/{}", base(), parent.target);
    let child_type_iri = format!("{}target/{}", base(), child.target);
    let parent_link_pred = source_prop_iri(&parent.name, parent_src_col);
    let has_child_pred = format!("{}hasChild/{}", base(), array_key);
    let lists_g = lists_graph_iri(&parent.target);
    let parent_canonical_g = canonical_graph_iri(&parent.target);
    let child_canonical_g = canonical_graph_iri(&child.target);

    // Feeder CONSTRUCT: project the parent's identity scalar (so
    // grouping works downstream), the parent → list-head edge, the
    // full rdf:first/rdf:rest chain, and each child's mapped scalars.
    // The list is consumed *as-is* from the lists graph — no sorting
    // here, no synthetic joins. Order lives in RDF.
    let mut construct_lines = vec![
        format!("  ?p <{RDF_TYPE}> <{parent_type_iri}> ."),
        format!("  ?p <{parent_link_pred}> ?link ."),
        format!("  ?p <{has_child_pred}> ?head ."),
        format!("  ?cell <{RDF_FIRST}> ?c ."),
        format!("  ?cell <{RDF_REST}> ?next ."),
        format!("  ?c <{RDF_TYPE}> <{child_type_iri}> ."),
    ];
    for (src, _tgt) in &elem_fields {
        construct_lines.push(format!(
            "  ?c <{}> ?{} .",
            source_prop_iri(&child.name, src),
            sanitize_var(src)
        ));
    }

    let mut where_lines = vec![
        format!(
            "  GRAPH <{parent_canonical_g}> {{ ?p <{prop}> ?link . }}",
            prop = canonical_prop_iri(&parent_link_target),
        ),
        format!(
            "  GRAPH <{lists_g}> {{ ?p <{has_child_pred}> ?head . ?head <{RDF_REST}>* ?cell . ?cell <{RDF_FIRST}> ?c . ?cell <{RDF_REST}> ?next . }}"
        ),
    ];
    for (src, tgt) in &elem_fields {
        where_lines.push(format!(
            "  GRAPH <{child_canonical_g}> {{ OPTIONAL {{ ?c <{prop}> ?{var} . }} }}",
            prop = canonical_prop_iri(tgt),
            var = sanitize_var(src),
        ));
    }
    let construct = format!(
        "CONSTRUCT {{\n{}\n}}\nWHERE {{\n{}\n}}",
        construct_lines.join("\n"),
        where_lines.join("\n"),
    );

    let child_props: Vec<FrameProp> = elem_fields
        .iter()
        .map(|(src, _)| FrameProp::Scalar {
            name: src.clone(),
            predicate: source_prop_iri(&child.name, src),
        })
        .collect();
    let child_frame = Frame {
        root_type: child_type_iri,
        properties: child_props,
    };

    let parent_props = vec![
        FrameProp::Scalar {
            name: "_p_link".to_string(),
            predicate: parent_link_pred,
        },
        FrameProp::EmbedList {
            name: array_key,
            predicate: has_child_pred,
            child_frame: Box::new(child_frame),
        },
    ];
    let frame = Frame {
        root_type: parent_type_iri,
        properties: parent_props,
    };

    Ok((frame, construct))
}

/// Build the multi-step SPARQL UPDATE that materialises one child
/// mapping's RDF list in the parent target's lists graph.
///
/// The UPDATE has three steps separated by `;`:
///
/// 1. **DELETE** any prior `osi:hasChild/<array>` head edges and
///    every cell reachable from them, scoped to the parents this
///    mapping touches. Idempotent re-execution is therefore safe.
/// 2. **INSERT** cell triples (`?cell rdf:first ?c . ?cell rdf:rest ?next .`)
///    where `?cell` and `?next` are computed from a per-pair rank,
///    and `?next` falls through to `rdf:nil` for the last cell.
/// 3. **INSERT** head edges (`?p osi:hasChild/<array> ?head`) for
///    parents that have at least one child (head = rank-0 cell).
///
/// **Ordering rule** (in the WHERE of step 2): per-pair rank is
/// `COUNT(?c2)` of strictly-earlier siblings of `?c` under the same
/// parent. "Strictly-earlier" is defined by lexicographic comparison
/// of the child target's identity components (in declared order),
/// with `STR(?c)` IRI as a final tiebreaker. CRDT strategies plug in
/// here by replacing the FILTER expression \u2014 the rest of the
/// UPDATE (cell IRI synthesis, head wiring) stays the same.
///
/// **Why pure SPARQL UPDATE?** Order is a first-class triplestore
/// concern in v2; the materialiser is the canonical place for it
/// (see CRDT examples). Keeping it as inspectable SPARQL means
/// downstream consumers can audit, override, or replay the rule
/// without going through the engine.
/// Shared components computed by both the UPDATE and CONSTRUCT forms of the
/// list materialisation step.  Extracted so the two builder functions don't
/// duplicate the setup logic.
struct ListParts {
    lists_g: String,
    has_child_pred: String,
    cell_prefix: String,
    parent_canonical_g: String,
    child_canonical_g: String,
    parent_link_pred: String,
    child_alias_pred: String,
    linkage_block: String,
    rank_subquery: String,
    count_subquery: String,
}

fn build_list_parts(doc: &Doc, child: &crate::model::Mapping) -> Result<ListParts> {
    let parent_name = child.parent.as_ref().ok_or_else(|| {
        anyhow!(
            "build_list_parts called on non-child mapping `{}`",
            child.name
        )
    })?;
    let parent = doc
        .mappings
        .iter()
        .find(|m| m.name == *parent_name)
        .ok_or_else(|| anyhow!("child `{}` parent `{}` not found", child.name, parent_name))?;
    let array_key = child
        .array
        .clone()
        .ok_or_else(|| anyhow!("child `{}` has no array:", child.name))?;
    let child_target = doc
        .targets
        .get(&child.target)
        .ok_or_else(|| anyhow!("mapping {} → unknown target {}", child.name, child.target))?;

    let (child_alias_src, parent_src_col) = child
        .parent_fields
        .iter()
        .next()
        .ok_or_else(|| anyhow!("child `{}` has no parent_fields", child.name))?;
    let child_alias_target = child
        .fields
        .iter()
        .find(|fm| fm.source == *child_alias_src)
        .ok_or_else(|| {
            anyhow!(
                "child `{}`: parent_fields alias `{}` not declared in fields",
                child.name,
                child_alias_src
            )
        })?
        .target
        .clone();
    let parent_link_target = parent
        .fields
        .iter()
        .find(|fm| fm.source == *parent_src_col)
        .ok_or_else(|| {
            anyhow!(
                "parent `{}`: linkage source column `{}` not declared in fields",
                parent.name,
                parent_src_col
            )
        })?
        .target
        .clone();

    let alias_targets: std::collections::HashSet<&str> = child
        .parent_fields
        .keys()
        .filter_map(|src| {
            child
                .fields
                .iter()
                .find(|fm| fm.source == *src)
                .map(|fm| fm.target.as_str())
        })
        .collect();
    let mut order_targets: Vec<String> = Vec::new();
    for ig in &child_target.identity {
        let fields: Vec<String> = match ig {
            IdentityGroup::Single(f) => vec![f.clone()],
            IdentityGroup::Tuple(fs) => fs.clone(),
        };
        for f in fields {
            if !alias_targets.contains(f.as_str()) {
                order_targets.push(f);
            }
        }
    }

    let parent_canonical_g = canonical_graph_iri(&parent.target);
    let child_canonical_g = canonical_graph_iri(&child.target);
    let lists_g = lists_graph_iri(&parent.target);
    let parent_link_pred = canonical_prop_iri(&parent_link_target);
    let child_alias_pred = canonical_prop_iri(&child_alias_target);
    let has_child_pred = format!("{}hasChild/{}", base(), array_key);
    let cell_prefix = format!("/cell/{array_key}/");

    let linkage_block = format!(
        "    GRAPH <{parent_canonical_g}> {{ ?p <{parent_link_pred}> ?link . }}\n    GRAPH <{child_canonical_g}> {{ ?c <{child_alias_pred}> ?link . }}",
    );

    let mut order_binds_c = String::new();
    let mut order_binds_c2 = String::new();
    let mut order_var_pairs: Vec<(String, String)> = Vec::new();
    for (i, t) in order_targets.iter().enumerate() {
        let prop = canonical_prop_iri(t);
        order_binds_c.push_str(&format!(
            "      GRAPH <{child_canonical_g}> {{ OPTIONAL {{ ?c <{prop}> ?v{i} . }} }}\n",
        ));
        order_binds_c2.push_str(&format!(
            "      GRAPH <{child_canonical_g}> {{ OPTIONAL {{ ?c2 <{prop}> ?w{i} . }} }}\n",
        ));
        order_var_pairs.push((
            format!("STR(COALESCE(?v{i}, \"\"))"),
            format!("STR(COALESCE(?w{i}, \"\"))"),
        ));
    }
    order_var_pairs.push(("STR(?c)".to_string(), "STR(?c2)".to_string()));

    let mut filter_clauses: Vec<String> = Vec::new();
    for k in 0..order_var_pairs.len() {
        let mut parts: Vec<String> = Vec::new();
        for (i, (cval, c2val)) in order_var_pairs.iter().enumerate().take(k) {
            parts.push(format!("({c2val} = {cval})"));
            let _ = i;
        }
        let (cval_k, c2val_k) = &order_var_pairs[k];
        parts.push(format!("({c2val_k} < {cval_k})"));
        filter_clauses.push(parts.join(" && "));
    }
    let strictly_earlier = filter_clauses
        .iter()
        .map(|c| format!("({c})"))
        .collect::<Vec<_>>()
        .join(" || ");

    let rank_subquery = format!(
        "    {{\n      SELECT ?p ?c (COUNT(?c2) AS ?rank) WHERE {{\n{linkage_block}\n        OPTIONAL {{\n          GRAPH <{child_canonical_g}> {{ ?c2 <{child_alias_pred}> ?link . }}\n{order_binds_c}{order_binds_c2}          FILTER ({strictly_earlier})\n        }}\n      }} GROUP BY ?p ?c\n    }}"
    );

    let count_subquery = format!(
        "    {{\n      SELECT ?p (COUNT(?c3) AS ?n) WHERE {{\n        GRAPH <{parent_canonical_g}> {{ ?p <{parent_link_pred}> ?l2 . }}\n        GRAPH <{child_canonical_g}> {{ ?c3 <{child_alias_pred}> ?l2 . }}\n      }} GROUP BY ?p\n    }}"
    );

    Ok(ListParts {
        lists_g,
        has_child_pred,
        cell_prefix,
        parent_canonical_g,
        child_canonical_g,
        parent_link_pred,
        child_alias_pred,
        linkage_block,
        rank_subquery,
        count_subquery,
    })
}

fn build_list_construct(doc: &Doc, child: &crate::model::Mapping) -> Result<String> {
    let p = build_list_parts(doc, child)?;
    let ListParts {
        lists_g,
        has_child_pred,
        cell_prefix,
        parent_canonical_g,
        child_canonical_g,
        parent_link_pred,
        child_alias_pred,
        linkage_block,
        rank_subquery,
        count_subquery,
    } = &p;

    // CONSTRUCT 1: cell triples (rdf:first / rdf:rest)
    let cells = format!(
        "# Maintains: GRAPH <{lists_g}>\n# Cell triples: rdf:first and rdf:rest for each ranked child\nCONSTRUCT {{\n  ?cell <{RDF_FIRST}> ?c .\n  ?cell <{RDF_REST}> ?next .\n}}\nWHERE {{\n{linkage_block}\n{rank_subquery}\n{count_subquery}\n  BIND (IRI(CONCAT(STR(?p), \"{cell_prefix}\", STR(?rank))) AS ?cell)\n  BIND (IF(?rank + 1 < ?n,\n           IRI(CONCAT(STR(?p), \"{cell_prefix}\", STR(?rank + 1))),\n           <{RDF_NIL}>) AS ?next)\n}}"
    );

    // CONSTRUCT 2: head edges (?p osi:hasChild/<array> ?head)
    let heads = format!(
        "# Head edges: ?p <{has_child_pred}> ?head for non-empty parents\nCONSTRUCT {{\n  ?p <{has_child_pred}> ?head .\n}}\nWHERE {{\n  GRAPH <{parent_canonical_g}> {{ ?p <{parent_link_pred}> ?link . }}\n  FILTER EXISTS {{ GRAPH <{child_canonical_g}> {{ ?cx <{child_alias_pred}> ?link . }} }}\n  BIND (IRI(CONCAT(STR(?p), \"{cell_prefix}\", \"0\")) AS ?head)\n}}"
    );

    Ok(format!("{cells}\n\n{heads}"))
}

/// Stable, key-sorted, whitespace-free JSON encoding for round-trip
/// equality checks. Booleans/numbers preserved; objects sort their keys
/// lexicographically.
pub fn canonical_json_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => serde_json::to_string(s).unwrap(),
        serde_json::Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(canonical_json_string).collect();
            format!("[{}]", parts.join(","))
        }
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k.as_str()).unwrap(),
                        canonical_json_string(&m[k.as_str()])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn row_differs_cols(proj: &Row, orig: &Row, cols: &[String]) -> bool {
    for c in cols {
        let p = proj.get(c).cloned().unwrap_or(serde_yaml::Value::Null);
        let o = orig.get(c).cloned().unwrap_or(serde_yaml::Value::Null);
        if yaml_to_text(&p) != yaml_to_text(&o) {
            return true;
        }
    }
    false
}

fn yaml_to_text(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::Null => None,
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::String(s) => Some(s.clone()),
        // Compound values: emit canonical JSON so child-array agg
        // round-trip comparisons agree byte-for-byte across PG and
        // SPARQL backends.
        serde_yaml::Value::Sequence(_) | serde_yaml::Value::Mapping(_) => {
            let json = yaml_to_json(v);
            Some(canonical_json_string(&json))
        }
        _ => Some(serde_yaml::to_string(v).unwrap_or_default()),
    }
}

fn yaml_to_json(v: &serde_yaml::Value) -> serde_json::Value {
    match v {
        serde_yaml::Value::Null => serde_json::Value::Null,
        serde_yaml::Value::Bool(b) => serde_json::Value::Bool(*b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::String(n.to_string())
            }
        }
        serde_yaml::Value::String(s) => serde_json::Value::String(s.clone()),
        // Slice 1: no nested values — stringify defensively.
        other => serde_json::Value::String(serde_yaml::to_string(other).unwrap_or_default()),
    }
}

// Suppress unused warning; NamedNodeRef is currently unused but useful as
// future reverse projection lands.
#[allow(dead_code)]
fn _nn(s: &str) -> Result<NamedNodeRef<'_>> {
    NamedNodeRef::new(s).map_err(|e| anyhow!("invalid IRI {s}: {e}"))
}

// ---------------------------------------------------------------------------
// CLI artifact rendering.
// ---------------------------------------------------------------------------

impl fmt::Display for SparqlPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Named graphs are the SPARQL equivalent of SQL views — each one is
        // a persistent, inspectable artifact you can query at any point:
        //
        //   SELECT * WHERE { GRAPH <IRI> { ?s ?p ?o } }
        //
        // Pipeline (execution order):
        //   LIFT → IDENTITY → FORWARD → LIST → REVERSE → delta
        //
        // Persistent named graphs (one block per graph below):
        //   <base>/sourcegraph/<mapping>    populated by LIFT
        //   <base>/identity/<target>        populated by IDENTITY UPDATE
        //   <base>/canonical/<target>       populated by FORWARD UPDATEs
        //   <base>/lists/<parent_target>    populated by LIST UPDATE  (parent/child only)
        //   <base>/reverse/<mapping>        populated by REVERSE UPDATE
        //
        // The REVERSE UPDATE is the deployable counterpart of the two
        // ephemeral CONSTRUCT queries the in-process executor uses for
        // diffing.  On a real triplestore, run these UPDATEs in order and
        // then query <reverse/<mapping>> to see the canonical state in
        // source shape — analogous to the SQL <M>_reverse view.
        //
        // Delta detection (which rows to UPDATE/INSERT/DELETE in the source
        // system) is not yet expressed as named graphs; the in-process
        // executor computes it by diffing <reverse/<mapping>> against the
        // source rows.  A future slice will emit those as SPARQL queries too.

        writeln!(f, "# Base IRI: {}", self.base_iri)?;
        writeln!(f)?;

        for m in &self.doc.mappings {
            let parent_note = match &m.parent {
                Some(p) => format!(
                    "  child of `{p}` via array `{}`",
                    m.array.as_deref().unwrap_or("?")
                ),
                None => String::new(),
            };
            writeln!(
                f,
                "━━━ mapping `{}` → target `{}`{parent_note} ━━━",
                m.name, m.target
            )?;
            writeln!(f)?;

            // ── LIFT ──────────────────────────────────────────────
            // artifact: GRAPH <base>/sourcegraph/<mapping>
            // The executor JSON-LD-expands each source row using this
            // context and loads the result into the source graph.
            if let Some(ctx) = self.contexts.get(&m.name) {
                writeln!(
                    f,
                    "## GRAPH <{base}sourcegraph/{mapping}>  [LIFT]",
                    base = self.base_iri,
                    mapping = m.name
                )?;
                writeln!(
                    f,
                    "## populated by: JSON-LD expand of source rows (context below)"
                )?;
                writeln!(
                    f,
                    "{}",
                    serde_json::to_string_pretty(ctx).unwrap_or_default()
                )?;
                writeln!(f)?;
            }

            // ── IDENTITY ─────────────────────────────────────────
            // artifact: GRAPH <base>/identity/<target>
            if let Some(c) = self.identity_constructs.get(&m.target) {
                writeln!(
                    f,
                    "## GRAPH <{base}identity/{target}>  [IDENTITY CONSTRUCT]",
                    base = self.base_iri,
                    target = m.target
                )?;
                writeln!(
                    f,
                    "## reads:  <{base}sourcegraph/{mapping}>",
                    base = self.base_iri,
                    mapping = m.name
                )?;
                writeln!(f, "{c}")?;
                writeln!(f)?;
            }

            // ── FORWARD ───────────────────────────────────────────
            // artifact: GRAPH <base>/canonical/<target>
            // One CONSTRUCT per target field; priority + decl_order
            // tiebreak mirrors the SQL coalesce strategy.
            let fwd_keys: Vec<&str> = self
                .forward_constructs
                .keys()
                .filter(|k| k.starts_with(&format!("{}.", m.target)))
                .map(|k| k.as_str())
                .collect();
            if !fwd_keys.is_empty() {
                writeln!(
                    f,
                    "## GRAPH <{base}canonical/{target}>  [FORWARD CONSTRUCTs]",
                    base = self.base_iri,
                    target = m.target
                )?;
                writeln!(
                    f,
                    "## reads:  <{base}sourcegraph/{mapping}> + <{base}identity/{target}>",
                    base = self.base_iri,
                    mapping = m.name,
                    target = m.target
                )?;
                for k in &fwd_keys {
                    let field = k.trim_start_matches(&format!("{}.", m.target));
                    writeln!(f, "### field `{field}`")?;
                    writeln!(f, "{}", self.forward_constructs[*k])?;
                    writeln!(f)?;
                }
            }

            // ── LIST ─────────────────────────────────────────────
            // artifact: GRAPH <base>/lists/<parent_target>
            if let Some(c) = self.list_constructs.get(&m.name) {
                let parent_target = m
                    .parent
                    .as_ref()
                    .and_then(|pn| self.doc.mappings.iter().find(|pm| pm.name == *pn))
                    .map(|pm| pm.target.as_str())
                    .unwrap_or("?");
                writeln!(
                    f,
                    "## GRAPH <{base}lists/{parent_target}>  [LIST CONSTRUCTs]",
                    base = self.base_iri,
                )?;
                writeln!(
                    f,
                    "## reads:  <{base}canonical/{parent_target}> + <{base}canonical/{child}>",
                    base = self.base_iri,
                    child = m.target
                )?;
                writeln!(
                    f,
                    "## ordering: lexicographic by child identity, IRI as tiebreaker"
                )?;
                writeln!(f, "{c}")?;
                writeln!(f)?;
            }

            // ── REVERSE ──────────────────────────────────────────
            // artifact: GRAPH <base>/reverse/<mapping>
            // CONSTRUCTs define the reverse projection (canonical → source
            // shape). The executor runs them and inserts results into the
            // named graph; a deployed incremental triplestore maintains
            // it automatically.
            if let Some(q) = self.reverse_existing.get(&m.name) {
                let rev_iri = reverse_graph_iri(&m.name);
                writeln!(f, "## GRAPH <{rev_iri}>  [REVERSE CONSTRUCTs]",)?;
                writeln!(
                    f,
                    "## reads:  <{base}sourcegraph/{mapping}> + <{base}identity/{target}> + <{base}canonical/{target}>",
                    base = self.base_iri,
                    mapping = m.name,
                    target = m.target
                )?;
                writeln!(f, "### existing source rows")?;
                writeln!(f, "{q}")?;
                writeln!(f)?;
                if let Some(qi) = self.reverse_inserts.get(&m.name) {
                    writeln!(f, "### insert candidates (in canonical, no source row)")?;
                    writeln!(f, "{qi}")?;
                    writeln!(f)?;
                }
            }

            // ── FRAMING ─────────────────────────────────────────
            // No persistent graph — CONSTRUCT output is fed directly
            // into the in-process JSON-LD framer.  The framed parent
            // objects (with embedded child @list) are merged into the
            // parent reverse rows before the delta diff runs.
            //
            // On a deployed triplestore, running the framing CONSTRUCT
            // gives you the full parent+children shape as RDF, which the
            // client can frame with the JSON-LD frame document below.
            if let Some(q) = self.frame_constructs.get(&m.name) {
                let parent_target = m
                    .parent
                    .as_ref()
                    .and_then(|pn| self.doc.mappings.iter().find(|pm| pm.name == *pn))
                    .map(|pm| pm.target.as_str())
                    .unwrap_or("?");
                writeln!(
                    f,
                    "## (query)  [FRAMING CONSTRUCT — child `{}` embedded into parent `{}`]",
                    m.name,
                    m.parent.as_deref().unwrap_or("?")
                )?;
                writeln!(
                    f,
                    "## reads:  <{base}canonical/{parent_target}> + <{base}lists/{parent_target}> + <{base}canonical/{child}>",
                    base = self.base_iri,
                    child = m.target
                )?;
                writeln!(
                    f,
                    "## output: RDF graph → JSON-LD framer (frame doc below) → embedded @list in parent reverse rows"
                )?;
                writeln!(f, "{q}")?;
                writeln!(f)?;
            }
            if let Some(frame) = self.frame_documents.get(&m.name) {
                writeln!(f, "## JSON-LD frame document (consumed by framer above)")?;
                writeln!(
                    f,
                    "{}",
                    serde_json::to_string_pretty(frame).unwrap_or_default()
                )?;
                writeln!(f)?;
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_file;
    use std::path::PathBuf;

    fn hello_world_doc() -> Doc {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples/hello-world/mapping.yaml");
        parse_file(&path).expect("parse hello-world")
    }

    #[test]
    fn renders_hello_world_artifacts() {
        let doc = hello_world_doc();
        let plan = render_sparql(&doc).unwrap();

        assert_eq!(plan.contexts.len(), 2, "one context per mapping");
        assert!(plan.identity_constructs.contains_key("contact"));
        assert!(plan.forward_constructs.contains_key("contact.email"));
        assert!(plan.forward_constructs.contains_key("contact.name"));
        assert!(plan.reverse_existing.contains_key("crm"));
        assert!(plan.reverse_inserts.contains_key("erp"));

        // Spot-check shape of the artifact.
        let id_c = &plan.identity_constructs["contact"];
        assert!(
            id_c.contains("CONSTRUCT"),
            "identity is a CONSTRUCT: {id_c}"
        );
        assert!(id_c.contains("SHA256"), "canonical IRI uses SHA256: {id_c}");
        assert!(
            id_c.contains("identity/contact"),
            "targets identity graph: {id_c}"
        );

        let fwd = &plan.forward_constructs["contact.name"];
        assert!(
            fwd.contains("FILTER NOT EXISTS"),
            "forward picks via NOT EXISTS"
        );
        assert!(
            fwd.contains("VALUES"),
            "compile-time priorities live in VALUES"
        );
    }
}
