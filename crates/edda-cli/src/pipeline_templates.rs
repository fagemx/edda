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

/// Render the issue-derived lookup shared by the implementation gate and review prompt.
fn linked_open_pr_lookup(issue_id: u64) -> String {
    format!(
        r#"gh pr list --state open --limit 1000 --json url,closingIssuesReferences --jq '[.[] | select(any(.closingIssuesReferences[]?; .number == {issue_id}))] as $matches | if ($matches | length) != 1 then error("expected exactly one open PR linked to issue #{issue_id}") elif (($matches[0].url | type) != "string" or (($matches[0].url | test("^https://github[.]com/[A-Za-z0-9-]+/[A-Za-z0-9._-]+/pull/[1-9][0-9]*$")) | not)) then error("linked PR has malformed GitHub URL") else $matches[0].url end'"#
    )
}

/// Render a Standard pipeline plan as YAML.
pub fn render_standard_plan(issue_id: u64, title: &str, url: &str) -> String {
    let escaped_title = escape_yaml(title);
    let linked_pr_lookup = linked_open_pr_lookup(issue_id);
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
      Validate with the repository's current focused author/L0 policy.
      Before this phase completes, create or update exactly one open PR through
      existing authorization and link its closingIssuesReferences to issue
      #{issue_id}. If that is unavailable, report the blocker; do not claim that
      a local candidate is a PR. Do not merge.
    depends_on: [plan-approval]
    check:
      - type: cmd_succeeds
        cmd: >-
          {linked_pr_lookup}
    on_fail: ask

  - id: pr-review
    prompt: |
      Independently resolve the open PR linked to issue #{issue_id} by running:
        {linked_pr_lookup}
      Use its sole stdout URL for /pr-review, then verify code quality, test
      coverage, and adherence to the plan. Refuse a gh error, zero linked open
      PRs, multiple linked open PRs, or a malformed GitHub PR URL. Report the
      blocker; do not run /pr-review. No implementation output is transferred to
      this phase, and it does not merge.
    depends_on: [implement]
    on_fail: ask

  - id: pr-approval
    prompt: |
      The PR for issue #{issue_id} has been reviewed.
      Write an approval_request event for the repository's merge authority.
      Then wait for its decision. This pipeline, its worker, and its reviewer
      never gain merge authority and never merge the PR.
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
    let linked_pr_lookup = linked_open_pr_lookup(issue_id);
    format!(
        r#"name: pipeline-issue-{issue_id}
purpose: "Quick fix pipeline for issue #{issue_id}: {escaped_title}"

phases:
  - id: implement
    prompt: |
      Run /issue-action {issue_id} to implement the fix.
      Issue: {escaped_title}
      URL: {url}
      Validate with the repository's current focused author/L0 policy.
      Before this phase completes, create or update exactly one open PR through
      existing authorization and link its closingIssuesReferences to issue
      #{issue_id}. If that is unavailable, report the blocker; do not claim that
      a local candidate is a PR. Do not merge.
    check:
      - type: cmd_succeeds
        cmd: >-
          {linked_pr_lookup}
    on_fail: ask

  - id: pr-review
    prompt: |
      Independently resolve the open PR linked to issue #{issue_id} by running:
        {linked_pr_lookup}
      Use its sole stdout URL for /pr-review, then verify the fix is correct and
      tests pass. Refuse a gh error, zero linked open PRs, multiple linked open
      PRs, or a malformed GitHub PR URL. Report the blocker; do not run
      /pr-review. No implementation output is transferred to this phase, and it
      does not merge.
    depends_on: [implement]
    on_fail: ask

  - id: pr-approval
    prompt: |
      The PR for issue #{issue_id} has been reviewed.
      Write an approval_request event for the repository's merge authority.
      Then wait for its decision. This pipeline, its worker, and its reviewer
      never gain merge authority and never merge the PR.
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

    fn phase<'a>(plan: &'a serde_yaml::Value, id: &str) -> &'a serde_yaml::Value {
        plan["phases"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|phase| phase["id"].as_str() == Some(id))
            .unwrap()
    }

    #[test]
    fn implementation_and_review_independently_require_one_linked_open_pr() {
        for (issue_id, yaml) in [
            (
                7,
                render_standard_plan(7, "standard", "https://example.com/7"),
            ),
            (8, render_quickfix_plan(8, "quick", "https://example.com/8")),
        ] {
            let plan: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
            let implement = phase(&plan, "implement");
            let implement_prompt = implement["prompt"].as_str().unwrap();
            assert!(implement_prompt.contains("create or update exactly one open PR"));
            assert!(implement_prompt.contains("closingIssuesReferences"));
            assert!(implement_prompt.contains("focused author/L0 policy"));
            assert!(implement_prompt.contains("Do not merge"));

            let checks = implement["check"].as_sequence().unwrap();
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0]["type"].as_str(), Some("cmd_succeeds"));
            let check_cmd = checks[0]["cmd"].as_str().unwrap();
            assert_eq!(check_cmd, linked_open_pr_lookup(issue_id));
            assert!(check_cmd.starts_with("gh pr list --state open"));
            assert!(check_cmd.contains("closingIssuesReferences"));
            assert!(check_cmd.contains(&format!(".number == {issue_id}")));
            assert!(check_cmd.contains("if ($matches | length) != 1 then error"));
            assert!(check_cmd.contains("linked PR has malformed GitHub URL"));
            assert!(check_cmd.contains("^https://github[.]com/"));
            assert!(!check_cmd.contains(" || "));
            assert_eq!(check_cmd.matches("gh pr list").count(), 1);
            assert!(!yaml.contains("cargo test --workspace"));

            let review_prompt = phase(&plan, "pr-review")["prompt"].as_str().unwrap();
            assert!(review_prompt.contains("Independently resolve"));
            assert!(review_prompt.contains(check_cmd));
            assert!(review_prompt.contains("Refuse a gh error"));
            assert!(review_prompt.contains("zero linked open"));
            assert!(review_prompt.contains("multiple linked open"));
            assert!(review_prompt.contains("malformed GitHub PR URL"));
            assert!(review_prompt.contains("blocker"));
            assert!(review_prompt.contains("do not run"));
            assert!(review_prompt.contains("No implementation output is transferred"));
            assert!(!review_prompt.contains("returned by the implementation phase"));

            let approval_prompt = phase(&plan, "pr-approval")["prompt"].as_str().unwrap();
            assert!(approval_prompt.contains("never gain merge authority"));
            assert!(approval_prompt.contains("never merge the PR"));
        }
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
