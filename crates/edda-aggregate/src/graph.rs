//! Decision dependency graph extraction from provenance references.

use edda_ledger::Ledger;
use edda_store::registry::ProjectEntry;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

/// A node in the dependency graph representing a decision event.
#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub event_id: String,
    pub key: String,
    pub value: String,
    pub project: String,
    pub ts: String,
}

/// An edge in the dependency graph representing a provenance relationship.
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub rel: String,
}

/// The full dependency graph: nodes (decisions) + edges (provenance links).
#[derive(Debug, Clone, Serialize)]
pub struct DependencyGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Build a dependency graph from all decision events across registered projects.
///
/// Nodes are decision events; edges are provenance links between them.
pub fn build_dependency_graph(projects: &[ProjectEntry]) -> DependencyGraph {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut decision_ids: HashSet<String> = HashSet::new();

    for entry in projects {
        let root = Path::new(&entry.path);
        let ledger = match Ledger::open(root) {
            Ok(l) => l,
            Err(_) => continue,
        };

        let events = match ledger.iter_events() {
            Ok(e) => e,
            Err(_) => continue,
        };

        for event in &events {
            if edda_core::decision::is_decision(&event.payload) {
                let dec = edda_core::decision::extract_decision(&event.payload);
                let key = dec.as_ref().map(|d| d.key.clone()).unwrap_or_default();
                let value = dec.as_ref().map(|d| d.value.clone()).unwrap_or_default();

                nodes.push(GraphNode {
                    event_id: event.event_id.clone(),
                    key,
                    value,
                    project: entry.name.clone(),
                    ts: event.ts.clone(),
                });
                decision_ids.insert(event.event_id.clone());
            }

            // Extract provenance edges
            for prov in &event.refs.provenance {
                edges.push(GraphEdge {
                    source: event.event_id.clone(),
                    target: prov.target.clone(),
                    rel: prov.rel.clone(),
                });
            }
        }
    }

    // Filter edges to only include those between known decision nodes
    let edges = edges
        .into_iter()
        .filter(|e| decision_ids.contains(&e.source) || decision_ids.contains(&e.target))
        .collect();

    DependencyGraph { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_decision_payload(
        key: &str,
        value: &str,
        reason: Option<&str>,
    ) -> edda_core::types::DecisionPayload {
        edda_core::types::DecisionPayload {
            key: key.to_string(),
            value: value.to_string(),
            reason: reason.map(|r| r.to_string()),
            scope: None,
        }
    }

    #[test]
    fn empty_projects_returns_empty_graph() {
        let graph = build_dependency_graph(&[]);
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn single_decision_produces_node_no_edges() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let paths = edda_ledger::EddaPaths::discover(root);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();

        let ledger = Ledger::open(root).unwrap();
        let payload = make_decision_payload("db.engine", "sqlite", Some("embedded"));
        let event = edda_core::event::new_decision_event("main", None, "user", &payload).unwrap();
        ledger.append_event(&event).unwrap();

        let entry = ProjectEntry {
            project_id: "test".to_string(),
            path: root.to_string_lossy().to_string(),
            name: "test-project".to_string(),
            registered_at: "2026-03-01T00:00:00Z".to_string(),
            last_seen: "2026-03-01T00:00:00Z".to_string(),
            group: None,
        };

        let graph = build_dependency_graph(&[entry]);
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].key, "db.engine");
        assert_eq!(graph.nodes[0].value, "sqlite");
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn superseded_decision_produces_edge() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let paths = edda_ledger::EddaPaths::discover(root);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();

        let ledger = Ledger::open(root).unwrap();

        // First decision
        let payload1 = make_decision_payload("db.engine", "sqlite", Some("mvp"));
        let event1 = edda_core::event::new_decision_event("main", None, "user", &payload1).unwrap();
        ledger.append_event(&event1).unwrap();
        let event1_id = event1.event_id.clone();

        // Second decision that supersedes the first
        let payload2 = make_decision_payload("db.engine", "postgres", Some("scale"));
        let mut event2 =
            edda_core::event::new_decision_event("main", None, "user", &payload2).unwrap();
        event2.refs.provenance.push(edda_core::types::Provenance {
            target: event1_id,
            rel: edda_core::types::rel::SUPERSEDES.to_string(),
            note: Some("key 'db.engine' re-decided".to_string()),
        });
        edda_core::event::finalize_event(&mut event2).unwrap();
        ledger.append_event(&event2).unwrap();

        let entry = ProjectEntry {
            project_id: "test".to_string(),
            path: root.to_string_lossy().to_string(),
            name: "test-project".to_string(),
            registered_at: "2026-03-01T00:00:00Z".to_string(),
            last_seen: "2026-03-01T00:00:00Z".to_string(),
            group: None,
        };

        let graph = build_dependency_graph(&[entry]);
        assert_eq!(graph.nodes.len(), 2);
        // The superseding decision should create a provenance edge
        assert!(
            !graph.edges.is_empty(),
            "Expected at least one edge from supersession"
        );
    }
}
