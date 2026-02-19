use crate::plan::schema::Plan;
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet, VecDeque};

/// Topological sort of phases by dependency order (Kahn's algorithm).
/// Returns phase IDs in execution order.
pub fn topo_sort(plan: &Plan) -> Result<Vec<String>> {
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    // Initialize
    for phase in &plan.phases {
        in_degree.entry(&phase.id).or_insert(0);
        dependents.entry(&phase.id).or_default();
    }

    // Build graph
    for phase in &plan.phases {
        for dep in &phase.depends_on {
            *in_degree.entry(&phase.id).or_insert(0) += 1;
            dependents.entry(dep.as_str()).or_default().push(&phase.id);
        }
    }

    // Kahn's algorithm
    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();

    // Sort the initial queue for deterministic output
    let mut sorted_queue: Vec<&str> = queue.drain(..).collect();
    sorted_queue.sort();
    queue.extend(sorted_queue);

    let mut order = Vec::with_capacity(plan.phases.len());

    while let Some(id) = queue.pop_front() {
        order.push(id.to_string());
        if let Some(deps) = dependents.get(id) {
            let mut next = Vec::new();
            for &dep in deps {
                let deg = in_degree.get_mut(dep).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    next.push(dep);
                }
            }
            // Sort for deterministic output
            next.sort();
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
        // b and c can be in either order, but sorted deterministically
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
        // Alphabetically sorted since all have in_degree 0
        assert_eq!(order, vec!["a", "b", "c"]);
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
