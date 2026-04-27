//! Minimal JSON-LD framing engine — a focused subset of the
//! [JSON-LD 1.1 framing](https://www.w3.org/TR/json-ld11-framing/)
//! algorithm, sufficient for v2's parent-with-embedded-children shape.
//!
//! ## Why hand-rolled?
//!
//! The full framing algorithm is large and the available crates
//! (`json-ld 0.21`) carry a heavy async / generic surface. v2's framing
//! needs are narrow:
//!
//! - One root type per frame (the parent target).
//! - Per-property embed of related nodes via a known IRI predicate.
//! - Stable element ordering driven by an explicit sort key (matching
//!   the PG backend's `jsonb_agg ORDER BY` clause).
//!
//! That subset fits in this module. The output of [`apply_frame`]
//! conforms to the framing-spec's "framed expanded form" minus
//! `@context` injection (callers can add a `@context` if they want
//! compact form).
//!
//! ## What it is NOT
//!
//! - It does not implement `@embed: @never` / `@always` / `@link`.
//!   Children are always embedded once.
//! - It does not implement frame matching beyond `@type` equality.
//! - It does not handle `@reverse`, `@nest`, `@included`, `@graph`
//!   inside frames, or frame inheritance.
//!
//! Callers that need full framing semantics should plug in the
//! `json-ld` crate later — the public surface here ([`Frame`],
//! [`Triple`], [`apply_frame`]) is intentionally small to make that
//! swap painless.

use indexmap::IndexMap;
use std::collections::HashMap;

/// A single RDF triple as consumed by the framer.
///
/// Subjects and predicates are IRIs. Objects are either IRIs (for
/// node-to-node links, e.g. embed edges) or scalar values (literals).
/// Blank nodes aren't supported; v2 mappings always emit named
/// canonical IRIs.
#[derive(Debug, Clone)]
pub struct Triple {
    pub subject: String,
    pub predicate: String,
    pub object: TripleObject,
}

#[derive(Debug, Clone)]
pub enum TripleObject {
    /// IRI reference; will participate in node-to-node embedding.
    Iri(String),
    /// Literal value (string, int, bool); becomes a JSON scalar.
    Literal(serde_json::Value),
}

/// One property in a [`Frame`].
#[derive(Debug, Clone)]
pub enum FrameProp {
    /// Scalar property: copy the literal value through into the framed
    /// object under this property name.
    ///
    /// `predicate` is the full IRI; `name` is the JSON output key.
    Scalar { name: String, predicate: String },

    /// Embedded RDF-list property: follow `predicate` from the current
    /// node to a list head, walk the `rdf:first` / `rdf:rest` chain to
    /// `rdf:nil`, frame each element node according to `child_frame`,
    /// and emit the framed elements as a JSON array under `name`.
    ///
    /// Order is whatever the RDF list says it is — this is the whole
    /// point. The triplestore is the source of truth for ordering;
    /// callers do not (and must not) re-sort.
    EmbedList {
        name: String,
        predicate: String,
        child_frame: Box<Frame>,
    },
}

/// A frame document. Currently only matches by `root_type` (rdf:type).
#[derive(Debug, Clone)]
pub struct Frame {
    /// Full IRI that root nodes must have as their `rdf:type` to match.
    pub root_type: String,
    /// Properties in the order they should appear in the framed output.
    pub properties: Vec<FrameProp>,
}

impl Frame {
    /// Serialise this frame as a standard JSON-LD 1.1 frame document.
    ///
    /// - Root `@type` selects matching nodes.
    /// - Each scalar property maps its JSON name to `{"@id": "<predicate>"}`.
    /// - Each embedded array property is a nested frame with
    ///   `@container: "@set"`.
    ///
    /// **Not included:** properties whose names begin with `_` (execution
    /// sentinels such as `_p_link`) and sort ordering (JSON-LD framing has
    /// no ordering concept; sorting is applied after framing by the engine
    /// internally via `EmbedArray::sort_keys`).
    pub fn to_json(&self) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        out.insert(
            "@type".to_string(),
            serde_json::Value::String(self.root_type.clone()),
        );
        for fp in &self.properties {
            match fp {
                FrameProp::Scalar { name, predicate } => {
                    // Skip internal execution sentinels.
                    if name.starts_with('_') {
                        continue;
                    }
                    let mut o = serde_json::Map::new();
                    o.insert(
                        "@id".to_string(),
                        serde_json::Value::String(predicate.clone()),
                    );
                    out.insert(name.clone(), serde_json::Value::Object(o));
                }
                FrameProp::EmbedList {
                    name,
                    predicate,
                    child_frame,
                } => {
                    let mut o = match child_frame.to_json() {
                        serde_json::Value::Object(m) => m,
                        _ => serde_json::Map::new(),
                    };
                    o.insert(
                        "@id".to_string(),
                        serde_json::Value::String(predicate.clone()),
                    );
                    // @list = ordered; ordering lives in RDF as
                    // rdf:first/rdf:rest chains.
                    o.insert(
                        "@container".to_string(),
                        serde_json::Value::String("@list".into()),
                    );
                    out.insert(name.clone(), serde_json::Value::Object(o));
                }
            }
        }
        serde_json::Value::Object(out)
    }
}

/// IRI of `rdf:type`.
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
/// IRI of `rdf:first`.
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
/// IRI of `rdf:rest`.
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
/// IRI of `rdf:nil`.
pub const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

/// Apply `frame` to `triples`, returning one framed JSON object per
/// matching root subject. Output is sorted by subject IRI for
/// determinism; callers can re-sort by their own keys if they need a
/// different order.
pub fn apply_frame(triples: &[Triple], frame: &Frame) -> Vec<serde_json::Value> {
    let by_subject = index_by_subject(triples);
    let mut roots: Vec<&String> = by_subject
        .iter()
        .filter_map(|(s, props)| {
            if has_type(props, &frame.root_type) {
                Some(s)
            } else {
                None
            }
        })
        .collect();
    roots.sort();
    roots
        .into_iter()
        .map(|s| frame_subject(s, &by_subject, frame))
        .collect()
}

/// Apply a frame and return framed roots indexed by the value of one
/// of the root's scalar properties (a "linkage key" — typically the
/// child's parent_fields alias). Values that share a key are grouped
/// into an array.
///
/// This is the convenience the SPARQL backend uses to look up "all
/// framed children belonging to parent `<P>`": we frame the children
/// once, then index by the linkage scalar to attach them to the
/// parent's reverse row.
pub fn apply_frame_grouped_by(
    triples: &[Triple],
    frame: &Frame,
    group_key: &str,
) -> HashMap<String, Vec<serde_json::Value>> {
    let framed = apply_frame(triples, frame);
    let mut out: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for mut obj in framed {
        let Some(serde_json::Value::Object(map)) = Some(&mut obj) else {
            continue;
        };
        let key = match map.get(group_key).cloned() {
            Some(serde_json::Value::String(s)) => s,
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => continue,
        };
        // Drop the linkage key from the embedded object; it duplicates
        // the parent's column and would be redundant in the framed
        // child shape.
        if let serde_json::Value::Object(map) = &mut obj {
            map.remove(group_key);
        }
        out.entry(key).or_default().push(obj);
    }
    out
}

// ---------------------------------------------------------------------------
// Internals.
// ---------------------------------------------------------------------------

type SubjectIndex = HashMap<String, IndexMap<String, Vec<TripleObject>>>;

fn index_by_subject(triples: &[Triple]) -> SubjectIndex {
    let mut out: SubjectIndex = HashMap::new();
    for t in triples {
        out.entry(t.subject.clone())
            .or_default()
            .entry(t.predicate.clone())
            .or_default()
            .push(t.object.clone());
    }
    out
}

fn has_type(props: &IndexMap<String, Vec<TripleObject>>, type_iri: &str) -> bool {
    let Some(types) = props.get(RDF_TYPE) else {
        return false;
    };
    types.iter().any(|o| match o {
        TripleObject::Iri(i) => i == type_iri,
        _ => false,
    })
}

fn frame_subject(subject: &str, idx: &SubjectIndex, frame: &Frame) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    out.insert(
        "@id".to_string(),
        serde_json::Value::String(subject.to_string()),
    );
    out.insert(
        "@type".to_string(),
        serde_json::Value::String(frame.root_type.clone()),
    );
    let Some(props) = idx.get(subject) else {
        return serde_json::Value::Object(out);
    };
    for fp in &frame.properties {
        match fp {
            FrameProp::Scalar { name, predicate } => {
                if let Some(values) = props.get(predicate) {
                    if let Some(v) = values
                        .iter()
                        .map(|o| match o {
                            TripleObject::Literal(j) => j.clone(),
                            TripleObject::Iri(i) => serde_json::Value::String(i.clone()),
                        })
                        .next()
                    {
                        out.insert(name.clone(), v);
                    }
                }
            }
            FrameProp::EmbedList {
                name,
                predicate,
                child_frame,
            } => {
                let mut children: Vec<serde_json::Value> = Vec::new();
                if let Some(values) = props.get(predicate) {
                    // The predicate may name multiple list heads (rare
                    // in practice for v2). Walk each.
                    for o in values {
                        let TripleObject::Iri(head_iri) = o else {
                            continue;
                        };
                        walk_rdf_list(head_iri, idx, child_frame, &mut children);
                    }
                }
                out.insert(name.clone(), serde_json::Value::Array(children));
            }
        }
    }
    serde_json::Value::Object(out)
}

/// Walk an RDF list starting from `head_iri`, framing each `rdf:first`
/// element via `child_frame` and appending to `out`. Cycles are
/// guarded against (a malformed graph that loops will be truncated).
fn walk_rdf_list(
    head_iri: &str,
    idx: &SubjectIndex,
    child_frame: &Frame,
    out: &mut Vec<serde_json::Value>,
) {
    let mut current = head_iri.to_string();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        if current == RDF_NIL || !seen.insert(current.clone()) {
            return;
        }
        let Some(props) = idx.get(&current) else {
            return;
        };
        if let Some(firsts) = props.get(RDF_FIRST) {
            if let Some(TripleObject::Iri(elem)) = firsts.first() {
                out.push(frame_subject(elem, idx, child_frame));
            }
        }
        let Some(rests) = props.get(RDF_REST) else {
            return;
        };
        let Some(TripleObject::Iri(next)) = rests.first() else {
            return;
        };
        current = next.clone();
    }
}
