//! Pipeline template selection and YAML rendering.
//!
//! Generates conductor-compatible one-phase routes from task intake context.
//! Both routes delegate implementation and delivery to `/issue-action`, which
//! follows the canonical `delivery-flow/1` operating contract.

/// Pipeline type determines how acceptance is established in the delivery phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineType {
    /// Reuse an accepted plan, or clarify acceptance boundedly before implementation.
    Standard,
    /// Skip a separate planning phase, but never skip clear acceptance.
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

/// Render a Standard pipeline as one terminal implementation/delivery phase.
pub fn render_standard_plan(issue_id: u64, title: &str, url: &str) -> String {
    let escaped_title = escape_yaml(title);
    format!(
        r#"name: pipeline-issue-{issue_id}
purpose: "Delivery route for issue #{issue_id}: {escaped_title}"

phases:
  - id: delivery
    prompt: |
      Follow coord-orchestrate's delivery-flow/1 operating contract.
      Run /issue-action {issue_id} to implement and deliver the accepted outcome.
      Issue: {escaped_title}
      URL: {url}
      Standard acceptance: reuse an accepted plan when one exists; otherwise
      perform bounded acceptance clarification in this same phase before
      implementation. Do not create a separate planning or approval phase, and
      do not implement while acceptance remains materially unclear.
      Validate with the repository's current focused author/L0 policy.
      Complete only to the output authorized by the assigned brief: a truthful
      local candidate, commit, or authorized PR. Report any unavailable
      publication path as a blocker instead of overstating the result.
      Conductor completion records only this implementation/delivery phase. It
      is not independent review, task completion, or merge, and grants no
      review or merge authority.
    on_fail: ask
"#,
        issue_id = issue_id,
        escaped_title = escaped_title,
        url = url,
    )
}

/// Render a QuickFix pipeline as one terminal implementation/delivery phase.
pub fn render_quickfix_plan(issue_id: u64, title: &str, url: &str) -> String {
    let escaped_title = escape_yaml(title);
    format!(
        r#"name: pipeline-issue-{issue_id}
purpose: "Quick fix delivery route for issue #{issue_id}: {escaped_title}"

phases:
  - id: delivery
    prompt: |
      Follow coord-orchestrate's delivery-flow/1 operating contract.
      Run /issue-action {issue_id} to implement and deliver the fix.
      Issue: {escaped_title}
      URL: {url}
      QuickFix skips a separate planning phase, but never skips clear
      acceptance. Reuse the accepted outcome when it is clear; if acceptance
      is materially unclear, clarify it boundedly in this same phase before
      implementation.
      Validate with the repository's current focused author/L0 policy.
      Complete only to the output authorized by the assigned brief: a truthful
      local candidate, commit, or authorized PR. Report any unavailable
      publication path as a blocker instead of overstating the result.
      Conductor completion records only this implementation/delivery phase. It
      is not independent review, task completion, or merge, and grants no
      review or merge authority.
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

    fn parsed(yaml: &str) -> serde_yaml::Value {
        serde_yaml::from_str(yaml).expect("rendered pipeline should be valid YAML")
    }

    fn assert_single_delivery_phase(yaml: &str, issue_id: u64) {
        let plan = parsed(yaml);
        let phases = plan["phases"]
            .as_sequence()
            .expect("phases should be a list");
        assert_eq!(phases.len(), 1, "pipeline must stay a one-phase thin route");

        let delivery = &phases[0];
        assert_eq!(delivery["id"].as_str(), Some("delivery"));
        assert!(delivery["depends_on"].is_null());
        assert!(delivery["check"].is_null());
        assert!(delivery["gate"].is_null());

        let prompt = delivery["prompt"].as_str().expect("delivery prompt");
        assert!(prompt.contains("delivery-flow/1"));
        assert!(prompt.contains(&format!("/issue-action {issue_id}")));
        assert!(prompt.contains("focused author/L0 policy"));
        assert!(prompt.contains("local candidate, commit, or authorized PR"));
        assert!(prompt.contains("not independent review, task completion, or merge"));
        assert!(prompt.contains("grants no\nreview or merge authority"));

        for forbidden in [
            "plan-approval",
            "pr-review",
            "pr-approval",
            "wait_until",
            "cmd_succeeds",
            "closingIssuesReferences",
            "gh pr list",
            "gh pr merge",
            "cargo test --workspace",
            "create or update exactly one open PR",
        ] {
            assert!(
                !yaml.contains(forbidden),
                "one-phase route revived forbidden lifecycle text: {forbidden}"
            );
        }
    }

    #[test]
    fn standard_is_one_delivery_phase_with_bounded_acceptance() {
        let yaml = render_standard_plan(42, "feat: add auth", "https://github.com/o/r/issues/42");
        assert_single_delivery_phase(&yaml, 42);
        let prompt = parsed(&yaml)["phases"][0]["prompt"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(prompt.contains("Standard acceptance: reuse an accepted plan"));
        assert!(prompt.contains("otherwise\nperform bounded acceptance clarification"));
        assert!(prompt.contains("same phase before\nimplementation"));
        assert!(prompt.contains("do not implement while acceptance remains materially unclear"));
    }

    #[test]
    fn quickfix_is_one_delivery_phase_without_skipping_acceptance() {
        let yaml = render_quickfix_plan(5, "fix: null ptr", "https://example.com");
        assert_single_delivery_phase(&yaml, 5);
        let prompt = parsed(&yaml)["phases"][0]["prompt"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(prompt.contains("QuickFix skips a separate planning phase"));
        assert!(prompt.contains("never skips clear\nacceptance"));
        assert!(prompt.contains("clarify it boundedly in this same phase"));
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
    fn both_routes_parse_with_conductor() {
        for yaml in [
            render_standard_plan(10, "feat: test", "https://example.com"),
            render_quickfix_plan(11, "fix: test", "https://example.com"),
        ] {
            let result = edda_conductor::plan::parser::parse_plan(&yaml);
            assert!(
                result.is_ok(),
                "conductor parse_plan failed: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn escape_yaml_handles_special_chars() {
        assert_eq!(escape_yaml(r#"say "hello""#), r#"say \"hello\""#);
        assert_eq!(escape_yaml(r"path\to\file"), r"path\\to\\file");
    }
}
