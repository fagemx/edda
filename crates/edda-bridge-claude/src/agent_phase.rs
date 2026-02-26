//! Agent phase detection engine.
//!
//! Detects what lifecycle phase an agent is in (Triage/Research/Plan/Implement/Review)
//! based on git state, artifact existence, and task heuristics. Persists phase state
//! to disk and detects transitions with debounce.

use edda_core::agent_phase::{AgentPhase, AgentPhaseMap, AgentPhaseState, AgentPhaseTransition};
use std::path::Path;

/// Configuration for phase detection debounce.
pub struct DetectorConfig {
    pub confidence_threshold: f32,
    pub min_interval_secs: u64,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.6,
            min_interval_secs: 30,
        }
    }
}

// ── State Persistence ──

/// Read last persisted phase state for a session.
pub fn read_phase_state(project_id: &str, session_id: &str) -> Option<AgentPhaseState> {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join(format!("phase.{session_id}.json"));
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Write phase state to disk.
pub fn write_phase_state(project_id: &str, state: &AgentPhaseState) -> Result<(), std::io::Error> {
    let dir = edda_store::project_dir(project_id).join("state");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("phase.{}.json", state.session_id));
    let json = serde_json::to_string(state).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(&path, json)
}

// ── Detection Engine ──

/// Detect current phase from available signals.
///
/// `deep_dive_base` overrides the default deep-dive search directory.
/// Pass `None` to use the system default (`/tmp/deep-dive` or `%TEMP%/deep-dive`).
pub fn detect_current_phase(
    session_id: &str,
    label: Option<&str>,
    branch: Option<&str>,
    active_tasks: &[String],
    cwd: &Path,
    deep_dive_base: Option<&Path>,
) -> AgentPhaseState {
    let mut signals = Vec::new();
    let mut phase = AgentPhase::Triage;
    let mut confidence: f32 = 0.3;
    let mut issue: Option<u64> = None;
    let mut pr: Option<u64> = None;

    // Extract issue number from branch name (e.g., feat/auth-45 -> 45)
    if let Some(b) = branch {
        if let Some(num) = extract_issue_from_branch(b) {
            issue = Some(num);
        }
    }

    // 1. Branch-based detection
    if let Some(b) = branch {
        if b.starts_with("feat/") || b.starts_with("fix/") || b.starts_with("issue-") {
            phase = AgentPhase::Implement;
            confidence = 0.5;
            signals.push(format!("branch {b} is a feature branch"));
        }
    }

    // 2. Artifact-based detection (overrides branch if more specific)
    let default_base = default_deep_dive_dir();
    let base = deep_dive_base.unwrap_or(&default_base);
    let artifacts = scan_artifacts(cwd, base);
    if artifacts.has_plan {
        // plan.md exists -> we're past planning
        if phase == AgentPhase::Implement || phase == AgentPhase::Triage {
            phase = AgentPhase::Implement;
            confidence = confidence.max(0.7);
            signals.push("plan.md artifact found".to_string());
        }
    } else if artifacts.has_research {
        // research.md but no plan.md -> in planning phase
        phase = AgentPhase::Plan;
        confidence = confidence.max(0.7);
        signals.push("research.md found, no plan.md".to_string());
    } else if issue.is_some() && !artifacts.has_research {
        // Issue exists but no research artifacts -> research phase
        if branch.is_none()
            || branch
                .map(|b| b == "main" || b == "master")
                .unwrap_or(false)
        {
            phase = AgentPhase::Research;
            confidence = confidence.max(0.6);
            signals.push("issue context exists, no research artifacts".to_string());
        }
    }

    // 3. Task name heuristics (boost confidence if aligned)
    let task_phase = detect_phase_from_tasks(active_tasks);
    if let Some(tp) = task_phase {
        if tp == phase {
            confidence = (confidence + 0.15).min(1.0);
            signals.push("task names align with detected phase".to_string());
        } else if confidence < 0.6 {
            // Tasks override low-confidence detection
            phase = tp;
            confidence = 0.55;
            signals.push("phase inferred from task names".to_string());
        }
    }

    // 4. PR detection (from cached state or task names)
    if let Some(pr_num) = detect_pr_from_tasks(active_tasks) {
        pr = Some(pr_num);
        phase = AgentPhase::Review;
        confidence = confidence.max(0.8);
        signals.push(format!("PR #{pr_num} detected in tasks"));
    }

    AgentPhaseState {
        phase,
        session_id: session_id.to_string(),
        label: label.map(|s| s.to_string()),
        issue,
        pr,
        branch: branch.map(|s| s.to_string()),
        confidence,
        detected_at: now_rfc3339(),
        signals,
    }
}

/// Compare current vs previous state, apply debounce, return transition if warranted.
pub fn detect_transition(
    current: &AgentPhaseState,
    previous: Option<&AgentPhaseState>,
    config: &DetectorConfig,
) -> Option<AgentPhaseTransition> {
    let prev = previous?;

    // Same phase -> no transition
    if current.phase == prev.phase {
        return None;
    }

    // Confidence below threshold -> skip
    if current.confidence < config.confidence_threshold {
        return None;
    }

    // Min interval check
    if let (Some(curr_epoch), Some(prev_epoch)) = (
        parse_rfc3339_to_epoch(&current.detected_at),
        parse_rfc3339_to_epoch(&prev.detected_at),
    ) {
        if curr_epoch.saturating_sub(prev_epoch) < config.min_interval_secs {
            return None;
        }
    }

    Some(AgentPhaseTransition {
        from: prev.phase.clone(),
        to: current.phase.clone(),
        state: current.clone(),
    })
}

/// Build AgentPhaseMap from all active sessions.
pub fn build_phase_map(project_id: &str, _current_session_id: &str) -> AgentPhaseMap {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let stale_threshold = crate::peers::stale_secs();
    let now_epoch = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);

    let mut active = Vec::new();
    let mut stale = Vec::new();

    let entries = match std::fs::read_dir(&state_dir) {
        Ok(entries) => entries,
        Err(_) => return AgentPhaseMap::from_agents(vec![], vec![]),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Match phase.{sid}.json files
        if !name_str.starts_with("phase.") || !name_str.ends_with(".json") {
            continue;
        }

        let content = match std::fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let state: AgentPhaseState = match serde_json::from_str(&content) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Check staleness via heartbeat
        let heartbeat_age = heartbeat_age_secs(project_id, &state.session_id, now_epoch);
        if heartbeat_age > stale_threshold {
            stale.push(state);
        } else {
            active.push(state);
        }
    }

    AgentPhaseMap::from_agents(active, stale)
}

// ── Internal Helpers ──

struct ArtifactScan {
    has_research: bool,
    has_plan: bool,
}

fn default_deep_dive_dir() -> std::path::PathBuf {
    if cfg!(windows) {
        std::env::temp_dir().join("deep-dive")
    } else {
        std::path::PathBuf::from("/tmp/deep-dive")
    }
}

fn scan_artifacts(cwd: &Path, deep_dive_dir: &Path) -> ArtifactScan {
    let mut has_research = false;
    let mut has_plan = false;

    if let Ok(entries) = std::fs::read_dir(deep_dive_dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if entry.path().join("research.md").exists() {
                    has_research = true;
                }
                if entry.path().join("plan.md").exists() {
                    has_plan = true;
                }
            }
        }
    }

    // Also check cwd-relative paths
    if cwd.join("research.md").exists() {
        has_research = true;
    }
    if cwd.join("plan.md").exists() {
        has_plan = true;
    }

    ArtifactScan {
        has_research,
        has_plan,
    }
}

fn extract_issue_from_branch(branch: &str) -> Option<u64> {
    // Match patterns: feat/foo-123, fix/bar-456, issue-789
    branch
        .rsplit('-')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
}

fn detect_phase_from_tasks(tasks: &[String]) -> Option<AgentPhase> {
    let joined = tasks.join(" ").to_lowercase();

    if joined.contains("review") || joined.contains("pr-review") || joined.contains("pr review") {
        return Some(AgentPhase::Review);
    }
    if joined.contains("implement")
        || joined.contains("issue-action")
        || joined.contains("coding")
        || joined.contains("fix ")
        || joined.contains("add ")
    {
        return Some(AgentPhase::Implement);
    }
    if joined.contains("plan") || joined.contains("deep-plan") || joined.contains("design") {
        return Some(AgentPhase::Plan);
    }
    if joined.contains("research")
        || joined.contains("deep-research")
        || joined.contains("investigate")
    {
        return Some(AgentPhase::Research);
    }
    if joined.contains("triage") || joined.contains("issue-scan") || joined.contains("scan") {
        return Some(AgentPhase::Triage);
    }

    None
}

fn detect_pr_from_tasks(tasks: &[String]) -> Option<u64> {
    for task in tasks {
        let lower = task.to_lowercase();
        if lower.contains("pr #") || lower.contains("pr-review") || lower.contains("pr review") {
            // Try to extract PR number: "PR #123" or "review PR #123"
            for word in task.split_whitespace() {
                if let Some(stripped) = word.strip_prefix('#') {
                    if let Ok(num) = stripped.parse::<u64>() {
                        return Some(num);
                    }
                }
            }
        }
    }
    None
}

fn heartbeat_age_secs(project_id: &str, session_id: &str, now_epoch: u64) -> u64 {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join(format!("session.{session_id}.json"));
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return u64::MAX, // No heartbeat -> stale
    };
    let hb: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return u64::MAX,
    };
    let ts = hb["last_heartbeat"].as_str().unwrap_or_default();
    match parse_rfc3339_to_epoch(ts) {
        Some(epoch) => now_epoch.saturating_sub(epoch),
        None => u64::MAX,
    }
}

fn parse_rfc3339_to_epoch(ts: &str) -> Option<u64> {
    time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
        .ok()
        .and_then(|dt| u64::try_from(dt.unix_timestamp()).ok())
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_issue_from_branch_feat() {
        assert_eq!(extract_issue_from_branch("feat/auth-45"), Some(45));
        assert_eq!(extract_issue_from_branch("fix/bug-123"), Some(123));
        assert_eq!(extract_issue_from_branch("issue-789"), Some(789));
    }

    #[test]
    fn extract_issue_from_branch_no_number() {
        assert_eq!(extract_issue_from_branch("main"), None);
        assert_eq!(extract_issue_from_branch("feat/no-number-here"), None);
    }

    #[test]
    fn detect_phase_from_tasks_research() {
        let tasks = vec!["Execute research phase".to_string()];
        assert_eq!(detect_phase_from_tasks(&tasks), Some(AgentPhase::Research));
    }

    #[test]
    fn detect_phase_from_tasks_implement() {
        let tasks = vec!["Implement auth feature".to_string()];
        assert_eq!(detect_phase_from_tasks(&tasks), Some(AgentPhase::Implement));
    }

    #[test]
    fn detect_phase_from_tasks_review() {
        let tasks = vec!["Run pr-review".to_string()];
        assert_eq!(detect_phase_from_tasks(&tasks), Some(AgentPhase::Review));
    }

    #[test]
    fn detect_phase_from_tasks_empty() {
        assert_eq!(detect_phase_from_tasks(&[]), None);
    }

    #[test]
    fn detect_pr_from_tasks_found() {
        let tasks = vec!["Review PR #53".to_string()];
        assert_eq!(detect_pr_from_tasks(&tasks), Some(53));
    }

    #[test]
    fn detect_pr_from_tasks_not_found() {
        let tasks = vec!["Implement feature".to_string()];
        assert_eq!(detect_pr_from_tasks(&tasks), None);
    }

    #[test]
    fn detect_current_phase_default_is_triage() {
        let tmp = std::env::temp_dir().join("edda_phase_test_default");
        let _ = std::fs::create_dir_all(&tmp);
        let no_artifacts = tmp.join("empty_deep_dive");
        let state = detect_current_phase("sess-1", None, None, &[], &tmp, Some(&no_artifacts));
        assert_eq!(state.phase, AgentPhase::Triage);
        assert!(state.confidence <= 0.5);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_current_phase_feature_branch() {
        let tmp = std::env::temp_dir().join("edda_phase_test_feat");
        let _ = std::fs::create_dir_all(&tmp);
        let no_artifacts = tmp.join("empty_deep_dive");
        let state = detect_current_phase(
            "sess-1",
            None,
            Some("feat/auth-45"),
            &[],
            &tmp,
            Some(&no_artifacts),
        );
        assert_eq!(state.phase, AgentPhase::Implement);
        assert!(state.confidence >= 0.5);
        assert_eq!(state.issue, Some(45));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_transition_same_phase_returns_none() {
        let s1 = AgentPhaseState {
            phase: AgentPhase::Implement,
            session_id: "s1".to_string(),
            label: None,
            issue: None,
            pr: None,
            branch: None,
            confidence: 0.8,
            detected_at: "2026-02-27T10:00:00Z".to_string(),
            signals: vec![],
        };
        let s2 = AgentPhaseState {
            phase: AgentPhase::Implement,
            detected_at: "2026-02-27T10:01:00Z".to_string(),
            ..s1.clone()
        };
        let config = DetectorConfig::default();
        assert!(detect_transition(&s2, Some(&s1), &config).is_none());
    }

    #[test]
    fn detect_transition_low_confidence_returns_none() {
        let prev = AgentPhaseState {
            phase: AgentPhase::Research,
            session_id: "s1".to_string(),
            label: None,
            issue: None,
            pr: None,
            branch: None,
            confidence: 0.8,
            detected_at: "2026-02-27T10:00:00Z".to_string(),
            signals: vec![],
        };
        let curr = AgentPhaseState {
            phase: AgentPhase::Plan,
            confidence: 0.4, // below threshold
            detected_at: "2026-02-27T10:01:00Z".to_string(),
            ..prev.clone()
        };
        let config = DetectorConfig::default();
        assert!(detect_transition(&curr, Some(&prev), &config).is_none());
    }

    #[test]
    fn detect_transition_too_soon_returns_none() {
        let prev = AgentPhaseState {
            phase: AgentPhase::Research,
            session_id: "s1".to_string(),
            label: None,
            issue: None,
            pr: None,
            branch: None,
            confidence: 0.8,
            detected_at: "2026-02-27T10:00:00Z".to_string(),
            signals: vec![],
        };
        let curr = AgentPhaseState {
            phase: AgentPhase::Plan,
            confidence: 0.8,
            detected_at: "2026-02-27T10:00:10Z".to_string(), // only 10s
            ..prev.clone()
        };
        let config = DetectorConfig::default();
        assert!(detect_transition(&curr, Some(&prev), &config).is_none());
    }

    #[test]
    fn detect_transition_valid() {
        let prev = AgentPhaseState {
            phase: AgentPhase::Research,
            session_id: "s1".to_string(),
            label: None,
            issue: Some(45),
            pr: None,
            branch: Some("feat/auth-45".to_string()),
            confidence: 0.8,
            detected_at: "2026-02-27T10:00:00Z".to_string(),
            signals: vec![],
        };
        let curr = AgentPhaseState {
            phase: AgentPhase::Implement,
            confidence: 0.85,
            detected_at: "2026-02-27T10:05:00Z".to_string(), // 5 min later
            signals: vec!["branch created".to_string()],
            ..prev.clone()
        };
        let config = DetectorConfig::default();
        let t = detect_transition(&curr, Some(&prev), &config);
        assert!(t.is_some());
        let t = t.unwrap();
        assert_eq!(t.from, AgentPhase::Research);
        assert_eq!(t.to, AgentPhase::Implement);
    }

    #[test]
    fn detect_transition_no_previous_returns_none() {
        let curr = AgentPhaseState {
            phase: AgentPhase::Implement,
            session_id: "s1".to_string(),
            label: None,
            issue: None,
            pr: None,
            branch: None,
            confidence: 0.8,
            detected_at: "2026-02-27T10:00:00Z".to_string(),
            signals: vec![],
        };
        let config = DetectorConfig::default();
        assert!(detect_transition(&curr, None, &config).is_none());
    }

    #[test]
    fn read_write_phase_state_roundtrip() {
        let pid = "test_phase_rt";
        let _ = edda_store::ensure_dirs(pid);

        let state = AgentPhaseState {
            phase: AgentPhase::Implement,
            session_id: "sess-test".to_string(),
            label: Some("worker".to_string()),
            issue: Some(45),
            pr: None,
            branch: Some("feat/auth-45".to_string()),
            confidence: 0.85,
            detected_at: "2026-02-27T10:00:00Z".to_string(),
            signals: vec!["test signal".to_string()],
        };

        write_phase_state(pid, &state).unwrap();
        let loaded = read_phase_state(pid, "sess-test").unwrap();
        assert_eq!(loaded.phase, AgentPhase::Implement);
        assert_eq!(loaded.session_id, "sess-test");
        assert_eq!(loaded.issue, Some(45));

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn read_phase_state_missing() {
        assert!(read_phase_state("nonexistent_project", "nonexistent_session").is_none());
    }
}
