use std::collections::BTreeMap;

use crate::model::MappingDocument;

/// A node in the view dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ViewNode {
    /// External source table (not created by engine).
    Source(String),
    /// Forward mapping view.
    Forward(String),
    /// Identity/transitive closure view for a target.
    Identity(String),
    /// Resolved golden record view for a target.
    Resolved(String),
    /// Reverse mapping view.
    Reverse(String),
    /// Delta/changeset view.
    Delta(String),
}

impl ViewNode {
    pub fn view_name(&self) -> String {
        match self {
            ViewNode::Source(name) => name.clone(),
            ViewNode::Forward(name) => format!("_fwd_{name}"),
            ViewNode::Identity(name) => format!("_id_{name}"),
            ViewNode::Resolved(name) => format!("_resolved_{name}"),
            ViewNode::Reverse(name) => format!("_rev_{name}"),
            ViewNode::Delta(name) => format!("_delta_{name}"),
        }
    }

    pub fn label(&self) -> String {
        match self {
            ViewNode::Source(name) => format!("SRC: {name}"),
            ViewNode::Forward(name) => format!("FWD: {name}"),
            ViewNode::Identity(name) => format!("ID: {name}"),
            ViewNode::Resolved(name) => format!("RES: {name}"),
            ViewNode::Reverse(name) => format!("REV: {name}"),
            ViewNode::Delta(name) => format!("DELTA: {name}"),
        }
    }
}

/// The DAG of view dependencies.
#[derive(Debug)]
pub struct ViewDag {
    /// Edges: node → list of nodes it depends on.
    pub edges: BTreeMap<ViewNode, Vec<ViewNode>>,
    /// Topologically sorted creation order (dependencies first).
    pub order: Vec<ViewNode>,
}

/// Build the view dependency graph from a mapping document.
pub fn build_dag(doc: &MappingDocument) -> ViewDag {
    let mut edges: BTreeMap<ViewNode, Vec<ViewNode>> = BTreeMap::new();

    // Collect unique target names from mappings.
    let mut target_names: Vec<String> = Vec::new();
    for mapping in &doc.mappings {
        let tname = mapping.target.name().to_string();
        if !target_names.contains(&tname) {
            target_names.push(tname);
        }
    }

    for mapping in &doc.mappings {
        let mname = &mapping.name;
        let tname = mapping.target.name();
        let src = mapping.source.dataset.clone();

        // Source table (external, no deps)
        edges.entry(ViewNode::Source(src.clone())).or_default();

        // Forward view depends on source
        let fwd = ViewNode::Forward(mname.clone());
        edges
            .entry(fwd.clone())
            .or_default()
            .push(ViewNode::Source(src.clone()));

        // Identity view depends on all forward views for this target
        let id = ViewNode::Identity(tname.to_string());
        edges.entry(id.clone()).or_default().push(fwd.clone());

        // Resolved view depends on identity view
        let res = ViewNode::Resolved(tname.to_string());
        edges
            .entry(res.clone())
            .or_default();
        if !edges[&res].contains(&id) {
            edges.get_mut(&res).unwrap().push(id.clone());
        }

        // Reverse view depends on resolved view
        let rev = ViewNode::Reverse(mname.clone());
        edges
            .entry(rev.clone())
            .or_default()
            .push(ViewNode::Resolved(tname.to_string()));

        // Delta view depends on reverse view and original source
        let delta = ViewNode::Delta(mname.clone());
        edges.entry(delta.clone()).or_default().push(rev.clone());
        if !edges[&delta].contains(&ViewNode::Source(src.clone())) {
            edges.get_mut(&delta).unwrap().push(ViewNode::Source(src));
        }
    }

    // Add cross-target dependencies via references.
    for (tname, target) in &doc.targets {
        for (_fname, field) in &target.fields {
            if let Some(ref_target) = field.references() {
                // Resolution of this target depends on the referenced target's identity view
                let res = ViewNode::Resolved(tname.clone());
                let ref_id = ViewNode::Identity(ref_target.to_string());
                if let Some(deps) = edges.get_mut(&res) {
                    if !deps.contains(&ref_id) {
                        deps.push(ref_id);
                    }
                }
            }
        }
    }

    // Topological sort (Kahn's algorithm).
    let order = topological_sort(&edges);

    ViewDag { edges, order }
}

fn topological_sort(edges: &BTreeMap<ViewNode, Vec<ViewNode>>) -> Vec<ViewNode> {
    use std::collections::{HashMap, VecDeque};

    let mut in_degree: HashMap<&ViewNode, usize> = HashMap::new();
    for node in edges.keys() {
        in_degree.entry(node).or_insert(0);
    }
    for deps in edges.values() {
        for dep in deps {
            // dep might not be a key if it's external
            in_degree.entry(dep).or_insert(0);
        }
    }
    // Note: edges map "node depends on deps", so the creation order is:
    // dep must come before node → in graph terms, dep → node

    // Build adjacency: dep → [nodes that depend on dep]
    let mut adj: HashMap<&ViewNode, Vec<&ViewNode>> = HashMap::new();
    let mut in_deg: HashMap<&ViewNode, usize> = HashMap::new();

    for node in edges.keys() {
        in_deg.entry(node).or_insert(0);
    }
    for (node, deps) in edges {
        for dep in deps {
            in_deg.entry(dep).or_insert(0);
            adj.entry(dep).or_default().push(node);
            *in_deg.entry(node).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<&ViewNode> = VecDeque::new();
    for (node, &deg) in &in_deg {
        if deg == 0 {
            queue.push_back(node);
        }
    }

    let mut order = Vec::new();
    while let Some(node) = queue.pop_front() {
        order.push(node.clone());
        if let Some(dependents) = adj.get(node) {
            for dep in dependents {
                let deg = in_deg.get_mut(dep).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(dep);
                }
            }
        }
    }

    order
}

/// Render the DAG as a GraphViz DOT string.
pub fn to_dot(dag: &ViewDag) -> String {
    let mut out = String::from("digraph view_dag {\n  rankdir=TB;\n  node [shape=box];\n\n");

    for (node, deps) in &dag.edges {
        let name = node.view_name();
        let label = node.label();
        let shape = match node {
            ViewNode::Source(_) => "cylinder",
            _ => "box",
        };
        out.push_str(&format!("  \"{name}\" [label=\"{label}\" shape={shape}];\n"));
        for dep in deps {
            out.push_str(&format!(
                "  \"{}\" -> \"{name}\";\n",
                dep.view_name()
            ));
        }
    }

    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn hello_world_dag() {
        let yaml = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("examples/hello-world/mapping.yaml"),
        )
        .unwrap();
        let doc = parser::parse_str(&yaml).unwrap();
        let dag = build_dag(&doc);

        // Should have forward, identity, resolved, reverse, delta for each mapping
        assert!(dag.order.len() > 0);

        // Source tables
        assert!(dag.edges.contains_key(&ViewNode::Source("crm".into())));
        assert!(dag.edges.contains_key(&ViewNode::Source("erp".into())));

        // Forward views
        assert!(dag.edges.contains_key(&ViewNode::Forward("crm".into())));
        assert!(dag.edges.contains_key(&ViewNode::Forward("erp".into())));

        // Identity and resolved for contact
        assert!(dag.edges.contains_key(&ViewNode::Identity("contact".into())));
        assert!(dag.edges.contains_key(&ViewNode::Resolved("contact".into())));
    }
}
