//! Throwaway spike for the v2 SPARQL renderer contract.
//!
//! Pipeline (per test):
//!   1. lift   — load source rows into per-source named graphs as triples
//!   2. ident  — cluster source rows whose identity fields share a value
//!   3. fwd    — for each canonical entity, resolve each field by priority
//!               and write triples into the canonical graph
//!   4. rev    — for each test source, project canonical triples back into
//!               source-PK-shaped records via SPARQL
//!   5. diff   — compare projected records to expected and report
//!
//! Everything is hardcoded: no YAML parser, no model. We're validating
//! the SPARQL contract, not the engine architecture.

use anyhow::{Context, Result};
use indexmap::IndexMap;
use oxigraph::model::{GraphName, Literal, NamedNode, Quad, Subject, Term};
use oxigraph::sparql::{QueryResults, QuerySolutionIter};
use oxigraph::store::Store;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

const BASE: &str = "https://example.test/";

fn iri(s: &str) -> NamedNode {
    NamedNode::new(s).expect("valid IRI")
}

fn lit(s: &str) -> Literal {
    Literal::new_simple_literal(s)
}

/// Minimal-escape PK encoding from the v2 spec: replace `%` then `/`.
fn encode_pk_component(s: &str) -> String {
    s.replace('%', "%25").replace('/', "%2F")
}

/// Source-row IRI: <base>/source/<mapping_name>/<encoded_pk>.
fn source_iri(mapping: &str, pk: &str) -> NamedNode {
    iri(&format!(
        "{}source/{}/{}",
        BASE,
        mapping,
        encode_pk_component(pk)
    ))
}

/// Source-row property IRI: <base>/sourceprop/<mapping_name>/<field>.
/// Distinct from canonical properties so we can rename via SPARQL.
fn source_prop_iri(mapping: &str, field: &str) -> NamedNode {
    iri(&format!("{}sourceprop/{}/{}", BASE, mapping, field))
}

/// Canonical-entity IRI: <base>/canonical/<target>/<sha256(identity_value)>.
fn canonical_iri(target: &str, identity_value: &str) -> NamedNode {
    let mut h = Sha256::new();
    h.update(identity_value.as_bytes());
    let digest = hex::encode_short(&h.finalize());
    iri(&format!("{}canonical/{}/{}", BASE, target, digest))
}

/// Canonical-property IRI: <base>/prop/<field>.
fn canonical_prop_iri(field: &str) -> NamedNode {
    iri(&format!("{}prop/{}", BASE, field))
}

/// Source-graph IRI for a given mapping: <base>/sourcegraph/<mapping_name>.
fn source_graph_iri(mapping: &str) -> NamedNode {
    iri(&format!("{}sourcegraph/{}", BASE, mapping))
}

/// Canonical-graph IRI for a target: <base>/canonical/<target>.
fn canonical_graph_iri(target: &str) -> NamedNode {
    iri(&format!("{}canonical/{}", BASE, target))
}

mod hex {
    pub fn encode_short(bytes: &[u8]) -> String {
        // first 16 chars of hex digest is plenty for spike uniqueness
        let mut s = String::with_capacity(16);
        for b in &bytes[..8] {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Mapping definition (hardcoded for hello-world)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct FieldMap {
    source: &'static str,
    target: &'static str,
    priority: Option<i32>,
}

#[derive(Debug, Clone)]
struct Mapping {
    name: &'static str,
    source: &'static str,
    target: &'static str,
    pk: &'static str, // primary_key (single field for hello-world)
    fields: Vec<FieldMap>,
}

#[derive(Debug, Clone)]
struct TargetDef {
    name: &'static str,
    /// OR-groups of identity (each group is a single field for hello-world).
    identity: Vec<&'static str>,
    /// Field resolution strategies (only "coalesce" needed for hello-world).
    fields: Vec<&'static str>,
}

struct Scenario {
    name: &'static str,
    targets: Vec<TargetDef>,
    mappings: Vec<Mapping>,
    cases: Vec<TestCase>,
}

fn hello_world_scenario() -> Scenario {
    let targets = vec![TargetDef {
        name: "contact",
        identity: vec!["email"],
        fields: vec!["email", "name"],
    }];
    let mappings = vec![
        Mapping {
            name: "crm",
            source: "crm",
            target: "contact",
            pk: "id",
            fields: vec![
                FieldMap {
                    source: "email",
                    target: "email",
                    priority: None,
                },
                FieldMap {
                    source: "name",
                    target: "name",
                    priority: Some(1),
                },
            ],
        },
        Mapping {
            name: "erp",
            source: "erp",
            target: "contact",
            pk: "id",
            fields: vec![
                FieldMap {
                    source: "contact_email",
                    target: "email",
                    priority: None,
                },
                FieldMap {
                    source: "contact_name",
                    target: "name",
                    priority: Some(2),
                },
            ],
        },
    ];
    Scenario {
        name: "hello-world",
        targets,
        mappings,
        cases: vec![test_1_shared(), test_2_crm_only(), test_3_erp_only()],
    }
}

fn merge_threeway_scenario() -> Scenario {
    // Three sources for `customer`; identity = email OR phone.
    // Tests transitive closure: A links to B by email, B links to C by phone
    // → all three end up in the same canonical entity.
    let targets = vec![TargetDef {
        name: "customer",
        identity: vec!["email", "phone"],
        fields: vec!["email", "phone", "name"],
    }];
    let mappings = vec![
        Mapping {
            name: "crm",
            source: "crm",
            target: "customer",
            pk: "id",
            fields: vec![
                FieldMap { source: "email", target: "email", priority: None },
                FieldMap { source: "phone", target: "phone", priority: None },
                FieldMap { source: "name", target: "name", priority: Some(1) },
            ],
        },
        Mapping {
            name: "erp",
            source: "erp",
            target: "customer",
            pk: "id",
            fields: vec![
                FieldMap { source: "email", target: "email", priority: None },
                FieldMap { source: "phone", target: "phone", priority: None },
                FieldMap { source: "name", target: "name", priority: Some(2) },
            ],
        },
        Mapping {
            name: "billing",
            source: "billing",
            target: "customer",
            pk: "id",
            fields: vec![
                FieldMap { source: "email", target: "email", priority: None },
                FieldMap { source: "phone", target: "phone", priority: None },
                FieldMap { source: "name", target: "name", priority: Some(3) },
            ],
        },
    ];
    Scenario {
        name: "merge-threeway",
        targets,
        mappings,
        cases: vec![test_threeway_transitive()],
    }
}

// ---------------------------------------------------------------------------
// Test fixtures (hello-world test 1)
// ---------------------------------------------------------------------------

type Row = IndexMap<&'static str, Value>;

fn row(pairs: &[(&'static str, Value)]) -> Row {
    pairs.iter().cloned().collect()
}

#[derive(Debug)]
struct TestCase {
    description: &'static str,
    /// source name → list of rows.
    input: HashMap<&'static str, Vec<Row>>,
    /// source name → expected updates (PK → projected row).
    expected_updates: HashMap<&'static str, Vec<Row>>,
    /// source name → expected number of inserts (canonical entities with no
    /// source row in this mapping).
    expected_inserts: HashMap<&'static str, usize>,
}

fn test_1_shared() -> TestCase {
    let mut input = HashMap::new();
    input.insert(
        "crm",
        vec![row(&[
            ("id", json!("1")),
            ("email", json!("alice@example.com")),
            ("name", json!("Alice")),
        ])],
    );
    input.insert(
        "erp",
        vec![row(&[
            ("id", json!("100")),
            ("contact_email", json!("alice@example.com")),
            ("contact_name", json!("A. Smith")),
        ])],
    );
    let mut expected = HashMap::new();
    expected.insert(
        "erp",
        vec![row(&[
            ("id", json!("100")),
            ("contact_email", json!("alice@example.com")),
            ("contact_name", json!("Alice")),
        ])],
    );
    let mut inserts = HashMap::new();
    inserts.insert("crm", 0);
    inserts.insert("erp", 0);
    TestCase {
        description: "Shared contact — CRM name wins",
        input,
        expected_updates: expected,
        expected_inserts: inserts,
    }
}

fn test_2_crm_only() -> TestCase {
    // CRM has Bob, ERP has nobody → ERP gets 1 insert, CRM 0 updates.
    let mut input = HashMap::new();
    input.insert(
        "crm",
        vec![row(&[
            ("id", json!("2")),
            ("email", json!("bob@example.com")),
            ("name", json!("Bob")),
        ])],
    );
    input.insert("erp", vec![]);
    let expected = HashMap::new(); // no rows projected back match existing PKs
    let mut inserts = HashMap::new();
    inserts.insert("crm", 0);
    inserts.insert("erp", 1);
    TestCase {
        description: "CRM-only contact — ERP gets insert",
        input,
        expected_updates: expected,
        expected_inserts: inserts,
    }
}

fn test_3_erp_only() -> TestCase {
    // ERP has Carol, CRM has nobody → CRM gets 1 insert.
    let mut input = HashMap::new();
    input.insert("crm", vec![]);
    input.insert(
        "erp",
        vec![row(&[
            ("id", json!("200")),
            ("contact_email", json!("carol@example.com")),
            ("contact_name", json!("Carol")),
        ])],
    );
    let expected = HashMap::new();
    let mut inserts = HashMap::new();
    inserts.insert("crm", 1);
    inserts.insert("erp", 0);
    TestCase {
        description: "ERP-only contact — CRM gets insert",
        input,
        expected_updates: expected,
        expected_inserts: inserts,
    }
}

fn test_threeway_transitive() -> TestCase {
    // CRM (id=1) shares email with ERP (id=10).
    // ERP (id=10) shares phone with Billing (id=B).
    // CRM has no direct overlap with Billing.
    // Expected: all three rows form one canonical customer.
    // Name resolves to CRM's value (priority 1).
    let mut input = HashMap::new();
    input.insert(
        "crm",
        vec![row(&[
            ("id", json!("1")),
            ("email", json!("x@a.com")),
            ("phone", json!("111")),
            ("name", json!("Xavier-CRM")),
        ])],
    );
    input.insert(
        "erp",
        vec![row(&[
            ("id", json!("10")),
            ("email", json!("x@a.com")),
            ("phone", json!("222")),
            ("name", json!("Xavier-ERP")),
        ])],
    );
    input.insert(
        "billing",
        vec![row(&[
            ("id", json!("B")),
            ("email", json!("different@b.com")),
            ("phone", json!("222")),
            ("name", json!("Xavier-BILL")),
        ])],
    );
    // After resolution every source row maps to the same canonical entity.
    // So projecting back, every row whose name differs from CRM's wins-name
    // becomes an update. Name flows from CRM (priority 1) to all sources.
    // Email: CRM and ERP have x@a.com; Billing has different@b.com.
    //   Lowest-priority among contributors is CRM → "x@a.com" wins.
    // Phone: ERP and Billing have different values; CRM has 111.
    //   CRM (priority None=MAX, but tie-broken by declaration order) has prio MAX.
    //   Actually all three have priority None for email/phone. Tie-breaker is
    //   first-declared mapping (CRM). CRM phone = "111".
    let mut expected: HashMap<&'static str, Vec<Row>> = HashMap::new();
    // CRM: name was Xavier-CRM, wins → unchanged. email/phone unchanged.
    //   No update for CRM.
    // ERP: email becomes x@a.com (was x@a.com → unchanged), phone becomes 111
    //   (was 222 → changed), name becomes Xavier-CRM (was Xavier-ERP → changed).
    expected.insert(
        "erp",
        vec![row(&[
            ("id", json!("10")),
            ("email", json!("x@a.com")),
            ("phone", json!("111")),
            ("name", json!("Xavier-CRM")),
        ])],
    );
    // Billing: email becomes x@a.com (was different@b.com), phone becomes 111
    //   (was 222), name becomes Xavier-CRM.
    expected.insert(
        "billing",
        vec![row(&[
            ("id", json!("B")),
            ("email", json!("x@a.com")),
            ("phone", json!("111")),
            ("name", json!("Xavier-CRM")),
        ])],
    );
    let mut inserts = HashMap::new();
    inserts.insert("crm", 0);
    inserts.insert("erp", 0);
    inserts.insert("billing", 0);
    TestCase {
        description: "Three-way transitive identity (email ↔ phone)",
        input,
        expected_updates: expected,
        expected_inserts: inserts,
    }
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

fn lift(store: &Store, mappings: &[Mapping], input: &HashMap<&str, Vec<Row>>) -> Result<()> {
    for m in mappings {
        let Some(rows) = input.get(m.source) else {
            continue;
        };
        let g = source_graph_iri(m.name);
        for r in rows {
            let pk_val = r
                .get(m.pk)
                .and_then(|v| v.as_str())
                .with_context(|| format!("row missing PK {}", m.pk))?;
            let subj_iri = source_iri(m.name, pk_val);
            for fm in &m.fields {
                let Some(v) = r.get(fm.source) else {
                    continue;
                };
                if v.is_null() {
                    continue;
                }
                let Some(s) = v.as_str() else { continue };
                let q = Quad::new(
                    Subject::NamedNode(subj_iri.clone()),
                    source_prop_iri(m.name, fm.source),
                    Term::Literal(lit(s)),
                    GraphName::NamedNode(g.clone()),
                );
                store.insert(&q)?;
            }
        }
    }
    Ok(())
}

/// Compute identity clusters using union-find for transitive closure.
/// Two source rows end up in the same canonical entity if any of their
/// declared identity-field values match (across all identity fields and all
/// mappings for that target).
fn cluster_identity(
    store: &Store,
    targets: &[TargetDef],
    mappings: &[Mapping],
) -> Result<HashMap<NamedNode, NamedNode>> {
    let mut row_to_canonical = HashMap::new();
    for t in targets {
        // Collect (row, field_index, value) triples from each mapping for this target.
        let mut row_idents: BTreeMap<NamedNode, Vec<(usize, String)>> = BTreeMap::new();
        let mut by_value: HashMap<(usize, String), Vec<NamedNode>> = HashMap::new();
        for (fi, ident_field) in t.identity.iter().enumerate() {
            for m in mappings.iter().filter(|m| m.target == t.name) {
                let Some(fm) = m.fields.iter().find(|fm| fm.target == *ident_field) else {
                    continue;
                };
                let q = format!(
                    r#"
                    SELECT ?row ?val WHERE {{
                      GRAPH <{}> {{
                        ?row <{}> ?val .
                      }}
                    }}
                    "#,
                    source_graph_iri(m.name).as_str(),
                    source_prop_iri(m.name, fm.source).as_str(),
                );
                for sol in run_select(store, &q)? {
                    let row_iri = sol.get("row").and_then(term_as_iri).unwrap();
                    let val = sol.get("val").and_then(term_as_str).unwrap();
                    row_idents
                        .entry(row_iri.clone())
                        .or_default()
                        .push((fi, val.clone()));
                    by_value.entry((fi, val)).or_default().push(row_iri);
                }
            }
        }

        // Union-find over all rows that have at least one identity value.
        let mut parent: HashMap<NamedNode, NamedNode> = row_idents
            .keys()
            .map(|r| (r.clone(), r.clone()))
            .collect();
        for members in by_value.values() {
            for w in members.windows(2) {
                union(&mut parent, &w[0], &w[1]);
            }
        }

        // Group rows by their root.
        let mut clusters: BTreeMap<NamedNode, Vec<NamedNode>> = BTreeMap::new();
        let row_keys: Vec<NamedNode> = row_idents.keys().cloned().collect();
        for r in row_keys {
            let root = find(&mut parent, &r);
            clusters.entry(root).or_default().push(r);
        }

        // Canonical IRI per cluster: hash sorted (field_idx, value) pairs.
        for (_root, members) in clusters {
            let mut all_idents: Vec<(usize, String)> = members
                .iter()
                .flat_map(|r| row_idents.get(r).cloned().unwrap_or_default())
                .collect();
            all_idents.sort();
            all_idents.dedup();
            let key = all_idents
                .iter()
                .map(|(fi, v)| format!("{}|{}", fi, v))
                .collect::<Vec<_>>()
                .join("\n");
            let cid = canonical_iri(t.name, &key);
            for r in members {
                row_to_canonical.insert(r, cid.clone());
            }
        }
    }
    Ok(row_to_canonical)
}

fn find(parent: &mut HashMap<NamedNode, NamedNode>, x: &NamedNode) -> NamedNode {
    let p = parent.get(x).cloned().unwrap_or_else(|| x.clone());
    if p == *x {
        return p;
    }
    let root = find(parent, &p);
    parent.insert(x.clone(), root.clone());
    root
}

fn union(parent: &mut HashMap<NamedNode, NamedNode>, a: &NamedNode, b: &NamedNode) {
    let ra = find(parent, a);
    let rb = find(parent, b);
    if ra != rb {
        parent.insert(ra, rb);
    }
}

/// Forward pass: for each canonical entity, resolve every target field by
/// picking the contributing source-row value with the lowest priority number.
/// Insert canonical triples into <base>/canonical/<target>.
fn forward(
    store: &Store,
    targets: &[TargetDef],
    mappings: &[Mapping],
    row_to_canonical: &HashMap<NamedNode, NamedNode>,
) -> Result<()> {
    // Gather all source-row triples grouped by canonical entity.
    // For each (canonical, target_field), pick the value from the
    // contributing mapping with the lowest priority (tie → first declared).
    for t in targets {
        let g = canonical_graph_iri(t.name);
        for tfield in &t.fields {
            // Collect (canonical_iri, value, priority) candidates.
            let mut candidates: HashMap<NamedNode, Vec<(i32, String, usize)>> = HashMap::new();
            for (mi, m) in mappings.iter().enumerate().filter(|(_, m)| m.target == t.name) {
                let Some(fm) = m.fields.iter().find(|fm| fm.target == *tfield) else {
                    continue;
                };
                let prio = fm.priority.unwrap_or(i32::MAX);
                let q = format!(
                    r#"
                    SELECT ?row ?val WHERE {{
                      GRAPH <{}> {{
                        ?row <{}> ?val .
                      }}
                    }}
                    "#,
                    source_graph_iri(m.name).as_str(),
                    source_prop_iri(m.name, fm.source).as_str(),
                );
                for sol in run_select(store, &q)? {
                    let row_iri = sol.get("row").and_then(term_as_iri).unwrap();
                    let val = sol.get("val").and_then(term_as_str).unwrap();
                    let Some(can) = row_to_canonical.get(&row_iri) else {
                        continue;
                    };
                    candidates
                        .entry(can.clone())
                        .or_default()
                        .push((prio, val, mi));
                }
            }
            for (can, mut cands) in candidates {
                cands.sort_by_key(|c| (c.0, c.2));
                let (_, val, _) = cands.into_iter().next().unwrap();
                let q = Quad::new(
                    Subject::NamedNode(can),
                    canonical_prop_iri(tfield),
                    Term::Literal(lit(&val)),
                    GraphName::NamedNode(g.clone()),
                );
                store.insert(&q)?;
            }
        }
    }
    Ok(())
}

/// Reverse pass: for a given mapping, project the canonical triples back to
/// source-shaped rows. For each source row currently in the source graph,
/// look up its canonical entity, fetch the canonical values for each mapped
/// target field, and reshape into the source's row form.
/// Returns: list of (pk, projected_row).
fn reverse_project(
    store: &Store,
    m: &Mapping,
    row_to_canonical: &HashMap<NamedNode, NamedNode>,
) -> Result<Vec<Row>> {
    let mut out = Vec::new();
    // Find each source-row IRI in this mapping's source graph.
    let q = format!(
        r#"
        SELECT DISTINCT ?row WHERE {{
          GRAPH <{}> {{ ?row ?p ?o . }}
        }}
        "#,
        source_graph_iri(m.name).as_str()
    );
    for sol in run_select(store, &q)? {
        let row_iri = sol.get("row").and_then(term_as_iri).unwrap();
        // Extract PK from IRI suffix.
        let prefix = format!("{}source/{}/", BASE, m.name);
        let suffix = row_iri.as_str().strip_prefix(&prefix).unwrap();
        let pk_val = suffix.replace("%2F", "/").replace("%25", "%");
        let mut projected: Row = IndexMap::new();
        projected.insert(
            // Re-use the literal &'static str by leaking; spike-only.
            Box::leak(m.pk.to_string().into_boxed_str()),
            json!(pk_val),
        );
        // For each mapped field, fetch canonical value.
        let Some(can) = row_to_canonical.get(&row_iri) else {
            // Source row has no canonical entity (e.g. identity fields all null).
            for fm in &m.fields {
                projected.insert(
                    Box::leak(fm.source.to_string().into_boxed_str()),
                    Value::Null,
                );
            }
            out.push(projected);
            continue;
        };
        for fm in &m.fields {
            let q = format!(
                r#"
                SELECT ?val WHERE {{
                  GRAPH <{}> {{
                    <{}> <{}> ?val .
                  }}
                }} LIMIT 1
                "#,
                canonical_graph_iri(m.target).as_str(),
                can.as_str(),
                canonical_prop_iri(fm.target).as_str(),
            );
            let val = run_select(store, &q)?
                .into_iter()
                .next()
                .and_then(|s| s.get("val").and_then(term_as_str))
                .map(Value::String)
                .unwrap_or(Value::Null);
            projected.insert(Box::leak(fm.source.to_string().into_boxed_str()), val);
        }
        out.push(projected);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_select(store: &Store, q: &str) -> Result<Vec<HashMap<String, Term>>> {
    let res = store.query(q).context("SPARQL SELECT failed")?;
    let QueryResults::Solutions(it) = res else {
        anyhow::bail!("expected SELECT results");
    };
    let it: QuerySolutionIter = it;
    let mut out = Vec::new();
    for sol in it {
        let sol = sol?;
        let mut m = HashMap::new();
        for (var, term) in sol.iter() {
            m.insert(var.as_str().to_string(), term.clone());
        }
        out.push(m);
    }
    Ok(out)
}

fn term_as_str(t: &Term) -> Option<String> {
    match t {
        Term::Literal(l) => Some(l.value().to_string()),
        _ => None,
    }
}

fn term_as_iri(t: &Term) -> Option<NamedNode> {
    match t {
        Term::NamedNode(n) => Some(n.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Test runner
// ---------------------------------------------------------------------------

fn run_test(scenario: &Scenario, case: &TestCase) -> Result<()> {
    let store = Store::new()?;
    lift(&store, &scenario.mappings, &case.input)?;
    let row_to_canonical = cluster_identity(&store, &scenario.targets, &scenario.mappings)?;
    forward(&store, &scenario.targets, &scenario.mappings, &row_to_canonical)?;

    println!("--- [{}] {} ---", scenario.name, case.description);
    println!("  store has {} quads", store.len()?);

    let mut all_pass = true;

    // Per-source updates: project current source rows back, diff against input.
    for (src_name, expected) in &case.expected_updates {
        let m = scenario
            .mappings
            .iter()
            .find(|m| m.source == *src_name)
            .unwrap();
        let projected = reverse_project(&store, m, &row_to_canonical)?;
        let inputs = case.input.get(src_name).cloned().unwrap_or_default();
        let mut updates: Vec<Row> = Vec::new();
        for proj in &projected {
            let pk = proj.get(m.pk).cloned().unwrap();
            let Some(orig) = inputs.iter().find(|r| r.get(m.pk) == Some(&pk)) else {
                continue;
            };
            let mut differs = false;
            for fm in &m.fields {
                if proj.get(fm.source) != orig.get(fm.source) {
                    differs = true;
                    break;
                }
            }
            if differs {
                updates.push(proj.clone());
            }
        }
        if rows_eq(&updates, expected) {
            println!("  {}.updates: PASS ({} rows)", src_name, updates.len());
        } else {
            all_pass = false;
            println!("  {}.updates: FAIL", src_name);
            println!("    expected: {}", pretty(expected));
            println!("    actual:   {}", pretty(&updates));
        }
    }

    // Per-source inserts: count canonical entities for the target that have
    // no source row in this mapping.
    for (src_name, expected_count) in &case.expected_inserts {
        let m = scenario
            .mappings
            .iter()
            .find(|m| m.source == *src_name)
            .unwrap();
        let count = count_inserts(&store, m, &row_to_canonical)?;
        if count == *expected_count {
            println!("  {}.inserts: PASS ({} rows)", src_name, count);
        } else {
            all_pass = false;
            println!(
                "  {}.inserts: FAIL  expected {}, got {}",
                src_name, expected_count, count
            );
        }
    }

    if !all_pass {
        anyhow::bail!("test failed");
    }
    Ok(())
}

/// Count canonical entities for `mapping.target` that have no source-row
/// from `mapping` mapped into them.
fn count_inserts(
    store: &Store,
    mapping: &Mapping,
    row_to_canonical: &HashMap<NamedNode, NamedNode>,
) -> Result<usize> {
    // Distinct canonical entities for this target = distinct values in row_to_canonical
    // for rows whose graph belongs to a mapping with the same target.
    // Simpler: take all canonical values, dedup. Then for each canonical, check if
    // any source row in `mapping`'s source graph maps to it.
    let mut canonicals_in_mapping: std::collections::HashSet<NamedNode> = Default::default();
    let mut all_canonicals: std::collections::HashSet<NamedNode> = Default::default();
    let prefix = format!("{}source/{}/", BASE, mapping.name);
    for (row, can) in row_to_canonical {
        all_canonicals.insert(can.clone());
        if row.as_str().starts_with(&prefix) {
            canonicals_in_mapping.insert(can.clone());
        }
    }
    // We want canonicals belonging to this mapping's target only. Approximate by
    // checking the canonical IRI prefix.
    let target_prefix = format!("{}canonical/{}/", BASE, mapping.target);
    let target_canonicals: std::collections::HashSet<NamedNode> = all_canonicals
        .into_iter()
        .filter(|c| c.as_str().starts_with(&target_prefix))
        .collect();
    let _ = store; // silence warning; lookups are via row_to_canonical
    Ok(target_canonicals.difference(&canonicals_in_mapping).count())
}

fn rows_eq(a: &[Row], b: &[Row]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let to_canon = |r: &Row| -> BTreeMap<String, Value> {
        r.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    };
    let mut a_set: Vec<_> = a.iter().map(to_canon).collect();
    let mut b_set: Vec<_> = b.iter().map(to_canon).collect();
    a_set.sort_by_key(|m| serde_json::to_string(m).unwrap_or_default());
    b_set.sort_by_key(|m| serde_json::to_string(m).unwrap_or_default());
    a_set == b_set
}

fn pretty(rows: &[Row]) -> String {
    let arr: Vec<Value> = rows
        .iter()
        .map(|r| {
            let m: Map<String, Value> =
                r.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            Value::Object(m)
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_default()
}

fn main() -> Result<()> {
    let scenarios = [hello_world_scenario(), merge_threeway_scenario()];
    let mut failures = 0;
    let mut total = 0;
    for s in &scenarios {
        for case in &s.cases {
            total += 1;
            if let Err(e) = run_test(s, case) {
                failures += 1;
                eprintln!("FAILED: {e}");
            }
        }
    }
    if failures > 0 {
        eprintln!("\n{failures}/{total} test(s) failed");
        std::process::exit(1);
    }
    println!("\nall {total} tests passed");
    Ok(())
}
