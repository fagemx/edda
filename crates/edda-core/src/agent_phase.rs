use serde::{Deserialize, Serialize};
use std::fmt;

/// The lifecycle phase an agent is currently in.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Triage,
    Research,
    Plan,
    Implement,
    Review,
}

impl fmt::Display for AgentPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Triage => write!(f, "triage"),
            Self::Research => write!(f, "research"),
            Self::Plan => write!(f, "plan"),
            Self::Implement => write!(f, "implement"),
            Self::Review => write!(f, "review"),
        }
    }
}

impl std::str::FromStr for AgentPhase {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "triage" => Ok(Self::Triage),
            "research" => Ok(Self::Research),
            "plan" => Ok(Self::Plan),
            "implement" => Ok(Self::Implement),
            "review" => Ok(Self::Review),
            other => Err(format!("unknown agent phase: {other}")),
        }
    }
}

/// Snapshot of a single agent's detected phase.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentPhaseState {
    pub phase: AgentPhase,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub confidence: f32,
    pub detected_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<String>,
}

/// A phase transition (from -> to) with the new state.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentPhaseTransition {
    pub from: AgentPhase,
    pub to: AgentPhase,
    pub state: AgentPhaseState,
}

/// Aggregated view of all active agents' phases.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentPhaseMap {
    pub agents: Vec<AgentPhaseState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<AgentPhaseState>,
    pub summary: String,
}

impl AgentPhaseMap {
    /// Build an AgentPhaseMap from active and stale agent states.
    pub fn from_agents(agents: Vec<AgentPhaseState>, stale: Vec<AgentPhaseState>) -> Self {
        let summary = Self::build_summary(&agents);
        Self {
            agents,
            stale,
            summary,
        }
    }

    fn build_summary(agents: &[AgentPhaseState]) -> String {
        if agents.is_empty() {
            return "no active agents".to_string();
        }
        let parts: Vec<String> = agents
            .iter()
            .map(|a| {
                let id = a.label.as_deref().unwrap_or(&a.session_id);
                let context = match (a.issue, a.pr) {
                    (_, Some(pr)) => format!(" PR #{pr}"),
                    (Some(issue), _) => format!(" #{issue}"),
                    _ => String::new(),
                };
                format!("{id} {}{context}", a.phase)
            })
            .collect();
        format!("{} active: {}", agents.len(), parts.join(", "))
    }
}

/// Suggestion for the agent based on current phase.
pub fn phase_suggestion(phase: &AgentPhase, issue: Option<u64>, pr: Option<u64>) -> String {
    match phase {
        AgentPhase::Triage => "/issue-scan or /issue-create".to_string(),
        AgentPhase::Research => match issue {
            Some(id) => format!("/deep-research {id}"),
            None => "/deep-research".to_string(),
        },
        AgentPhase::Plan => match issue {
            Some(id) => format!("/deep-plan {id}"),
            None => "/deep-plan".to_string(),
        },
        AgentPhase::Implement => "/issue-action".to_string(),
        AgentPhase::Review => match pr {
            Some(id) => format!("/pr-review {id}"),
            None => "/pr-review".to_string(),
        },
    }
}

/// Format a phase nudge line for hook injection.
pub fn format_phase_nudge(state: &AgentPhaseState) -> String {
    let context = match (state.issue, state.pr) {
        (_, Some(pr)) => format!(" (PR #{pr})"),
        (Some(issue), _) => format!(" (#{issue})"),
        _ => String::new(),
    };
    let suggestion = phase_suggestion(&state.phase, state.issue, state.pr);
    format!(
        "-> AgentPhase: {}{context}. Suggested: {suggestion}",
        state.phase
    )
}

/// Generate a mobile-friendly context summary within a character budget.
pub fn mobile_context_summary(
    state: &AgentPhaseState,
    decisions: &[String],
    commits: &[String],
    budget_chars: usize,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Phase header (always included)
    let header = match (state.issue, state.pr) {
        (_, Some(pr)) => format!("{} PR #{pr}", state.phase),
        (Some(issue), _) => format!("{} #{issue}", state.phase),
        _ => state.phase.to_string(),
    };
    parts.push(header);

    // Phase-specific content priority
    match state.phase {
        AgentPhase::Research | AgentPhase::Triage => {
            for d in decisions.iter().take(2) {
                parts.push(d.clone());
            }
        }
        AgentPhase::Plan => {
            for d in decisions.iter().take(1) {
                parts.push(d.clone());
            }
            for c in commits.iter().take(1) {
                parts.push(c.clone());
            }
        }
        AgentPhase::Implement => {
            for c in commits.iter().take(2) {
                parts.push(c.clone());
            }
            for d in decisions.iter().take(1) {
                parts.push(d.clone());
            }
        }
        AgentPhase::Review => {
            for c in commits.iter().take(2) {
                parts.push(c.clone());
            }
        }
    }

    let joined = parts.join("; ");
    if joined.len() <= budget_chars {
        joined
    } else {
        let mut truncated = joined[..budget_chars.saturating_sub(3)].to_string();
        truncated.push_str("...");
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> AgentPhaseState {
        AgentPhaseState {
            phase: AgentPhase::Implement,
            session_id: "sess-abc".to_string(),
            label: Some("feature-worker".to_string()),
            issue: Some(45),
            pr: None,
            branch: Some("feat/auth-45".to_string()),
            confidence: 0.85,
            detected_at: "2026-02-27T10:00:00Z".to_string(),
            signals: vec!["branch feat/auth-45 created".to_string()],
        }
    }

    #[test]
    fn agent_phase_display_roundtrip() {
        for phase in [
            AgentPhase::Triage,
            AgentPhase::Research,
            AgentPhase::Plan,
            AgentPhase::Implement,
            AgentPhase::Review,
        ] {
            let s = phase.to_string();
            let parsed: AgentPhase = s.parse().unwrap();
            assert_eq!(phase, parsed);
        }
    }

    #[test]
    fn agent_phase_from_str_unknown() {
        let result: Result<AgentPhase, _> = "unknown".parse();
        assert!(result.is_err());
    }

    #[test]
    fn agent_phase_state_serde_roundtrip() {
        let state = sample_state();
        let json = serde_json::to_string(&state).unwrap();
        let parsed: AgentPhaseState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.phase, AgentPhase::Implement);
        assert_eq!(parsed.session_id, "sess-abc");
        assert_eq!(parsed.label.as_deref(), Some("feature-worker"));
        assert_eq!(parsed.issue, Some(45));
        assert_eq!(parsed.confidence, 0.85);
    }

    #[test]
    fn agent_phase_state_serde_minimal() {
        let json = r#"{"phase":"triage","session_id":"s1","confidence":0.3,"detected_at":"2026-01-01T00:00:00Z"}"#;
        let state: AgentPhaseState = serde_json::from_str(json).unwrap();
        assert_eq!(state.phase, AgentPhase::Triage);
        assert!(state.label.is_none());
        assert!(state.issue.is_none());
        assert!(state.signals.is_empty());
    }

    #[test]
    fn agent_phase_transition_serde_roundtrip() {
        let t = AgentPhaseTransition {
            from: AgentPhase::Research,
            to: AgentPhase::Implement,
            state: sample_state(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let parsed: AgentPhaseTransition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.from, AgentPhase::Research);
        assert_eq!(parsed.to, AgentPhase::Implement);
    }

    #[test]
    fn agent_phase_map_serde_roundtrip() {
        let map = AgentPhaseMap::from_agents(vec![sample_state()], vec![]);
        let json = serde_json::to_string(&map).unwrap();
        let parsed: AgentPhaseMap = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agents.len(), 1);
        assert!(parsed.stale.is_empty());
        assert!(parsed.summary.contains("1 active"));
    }

    #[test]
    fn agent_phase_map_summary_empty() {
        let map = AgentPhaseMap::from_agents(vec![], vec![]);
        assert_eq!(map.summary, "no active agents");
    }

    #[test]
    fn agent_phase_map_summary_multiple() {
        let s1 = sample_state();
        let mut s2 = sample_state();
        s2.session_id = "sess-def".to_string();
        s2.label = None;
        s2.phase = AgentPhase::Review;
        s2.pr = Some(53);

        let map = AgentPhaseMap::from_agents(vec![s1, s2], vec![]);
        assert!(map.summary.contains("2 active"));
        assert!(map.summary.contains("feature-worker implement #45"));
        assert!(map.summary.contains("sess-def review PR #53"));
    }

    #[test]
    fn phase_suggestion_with_issue() {
        let s = phase_suggestion(&AgentPhase::Research, Some(45), None);
        assert_eq!(s, "/deep-research 45");
    }

    #[test]
    fn phase_suggestion_with_pr() {
        let s = phase_suggestion(&AgentPhase::Review, None, Some(53));
        assert_eq!(s, "/pr-review 53");
    }

    #[test]
    fn format_phase_nudge_output() {
        let state = sample_state();
        let nudge = format_phase_nudge(&state);
        assert!(nudge.starts_with("-> AgentPhase: implement (#45)"));
        assert!(nudge.contains("/issue-action"));
    }

    #[test]
    fn mobile_context_summary_within_budget() {
        let state = sample_state();
        let decisions = vec!["db.engine=sqlite".to_string()];
        let commits = vec!["feat: add auth".to_string()];
        let summary = mobile_context_summary(&state, &decisions, &commits, 200);
        assert!(summary.len() <= 200);
        assert!(summary.contains("implement #45"));
    }

    #[test]
    fn mobile_context_summary_truncates() {
        let state = sample_state();
        let decisions = vec!["a very long decision description that takes up lots of space".to_string()];
        let commits = vec!["a very long commit message that also takes space".to_string()];
        let summary = mobile_context_summary(&state, &decisions, &commits, 50);
        assert!(summary.len() <= 50);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn agent_phase_serde_snake_case() {
        let json = serde_json::to_string(&AgentPhase::Implement).unwrap();
        assert_eq!(json, "\"implement\"");
        let parsed: AgentPhase = serde_json::from_str("\"research\"").unwrap();
        assert_eq!(parsed, AgentPhase::Research);
    }
}
