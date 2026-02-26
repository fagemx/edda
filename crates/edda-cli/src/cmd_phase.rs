use edda_bridge_claude::agent_phase;
use edda_core::agent_phase::{phase_suggestion, AgentPhaseMap};
use std::path::Path;

/// Execute `edda phase` — show current agent phase map.
pub fn execute(repo_root: &Path, json: bool) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let session_id = infer_session_id(&project_id);

    let map = agent_phase::build_phase_map(&project_id, session_id.as_deref().unwrap_or(""));

    if json {
        println!("{}", serde_json::to_string_pretty(&map)?);
        return Ok(());
    }

    print_phase_map(&map, session_id.as_deref());
    Ok(())
}

/// Print human-readable phase map.
fn print_phase_map(map: &AgentPhaseMap, current_session: Option<&str>) {
    if map.agents.is_empty() && map.stale.is_empty() {
        println!("No agent phase data found.");
        println!("Phase detection runs automatically during Claude Code hook dispatch.");
        return;
    }

    println!("Agent Phase Map");
    println!("===============");
    println!();

    if !map.agents.is_empty() {
        println!("Active ({}):", map.agents.len());
        for state in &map.agents {
            let is_me = current_session
                .map(|s| s == state.session_id)
                .unwrap_or(false);
            let marker = if is_me { " (you)" } else { "" };
            let id = state.label.as_deref().unwrap_or(&state.session_id);
            let context = match (state.issue, state.pr) {
                (_, Some(pr)) => format!(" PR #{pr}"),
                (Some(issue), _) => format!(" #{issue}"),
                _ => String::new(),
            };
            let suggestion = phase_suggestion(&state.phase, state.issue, state.pr);
            println!(
                "  {} {}{context}{marker}  (confidence: {:.0}%)  suggested: {suggestion}",
                id,
                state.phase,
                state.confidence * 100.0
            );
            if !state.signals.is_empty() {
                for signal in &state.signals {
                    println!("    - {signal}");
                }
            }
        }
    }

    if !map.stale.is_empty() {
        println!();
        println!("Stale ({}):", map.stale.len());
        for state in &map.stale {
            let id = state.label.as_deref().unwrap_or(&state.session_id);
            println!(
                "  {} {} (stale since {})",
                id, state.phase, state.detected_at
            );
        }
    }

    println!();
    println!("Summary: {}", map.summary);
}

/// Infer session ID from active heartbeats (same as other bridge commands).
fn infer_session_id(project_id: &str) -> Option<String> {
    let peers = edda_bridge_claude::peers::discover_all_sessions(project_id);
    // If only one session, that's us
    if peers.len() == 1 {
        return Some(peers[0].session_id.clone());
    }
    // Otherwise, check env var
    std::env::var("EDDA_SESSION_ID")
        .ok()
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::agent_phase::{AgentPhase, AgentPhaseMap, AgentPhaseState};

    #[test]
    fn print_phase_map_empty() {
        let map = AgentPhaseMap::from_agents(vec![], vec![]);
        // Should not panic
        print_phase_map(&map, None);
    }

    #[test]
    fn print_phase_map_with_agents() {
        let state = AgentPhaseState {
            phase: AgentPhase::Implement,
            session_id: "sess-1".to_string(),
            label: Some("auth-worker".to_string()),
            issue: Some(45),
            pr: None,
            branch: Some("feat/auth-45".to_string()),
            confidence: 0.85,
            detected_at: "2026-02-27T10:00:00Z".to_string(),
            signals: vec!["branch feat/auth-45 created".to_string()],
        };
        let map = AgentPhaseMap::from_agents(vec![state], vec![]);
        // Should not panic
        print_phase_map(&map, Some("sess-1"));
    }
}
