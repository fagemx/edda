use crate::plan::schema::Plan;
use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet, VecDeque};

/// Topological sort of phases by dependency order (Kahn's algorithm).
/// Returns phase IDs in execution order.
///
/// Ties (phases whose dependencies are all satisfied at the same time) are
/// broken by declaration order in the plan file, not alphabetically — the
/// YAML sequence is what the author reads top-to-bottom (GH-532).
pub fn topo_sort(plan: &Plan) -> Result<Vec<String>> {
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    // Declaration position of each phase id — the deterministic tie-break.
    let mut position: HashMap<&str, usize> = HashMap::new();

    // Initialize
    for (idx, phase) in plan.phases.iter().enumerate() {
        in_degree.entry(&phase.id).or_insert(0);
        dependents.entry(&phase.id).or_default();
        position.entry(&phase.id).or_insert(idx);
    }

    // Build graph
    for phase in &plan.phases {
        for dep in &phase.depends_on {
            *in_degree.entry(&phase.id).or_insert(0) += 1;
            dependents.entry(dep.as_str()).or_default().push(&phase.id);
        }
    }

    // Kahn's algorithm; the queue is kept in declaration order so that
    // equal-rank phases execute in the order they were declared (GH-532).
    let mut queue: VecDeque<&str> = plan
        .phases
        .iter()
        .filter(|p| in_degree.get(p.id.as_str()) == Some(&0))
        .map(|p| p.id.as_str())
        .collect();

    let mut order = Vec::with_capacity(plan.phases.len());

    while let Some(id) = queue.pop_front() {
        order.push(id.to_string());
        if let Some(deps) = dependents.get(id) {
            let mut next = Vec::new();
            for &dep in deps {
                let deg = in_degree
                    .get_mut(dep)
                    .context("dependent phase not found in in-degree map")?;
                *deg -= 1;
                if *deg == 0 {
                    next.push(dep);
                }
            }
            // Re-sort by declaration position for deterministic output
            next.sort_by_key(|id| position.get(id).copied().unwrap_or(usize::MAX));
            queue.extend(next);
        }
    }

    if order.len() != plan.phases.len() {
        // Find cycle participants
        let in_order: HashSet<&str> = order.iter().map(|s| s.as_str()).collect();
        let cycle_members: Vec<&str> = plan
            .phases
            .iter()
            .map(|p| p.id.as_str())
            .filter(|id| !in_order.contains(id))
            .collect();
        bail!(
            "dependency cycle detected among phases: [{}]",
            cycle_members.join(", ")
        );
    }

    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parser::parse_plan;

    #[test]
    fn linear_chain() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
  - id: b
    prompt: "x"
    depends_on: [a]
  - id: c
    prompt: "x"
    depends_on: [b]
"#;
        let plan = parse_plan(yaml).unwrap();
        let order = topo_sort(&plan).unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn diamond_dependency() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
  - id: b
    prompt: "x"
    depends_on: [a]
  - id: c
    prompt: "x"
    depends_on: [a]
  - id: d
    prompt: "x"
    depends_on: [b, c]
"#;
        let plan = parse_plan(yaml).unwrap();
        let order = topo_sort(&plan).unwrap();
        assert_eq!(order[0], "a");
        assert_eq!(order[3], "d");
        // b and c are tied after a; they run in declaration order (b first)
        assert_eq!(order[1], "b");
        assert_eq!(order[2], "c");
    }

    #[test]
    fn no_dependencies() {
        let yaml = r#"
name: test
phases:
  - id: c
    prompt: "x"
  - id: a
    prompt: "x"
  - id: b
    prompt: "x"
"#;
        let plan = parse_plan(yaml).unwrap();
        let order = topo_sort(&plan).unwrap();
        // Declaration order preserved when all have in_degree 0
        assert_eq!(order, vec!["c", "a", "b"]);
    }

    #[test]
    fn declaration_order_tie_break_regression_gh532() {
        // GH-532: a real plan declared phases in the order adapt, wire, freeze
        // (no depends_on). Alphabetical tie-breaking executed freeze before
        // wire — the commit/receipt phase sealed an unfinished tree.
        let yaml = r#"
name: gh532-regression
phases:
  - id: adapt
    prompt: "x"
  - id: wire
    prompt: "x"
  - id: freeze
    prompt: "x"
"#;
        let plan = parse_plan(yaml).unwrap();
        let order = topo_sort(&plan).unwrap();
        assert_eq!(order, vec!["adapt", "wire", "freeze"]);
    }

    #[test]
    fn declaration_order_tie_break_among_siblings() {
        // Siblings freed at the same time run in declaration order, even when
        // their ids sort the other way.
        let yaml = r#"
name: test
phases:
  - id: root
    prompt: "x"
  - id: zeta
    prompt: "x"
    depends_on: [root]
  - id: alpha
    prompt: "x"
    depends_on: [root]
"#;
        let plan = parse_plan(yaml).unwrap();
        let order = topo_sort(&plan).unwrap();
        assert_eq!(order, vec!["root", "zeta", "alpha"]);
    }

    #[test]
    fn cycle_detected() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
    depends_on: [b]
  - id: b
    prompt: "x"
    depends_on: [a]
"#;
        let plan = parse_plan(yaml).unwrap();
        let err = topo_sort(&plan).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn three_node_cycle() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
    depends_on: [c]
  - id: b
    prompt: "x"
    depends_on: [a]
  - id: c
    prompt: "x"
    depends_on: [b]
"#;
        let plan = parse_plan(yaml).unwrap();
        let err = topo_sort(&plan).unwrap_err();
        assert!(err.to_string().contains("cycle"));
        assert!(err.to_string().contains("a"));
    }

    #[test]
    fn single_phase() {
        let yaml = r#"
name: test
phases:
  - id: only
    prompt: "x"
"#;
        let plan = parse_plan(yaml).unwrap();
        let order = topo_sort(&plan).unwrap();
        assert_eq!(order, vec!["only"]);
    }
}
