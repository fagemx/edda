//! Pipeline template selection and YAML rendering.
//!
//! Generates conductor-compatible YAML plans from task intake context.
//! Templates reference existing skills (/issue-plan, /issue-action, /pr-review).

/// Pipeline type determines which phases are included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineType {
    /// plan → approval → implement → pr-review → approval
    Standard,
    /// implement → pr-review → approval (skip plan phase)
    QuickFix,
}

/// Select pipeline type based on task intake intent and labels.
pub fn select_pipeline(intent: &str, labels: &[String]) -> PipelineType {
    if intent == "fix" && labels.iter().any(|l| l == "small" || l == "trivial") {
        PipelineType::QuickFix
    } else {
        PipelineType::Standard
    }
}

/// Render a Standard pipeline plan as YAML.
pub fn render_standard_plan(issue_id: u64, title: &str, url: &str) -> String {
    let escaped_title = escape_yaml(title);
    format!(
        r#"name: pipeline-issue-{issue_id}
purpose: "Automated pipeline for issue #{issue_id}: {escaped_title}"

phases:
  - id: plan
    prompt: |
      Run /issue-plan {issue_id} to research, innovate, and plan for this issue.
      Issue: {escaped_title}
      URL: {url}
    on_fail: ask

  - id: plan-approval
    prompt: |
      The plan for issue #{issue_id} has been created.
      Write an approval_request event using:
        edda draft propose --title "Plan for #{issue_id}" --purpose "Approve implementation plan"
      Then wait for human approval.
    depends_on: [plan]
    check:
      - type: wait_until
        check:
          type: edda_event
          event_type: approval
        interval_sec: 30
        timeout_sec: 86400
        backoff: linear
    on_fail: ask

  - id: implement
    prompt: |
      Run /issue-action {issue_id} to implement the approved plan.
      Issue: {escaped_title}
      URL: {url}
    depends_on: [plan-approval]
    check:
      - type: cmd_succeeds
        cmd: "cargo test --workspace"
    on_fail: ask

  - id: pr-review
    prompt: |
      Run /pr-review to review the PR created by the implementation phase.
      Verify code quality, test coverage, and adherence to the plan.
    depends_on: [implement]
    on_fail: ask

  - id: pr-approval
    prompt: |
      The PR for issue #{issue_id} has been reviewed.
      Write an approval_request event for the PR merge.
      Then wait for human approval to merge.
    depends_on: [pr-review]
    check:
      - type: wait_until
        check:
          type: edda_event
          event_type: approval
        interval_sec: 30
        timeout_sec: 86400
        backoff: linear
    on_fail: ask
"#,
        issue_id = issue_id,
        escaped_title = escaped_title,
        url = url,
    )
}

/// Render a QuickFix pipeline plan as YAML (skips plan phase).
pub fn render_quickfix_plan(issue_id: u64, title: &str, url: &str) -> String {
    let escaped_title = escape_yaml(title);
    format!(
        r#"name: pipeline-issue-{issue_id}
purpose: "Quick fix pipeline for issue #{issue_id}: {escaped_title}"

phases:
  - id: implement
    prompt: |
      Run /issue-action {issue_id} to implement the fix.
      Issue: {escaped_title}
      URL: {url}
    check:
      - type: cmd_succeeds
        cmd: "cargo test --workspace"
    on_fail: ask

  - id: pr-review
    prompt: |
      Run /pr-review to review the PR created by the implementation phase.
      Verify the fix is correct and tests pass.
    depends_on: [implement]
    on_fail: ask

  - id: pr-approval
    prompt: |
      The PR for issue #{issue_id} has been reviewed.
      Write an approval_request event for the PR merge.
      Then wait for human approval to merge.
    depends_on: [pr-review]
    check:
      - type: wait_until
        check:
          type: edda_event
          event_type: approval
        interval_sec: 30
        timeout_sec: 86400
        backoff: linear
    on_fail: ask
"#,
        issue_id = issue_id,
        escaped_title = escaped_title,
        url = url,
    )
}

/// Escape a string for use in YAML double-quoted context.
fn escape_yaml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_pipeline_standard_default() {
        assert_eq!(select_pipeline("implement", &[]), PipelineType::Standard);
    }

    #[test]
    fn select_pipeline_quickfix() {
        let labels = vec!["small".to_string()];
        assert_eq!(select_pipeline("fix", &labels), PipelineType::QuickFix);

        let labels = vec!["trivial".to_string()];
        assert_eq!(select_pipeline("fix", &labels), PipelineType::QuickFix);
    }

    #[test]
    fn select_pipeline_fix_without_small() {
        let labels = vec!["enhancement".to_string()];
        assert_eq!(select_pipeline("fix", &labels), PipelineType::Standard);
    }

    #[test]
    fn render_standard_plan_valid_yaml() {
        let yaml = render_standard_plan(42, "feat: add auth", "https://github.com/o/r/issues/42");
        let plan: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("should be valid YAML");
        assert_eq!(plan["name"].as_str().unwrap(), "pipeline-issue-42");
    }

    #[test]
    fn render_standard_plan_has_required_phases() {
        let yaml = render_standard_plan(1, "test", "https://example.com");
        let plan: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let phases = plan["phases"].as_sequence().unwrap();
        let ids: Vec<&str> = phases.iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            vec![
                "plan",
                "plan-approval",
                "implement",
                "pr-review",
                "pr-approval"
            ]
        );
    }

    #[test]
    fn render_quickfix_plan_skips_plan_phase() {
        let yaml = render_quickfix_plan(5, "fix: null ptr", "https://example.com");
        let plan: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let phases = plan["phases"].as_sequence().unwrap();
        let ids: Vec<&str> = phases.iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert!(!ids.contains(&"plan"));
        assert!(!ids.contains(&"plan-approval"));
        assert!(ids.contains(&"implement"));
    }

    #[test]
    fn render_plan_injects_issue_context() {
        let yaml = render_standard_plan(
            99,
            "feat: user auth",
            "https://github.com/owner/repo/issues/99",
        );
        assert!(yaml.contains("issue #99") || yaml.contains("issue-99"));
        assert!(yaml.contains("user auth"));
        assert!(yaml.contains("https://github.com/owner/repo/issues/99"));
    }

    #[test]
    fn render_plan_parses_with_conductor() {
        let yaml = render_standard_plan(10, "feat: test", "https://example.com");
        // Use conductor's parse_plan to validate full compatibility
        let result = edda_conductor::plan::parser::parse_plan(&yaml);
        assert!(
            result.is_ok(),
            "conductor parse_plan failed: {:?}",
            result.err()
        );
    }

    #[test]
    fn escape_yaml_handles_special_chars() {
        assert_eq!(escape_yaml(r#"say "hello""#), r#"say \"hello\""#);
        assert_eq!(escape_yaml(r"path\to\file"), r"path\\to\\file");
    }
}
