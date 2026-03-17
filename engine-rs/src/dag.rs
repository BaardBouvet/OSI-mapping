use std::collections::BTreeMap;

use crate::model::MappingDocument;

/// A node in the view dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ViewNode {
    /// External source table (not created by engine).
    Source(String),
    /// Forward mapping view. Named `_fwd_{mapping}`.
    Forward(String),
    /// Identity/transitive closure view for a target.
    Identity(String),
    /// Resolved golden record view for a target.
    Resolved(String),
    /// Analytics view — clean golden record for BI consumers. Named `{target}`.
    Analytics(String),
    /// Reverse mapping view. Named `_rev_{mapping}`. Opt-in via `sync: true`.
    Reverse(String),
    /// Delta/changeset view per source dataset. Named `_delta_{source}`.
    /// Combines reverse views from all mappings sharing that source.
    Delta(String),
}

impl ViewNode {
    pub fn view_name(&self) -> String {
        match self {
            ViewNode::Source(name) => name.clone(),
            ViewNode::Forward(name) => format!("_fwd_{name}"),
            ViewNode::Identity(name) => format!("_id_{name}"),
            ViewNode::Resolved(name) => format!("_resolved_{name}"),
            ViewNode::Analytics(name) => name.clone(),
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
            ViewNode::Analytics(name) => format!("ANA: {name}"),
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
    /// SQL JOIN edges that are not in the primary dependency chain.
    /// These are transitively satisfied but represent real SQL JOINs.
    pub join_edges: Vec<(ViewNode, ViewNode)>,
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

        if mapping.is_linkage_only() {
            // Linkage-only mapping: only contributes identity edges.
            let id = ViewNode::Identity(tname.to_string());
            edges
                .entry(id)
                .or_default()
                .push(ViewNode::Source(src.clone()));
            continue;
        }

        // Forward view depends on source table.
        let fwd = ViewNode::Forward(mname.clone());
        edges.entry(fwd.clone()).or_default();
        if !edges[&fwd].contains(&ViewNode::Source(src.clone())) {
            edges
                .get_mut(&fwd)
                .unwrap()
                .push(ViewNode::Source(src.clone()));
        }

        // Identity view depends on forward views.
        let id = ViewNode::Identity(tname.to_string());
        edges.entry(id.clone()).or_default();
        if !edges[&id].contains(&fwd) {
            edges.get_mut(&id).unwrap().push(fwd.clone());
        }

        // cluster_members source tables feed the forward view
        if let Some(ref cm) = mapping.cluster_members {
            let cm_table = cm.table_name(mname);
            edges.entry(ViewNode::Source(cm_table.clone())).or_default();
            if !edges[&fwd].contains(&ViewNode::Source(cm_table.clone())) {
                edges
                    .get_mut(&fwd)
                    .unwrap()
                    .push(ViewNode::Source(cm_table));
            }
        }

        // Resolved view depends on identity view
        let res = ViewNode::Resolved(tname.to_string());
        edges.entry(res.clone()).or_default();
        if !edges[&res].contains(&id) {
            edges.get_mut(&res).unwrap().push(id.clone());
        }

        // Analytics view depends on resolved view (one per target).
        // Consumer-facing: named just `{target}`.
        let analytics = ViewNode::Analytics(tname.to_string());
        edges.entry(analytics.clone()).or_default();
        if !edges[&analytics].contains(&res) {
            edges.get_mut(&analytics).unwrap().push(res.clone());
        }

        // Reverse + delta views (auto-derived from field directions).
        if mapping.needs_sync() {
            let rev = ViewNode::Reverse(mname.clone());
            edges
                .entry(rev.clone())
                .or_default()
                .push(ViewNode::Resolved(tname.to_string()));

            // Delta is per-source-dataset (combines all reverse views for this source).
            let delta = ViewNode::Delta(src.clone());
            edges.entry(delta.clone()).or_default();
            if mapping.source.path.is_some() {
                // Nested-path child mappings are LEFT JOINed into the delta
                // (not the driving table). Record as a join edge so the DOT
                // output renders a dotted line instead of a solid dependency.
                if !edges[&delta].contains(&rev) {
                    edges.get_mut(&delta).unwrap().push(rev.clone());
                }
                // We still need the edge for topological ordering, but we also
                // record it as a join_edge so to_dot renders it dotted.
                // The actual marking happens below in the join_edges section.
            } else if !edges[&delta].contains(&rev) {
                edges.get_mut(&delta).unwrap().push(rev);
            }
        }
    }

    // Add cross-target dependencies via references.
    for (tname, target) in &doc.targets {
        for (_fname, field) in &target.fields {
            if let Some(ref_target) = field.references() {
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

    // Collect SQL JOIN edges that are not primary dependencies.
    // Reverse views LEFT JOIN identity (diamond for IVM, safe for ordered refresh).
    let mut join_edges: Vec<(ViewNode, ViewNode)> = Vec::new();
    for mapping in &doc.mappings {
        if mapping.is_linkage_only() || !mapping.needs_sync() {
            continue;
        }
        let tname = mapping.target.name();
        join_edges.push((
            ViewNode::Identity(tname.to_string()),
            ViewNode::Reverse(mapping.name.clone()),
        ));
        // Nested-path child reverse views are LEFT JOINed into the delta
        // (the parent mapping drives it). Mark as join edge for dotted rendering.
        if mapping.source.path.is_some() {
            join_edges.push((
                ViewNode::Reverse(mapping.name.clone()),
                ViewNode::Delta(mapping.source.dataset.clone()),
            ));
        }
    }

    // Topological sort (Kahn's algorithm).
    let order = topological_sort(&edges);

    ViewDag {
        edges,
        order,
        join_edges,
    }
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

    // Collect join edge pairs so we can render them dotted instead of solid.
    let join_set: std::collections::HashSet<(String, String)> = dag
        .join_edges
        .iter()
        .map(|(from, to)| (from.view_name(), to.view_name()))
        .collect();

    for (node, deps) in &dag.edges {
        let name = node.view_name();
        let label = node.label();
        let shape = match node {
            ViewNode::Source(_) => "cylinder",
            ViewNode::Forward(_) => "box",
            ViewNode::Analytics(_) => "note",
            ViewNode::Reverse(_) | ViewNode::Delta(_) => "box",
            _ => "box",
        };
        out.push_str(&format!(
            "  \"{name}\" [label=\"{label}\" shape={shape}];\n"
        ));
        for dep in deps {
            // Skip solid edge if a join edge exists for the same pair
            // (will be rendered dotted below).
            let dep_name = dep.view_name();
            if join_set.contains(&(dep_name.clone(), name.clone())) {
                continue;
            }
            out.push_str(&format!("  \"{dep_name}\" -> \"{name}\";\n",));
        }
    }

    // Render SQL JOIN edges as dotted lines.
    for (from, to) in &dag.join_edges {
        out.push_str(&format!(
            "  \"{}\" -> \"{}\" [style=dotted label=\"JOIN\"];\n",
            from.view_name(),
            to.view_name()
        ));
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

        // Should have source, forward, identity, resolved, analytics, sync nodes
        assert!(!dag.order.is_empty());

        // Source tables
        assert!(dag.edges.contains_key(&ViewNode::Source("crm".into())));
        assert!(dag.edges.contains_key(&ViewNode::Source("erp".into())));

        // Forward views
        assert!(dag.edges.contains_key(&ViewNode::Forward("crm".into())));
        assert!(dag.edges.contains_key(&ViewNode::Forward("erp".into())));

        // Identity and resolved for contact
        assert!(dag
            .edges
            .contains_key(&ViewNode::Identity("contact".into())));
        assert!(dag
            .edges
            .contains_key(&ViewNode::Resolved("contact".into())));

        // Analytics view for contact
        assert!(dag
            .edges
            .contains_key(&ViewNode::Analytics("contact".into())));

        // Reverse + delta views (hello-world has sync: true)
        assert!(dag.edges.contains_key(&ViewNode::Reverse("crm".into())));
        assert!(dag.edges.contains_key(&ViewNode::Reverse("erp".into())));
        assert!(dag.edges.contains_key(&ViewNode::Delta("crm".into())));
        assert!(dag.edges.contains_key(&ViewNode::Delta("erp".into())));
    }
}
