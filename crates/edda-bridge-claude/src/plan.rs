use crate::signals::{CommitInfo, TaskSnapshot};
use std::collections::HashSet;

// ── Data Structures ──

#[derive(Debug, Clone)]
pub(crate) struct PlanStep {
    pub index: usize,       // 1-based (from heading number)
    pub title: String,       // heading text without the "## Step N:" prefix
    pub heading_line: usize, // 0-based line index in plan content
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StepStatus {
    Done,    // matched task completed, or positionally before first active
    Active,  // matched task in_progress
    Pending, // no evidence or future
}

#[derive(Debug, Clone)]
pub(crate) struct StepProgress {
    pub step: PlanStep,
    pub status: StepStatus,
}

// ── Stop Words ──

/// Words too common to be useful for matching.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "into", "that", "this", "will",
    "step", "plan", "add", "new", "use", "set", "get", "fix", "run",
];

// ── Parsing ──

/// Parse numbered step headings from plan content.
///
/// Supports two patterns observed in real plan files:
/// - Pattern A: `## Step N: Title`
/// - Pattern B: `## N. Title`
///
/// Returns empty vec if fewer than 2 steps found (fallback to truncation).
pub(crate) fn parse_plan_steps(content: &str) -> Vec<PlanStep> {
    let mut steps = Vec::new();

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with("## ") {
            continue;
        }
        let after_hash = &trimmed[3..]; // skip "## "

        // Pattern A: "Step N: Title" or "Step N — Title"
        if let Some(rest) = after_hash.strip_prefix("Step ").or_else(|| after_hash.strip_prefix("step ")) {
            if let Some((index, title)) = parse_step_number_and_title(rest) {
                steps.push(PlanStep {
                    index,
                    title,
                    heading_line: line_idx,
                });
                continue;
            }
        }

        // Pattern B: "N. Title" or "N: Title"
        if let Some((index, title)) = parse_step_number_and_title(after_hash) {
            steps.push(PlanStep {
                index,
                title,
                heading_line: line_idx,
            });
        }
    }

    // Only treat as stepped plan if 2+ steps found
    if steps.len() < 2 {
        return Vec::new();
    }

    steps
}

/// Try to parse "N: Title" or "N. Title" from a string.
/// Returns (index, title) if successful.
fn parse_step_number_and_title(s: &str) -> Option<(usize, String)> {
    let s = s.trim();
    // Find the end of the number
    let num_end = s.find(|c: char| !c.is_ascii_digit())?;
    if num_end == 0 {
        return None;
    }
    let index: usize = s[..num_end].parse().ok()?;
    if index == 0 {
        return None;
    }

    // After the number, expect a separator: '.', ':', ' —', whitespace
    let rest = &s[num_end..];
    let title_start = if rest.starts_with(". ")
        || rest.starts_with(": ")
        || rest.starts_with(" —")
        || rest.starts_with(" -")
    {
        // Skip separator + space
        if rest.starts_with(" —") || rest.starts_with(" -") {
            rest.find(char::is_alphanumeric).unwrap_or(rest.len())
        } else {
            2
        }
    } else if rest.starts_with('.') || rest.starts_with(':') {
        // Separator without space
        1
    } else {
        return None;
    };

    let title = rest[title_start..].trim().to_string();
    if title.is_empty() {
        return None;
    }

    Some((index, title))
}

// ── Token Extraction ──

/// Extract significant tokens from a string for matching.
/// Filters out stop words and tokens <= 3 chars.
fn extract_tokens(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() > 3)
        .filter(|w| !STOP_WORDS.contains(w))
        .map(|w| w.to_string())
        .collect()
}

// ── Progress Matching ──

/// Cross-reference plan steps with tasks and commits to determine progress.
///
/// Strategy (conservative, L1 deterministic):
/// 1. For each step, extract tokens from title
/// 2. Match against task subjects using token overlap (threshold >= 0.5)
/// 3. Match against commit messages for completion evidence
/// 4. Positional rule: steps before first Active are marked Done
pub(crate) fn match_step_progress(
    steps: &[PlanStep],
    tasks: &[TaskSnapshot],
    commits: &[CommitInfo],
) -> Vec<StepProgress> {
    let task_data: Vec<(&TaskSnapshot, HashSet<String>)> = tasks
        .iter()
        .map(|t| (t, extract_tokens(&t.subject)))
        .collect();

    let commit_tokens: Vec<HashSet<String>> = commits
        .iter()
        .map(|c| extract_tokens(&c.message))
        .collect();

    let mut result: Vec<StepProgress> = steps
        .iter()
        .map(|step| {
            let step_tokens = extract_tokens(&step.title);
            if step_tokens.is_empty() {
                return StepProgress {
                    step: step.clone(),
                    status: StepStatus::Pending,
                };
            }

            // Try matching against tasks
            let mut best_task_match: Option<(&TaskSnapshot, f64)> = None;
            for (task, task_tok) in &task_data {
                if task_tok.is_empty() {
                    continue;
                }
                let overlap = step_tokens.intersection(task_tok).count();
                let min_len = step_tokens.len().min(task_tok.len());
                let score = overlap as f64 / min_len as f64;
                if score >= 0.5
                    && best_task_match.as_ref().is_none_or(|(_, s)| score > *s)
                {
                    best_task_match = Some((task, score));
                }
            }

            if let Some((task, _)) = best_task_match {
                let status = match task.status.as_str() {
                    "completed" => StepStatus::Done,
                    "in_progress" => StepStatus::Active,
                    _ => StepStatus::Pending,
                };
                return StepProgress {
                    step: step.clone(),
                    status,
                };
            }

            // Try matching against commits (completion evidence)
            for ctok in &commit_tokens {
                let overlap = step_tokens.intersection(ctok).count();
                if overlap >= 1 && !step_tokens.is_empty() {
                    let min_len = step_tokens.len().min(ctok.len());
                    let score = overlap as f64 / min_len as f64;
                    if score >= 0.3 {
                        return StepProgress {
                            step: step.clone(),
                            status: StepStatus::Done,
                        };
                    }
                }
            }

            StepProgress {
                step: step.clone(),
                status: StepStatus::Pending,
            }
        })
        .collect();

    // Positional rule: steps before first Active are implicitly Done
    apply_positional_rule(&mut result);

    result
}

/// Steps before the first Active step (with no evidence) are marked Done.
/// Rationale: plans execute sequentially, so prior steps are likely complete.
fn apply_positional_rule(progress: &mut [StepProgress]) {
    if let Some(first_active_idx) = progress.iter().position(|p| p.status == StepStatus::Active) {
        for p in progress[..first_active_idx].iter_mut() {
            if p.status == StepStatus::Pending {
                p.status = StepStatus::Done;
            }
        }
    }
}

// ── Rendering ──

/// Default max chars for plan excerpt (same as dispatch.rs constant).
const PLAN_MAX_CHARS: usize = 700;
/// Minimum chars reserved for current step body.
const CURRENT_STEP_MIN_CHARS: usize = 300;

/// Render a plan with progress tracking.
///
/// Returns `None` if the plan has no recognizable step structure (caller
/// should fall back to simple truncation).
pub(crate) fn render_plan_with_progress(
    content: &str,
    project_id: &str,
    filename: &str,
    mtime_str: &str,
) -> Option<String> {
    let steps = parse_plan_steps(content);
    if steps.is_empty() {
        return None;
    }

    // Load signals from state files
    let tasks: Vec<TaskSnapshot> =
        crate::signals::load_state_vec(project_id, "active_tasks.json", "tasks");
    let commits: Vec<CommitInfo> =
        crate::signals::load_state_vec(project_id, "recent_commits.json", "commits");

    let progress = match_step_progress(&steps, &tasks, &commits);

    let done_count = progress.iter().filter(|p| p.status == StepStatus::Done).count();
    let total = progress.len();

    // Build step list
    let mut lines = Vec::new();
    lines.push(format!("## Active Plan\n> {filename} ({mtime_str})"));
    lines.push(format!("\nProgress: {done_count}/{total} steps completed\n"));

    for p in &progress {
        let icon = match p.status {
            StepStatus::Done => "\u{2705}",   // ✅
            StepStatus::Active => "\u{1f527}", // 🔧
            StepStatus::Pending => "\u{2b1c}", // ⬜
        };
        lines.push(format!("{icon} Step {}: {}", p.step.index, p.step.title));
    }

    let header = lines.join("\n");

    // Find current step (first Active, or first Pending if no Active)
    let current_step = progress
        .iter()
        .find(|p| p.status == StepStatus::Active)
        .or_else(|| progress.iter().find(|p| p.status == StepStatus::Pending));

    if let Some(current) = current_step {
        let body = extract_step_body(content, current.step.heading_line, &steps);
        let remaining_budget = PLAN_MAX_CHARS.saturating_sub(header.len()).max(CURRENT_STEP_MIN_CHARS);
        let truncated = truncate_to_budget(&body, remaining_budget);

        Some(format!("{header}\n\n### Current Step\n{truncated}"))
    } else {
        // All done — just show the step list
        Some(header)
    }
}

/// Extract the body text of a step (from heading to next step heading).
fn extract_step_body(content: &str, heading_line: usize, all_steps: &[PlanStep]) -> String {
    let lines: Vec<&str> = content.lines().collect();

    // Find the next step's heading line (or end of content)
    let end_line = all_steps
        .iter()
        .filter(|s| s.heading_line > heading_line)
        .map(|s| s.heading_line)
        .min()
        .unwrap_or(lines.len());

    // Skip the heading line itself, collect body
    let start = heading_line + 1;
    if start >= lines.len() || start >= end_line {
        return String::new();
    }

    lines[start..end_line]
        .to_vec()
        .join("\n")
        .trim()
        .to_string()
}

/// Truncate text to fit within a character budget.
fn truncate_to_budget(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    // Find a line boundary near the budget
    let mut end = 0;
    for line in text.lines() {
        let next = end + line.len() + 1; // +1 for newline
        if next > max_chars {
            break;
        }
        end = next;
    }
    if end == 0 {
        // Single long line — hard truncate
        let truncated: String = text.chars().take(max_chars.saturating_sub(15)).collect();
        format!("{truncated}\n...(truncated)")
    } else {
        format!("{}...(truncated)", &text[..end])
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_plan_steps tests ──

    #[test]
    fn parse_step_pattern_a() {
        let content = "\
# Plan Title

## Step 1: Exit Code 1 for Warnings
Some description

## Step 2: Injection Dedup
More description

## Step 3: Last Assistant Message
Final description
";
        let steps = parse_plan_steps(content);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].index, 1);
        assert_eq!(steps[0].title, "Exit Code 1 for Warnings");
        assert_eq!(steps[1].index, 2);
        assert_eq!(steps[1].title, "Injection Dedup");
        assert_eq!(steps[2].index, 3);
        assert_eq!(steps[2].title, "Last Assistant Message");
    }

    #[test]
    fn parse_step_pattern_b() {
        let content = "\
# Plan

## 1. Setup database schema
Details

## 2. Implement API endpoints
Details

## 3. Add integration tests
Details
";
        let steps = parse_plan_steps(content);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].index, 1);
        assert_eq!(steps[0].title, "Setup database schema");
        assert_eq!(steps[1].index, 2);
        assert_eq!(steps[1].title, "Implement API endpoints");
        assert_eq!(steps[2].index, 3);
        assert_eq!(steps[2].title, "Add integration tests");
    }

    #[test]
    fn parse_mixed_patterns() {
        let content = "\
# Plan

## Step 1: Setup the foundation
Stuff

## 2. Build the features
More stuff

## Step 3: Write tests
Even more stuff
";
        let steps = parse_plan_steps(content);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].title, "Setup the foundation");
        assert_eq!(steps[1].title, "Build the features");
        assert_eq!(steps[2].title, "Write tests");
    }

    #[test]
    fn parse_no_steps_returns_empty() {
        let content = "\
# Plan

## Context
Some context

## Design Decisions
Some decisions

## Implementation
Some implementation
";
        let steps = parse_plan_steps(content);
        assert!(steps.is_empty(), "Non-numbered headings should not match");
    }

    #[test]
    fn parse_single_step_returns_empty() {
        let content = "\
# Plan

## Step 1: Only one step
Description
";
        let steps = parse_plan_steps(content);
        assert!(steps.is_empty(), "Single step should trigger fallback");
    }

    // ── Token extraction tests ──

    #[test]
    fn extract_tokens_filters_stop_words() {
        let tokens = extract_tokens("Fix the authentication bug with session");
        assert!(tokens.contains("authentication"));
        assert!(tokens.contains("session"));
        assert!(!tokens.contains("the"));  // stop word
        assert!(!tokens.contains("fix"));  // stop word
        assert!(!tokens.contains("bug"));  // too short (3 chars)
    }

    // ── match_step_progress tests ──

    #[test]
    fn match_task_to_step() {
        let steps = vec![
            PlanStep { index: 1, title: "Exit Code for Warnings".into(), heading_line: 0 },
            PlanStep { index: 2, title: "Injection Dedup".into(), heading_line: 5 },
        ];
        let tasks = vec![
            TaskSnapshot {
                id: "1".into(),
                subject: "Implement exit code warnings".into(),
                status: "completed".into(),
            },
            TaskSnapshot {
                id: "2".into(),
                subject: "Add injection dedup logic".into(),
                status: "in_progress".into(),
            },
        ];

        let progress = match_step_progress(&steps, &tasks, &[]);
        assert_eq!(progress[0].status, StepStatus::Done);
        assert_eq!(progress[1].status, StepStatus::Active);
    }

    #[test]
    fn match_commit_evidence() {
        let steps = vec![
            PlanStep { index: 1, title: "Privacy Stripping".into(), heading_line: 0 },
            PlanStep { index: 2, title: "Session Index GC".into(), heading_line: 5 },
        ];
        let commits = vec![CommitInfo {
            hash: "abc1234".into(),
            message: "feat(bridge): privacy stripping + redact patterns".into(),
        }];

        let progress = match_step_progress(&steps, &[], &commits);
        assert_eq!(progress[0].status, StepStatus::Done, "commit should mark step done");
        assert_eq!(progress[1].status, StepStatus::Pending, "unmatched step stays pending");
    }

    #[test]
    fn positional_rule_marks_prior_done() {
        let steps = vec![
            PlanStep { index: 1, title: "Exit Code for Warnings".into(), heading_line: 0 },
            PlanStep { index: 2, title: "Injection Dedup Logic".into(), heading_line: 5 },
            PlanStep { index: 3, title: "Privacy Stripping Patterns".into(), heading_line: 10 },
        ];
        let tasks = vec![TaskSnapshot {
            id: "1".into(),
            subject: "Implement injection dedup".into(),
            status: "in_progress".into(),
        }];

        let progress = match_step_progress(&steps, &tasks, &[]);
        assert_eq!(progress[0].status, StepStatus::Done, "step 1 before active → done");
        assert_eq!(progress[1].status, StepStatus::Active, "step 2 matched → active");
        assert_eq!(progress[2].status, StepStatus::Pending, "step 3 after active → pending");
    }

    #[test]
    fn no_signals_all_pending() {
        let steps = vec![
            PlanStep { index: 1, title: "Setup database schema".into(), heading_line: 0 },
            PlanStep { index: 2, title: "Build API endpoints".into(), heading_line: 5 },
        ];

        let progress = match_step_progress(&steps, &[], &[]);
        assert!(progress.iter().all(|p| p.status == StepStatus::Pending));
    }

    // ── extract_step_body tests ──

    #[test]
    fn extract_body_between_steps() {
        let content = "\
## Step 1: First
Body line 1
Body line 2

## Step 2: Second
Body of second
";
        let steps = parse_plan_steps(content);
        let body = extract_step_body(content, steps[0].heading_line, &steps);
        assert!(body.contains("Body line 1"));
        assert!(body.contains("Body line 2"));
        assert!(!body.contains("Body of second"));
    }

    #[test]
    fn extract_body_last_step() {
        let content = "\
## Step 1: First
Earlier

## Step 2: Last Step
Final body line 1
Final body line 2
";
        let steps = parse_plan_steps(content);
        let body = extract_step_body(content, steps[1].heading_line, &steps);
        assert!(body.contains("Final body line 1"));
        assert!(body.contains("Final body line 2"));
    }

    // ── truncate_to_budget tests ──

    #[test]
    fn truncate_short_text_unchanged() {
        let text = "Short text\nSecond line";
        assert_eq!(truncate_to_budget(text, 100), text);
    }

    #[test]
    fn truncate_respects_line_boundary() {
        let text = "Line 1 here\nLine 2 here\nLine 3 here\nLine 4 here";
        let result = truncate_to_budget(text, 30);
        assert!(result.contains("Line 1 here"));
        assert!(result.contains("Line 2 here"));
        assert!(result.contains("...(truncated)"));
        assert!(!result.contains("Line 4"));
    }

    // ── render_plan_with_progress tests ──
    // Note: render_plan_with_progress reads state files, so full integration
    // tests are in dispatch.rs. Here we test via match_step_progress + manual render.

    #[test]
    fn render_fallback_no_steps() {
        // Non-stepped plan should return None
        let content = "# Plan\n\n## Context\nSome context\n\n## Design\nSome design\n";
        let result = render_plan_with_progress(content, "nonexistent_project", "test.md", "2026-01-01");
        assert!(result.is_none());
    }
}
