//! GH-765: deterministic operator digest from the ledger.
//!
//! `edda recap --digest` prints the three ledger-sourced sections of the
//! daily fleet digest (例外 / 成本 / 明天會做的) for the last window. It is
//! offline and side-effect free: no LLM, no chronicle state, no writes.
//! Everything it prints is embedded verbatim by
//! `scripts/fleet/daily-digest.sh`, so the output format is a contract.

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Utc};
use edda_ledger::{Ledger, TaskStatus};
use std::path::Path;

/// Everything the renderer needs, pre-collected so `render` is testable
/// without a ledger.
pub struct DigestInput {
    pub unratified_decisions: Vec<UnratifiedDecision>,
    pub blocked_failed_tasks: Vec<TaskLine>,
    pub ready_tasks: Vec<TaskLine>,
    pub session_costs: CostSamples,
    pub execution_costs: CostSamples,
    pub telemetry_costs: CostSamples,
}

pub struct UnratifiedDecision {
    pub key: String,
    pub authority: String,
    pub ts: String,
}

pub struct TaskLine {
    pub id: u64,
    pub title: String,
    pub status: String,
    pub assignee: Option<String>,
}

/// Cost samples from one ledger source. A sample is present only when the
/// event actually carried a number: `Some(0.0)` is a measured zero, an
/// absent/null field is unmeasured and never conflated with `$0.00` (GH-585).
#[derive(Default)]
pub struct CostSamples {
    pub measured: Vec<f64>,
    pub unmeasured: usize,
}

/// CLI entry point. Parse failures exit 2 (usage error), matching how
/// `main.rs` treats contract violations.
pub fn execute(repo_root: &Path, since: Option<&str>) -> Result<()> {
    let now = Utc::now();
    let window_start = match parse_since(since, now) {
        Ok(start) => start,
        Err(err) => {
            eprintln!("Error: {err:#}");
            std::process::exit(2);
        }
    };
    let input = collect(repo_root, window_start, now)?;
    print!("{}", render(&input));
    Ok(())
}

/// `None` → last 24h; `"<N>h"` / `"<N>d"` → relative to `now`; otherwise
/// an RFC3339 timestamp.
pub fn parse_since(s: Option<&str>, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let Some(v) = s else {
        return Ok(now - Duration::hours(24));
    };
    if let Some(hours) = v.strip_suffix('h').and_then(|n| n.parse::<i64>().ok()) {
        return Ok(now - Duration::hours(hours));
    }
    if let Some(days) = v.strip_suffix('d').and_then(|n| n.parse::<i64>().ok()) {
        return Ok(now - Duration::days(days));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(v) {
        return Ok(dt.with_timezone(&Utc));
    }
    bail!("invalid --since '{v}' (expected RFC3339, <N>h or <N>d)")
}

fn collect(repo_root: &Path, since: DateTime<Utc>, now: DateTime<Utc>) -> Result<DigestInput> {
    let ledger = Ledger::open(repo_root)?;

    // An unratified decision is an exception until it is ratified, so this
    // list is deliberately NOT windowed.
    let ratified = ledger.ratified_decisions_map()?;
    let mut unratified: Vec<UnratifiedDecision> = ledger
        .active_decisions(None, None, None, None)?
        .into_iter()
        .filter(|d| !ratified.contains_key(&d.event_id))
        .map(|d| UnratifiedDecision {
            key: d.key,
            authority: d.authority,
            ts: d.ts.unwrap_or_else(|| "unknown".to_string()),
        })
        .collect();
    unratified.sort_by(|a, b| a.key.cmp(&b.key));

    let mut blocked_failed = Vec::new();
    let mut ready = Vec::new();
    for t in ledger.task_views()? {
        let line = TaskLine {
            id: t.task_id,
            title: t.title,
            status: t.status.to_string(),
            assignee: t.assignee,
        };
        match t.status {
            TaskStatus::Blocked | TaskStatus::Failed => blocked_failed.push(line),
            TaskStatus::Ready => ready.push(line),
            TaskStatus::Running | TaskStatus::Done => {}
        }
    }
    blocked_failed.sort_by_key(|t| t.id);
    ready.sort_by_key(|t| t.id);

    let mut session_costs = CostSamples::default();
    let mut execution_costs = CostSamples::default();
    let mut telemetry_costs = CostSamples::default();
    for event in ledger.iter_events()? {
        let ts = match DateTime::parse_from_rfc3339(&event.ts) {
            Ok(ts) => ts.with_timezone(&Utc),
            Err(_) => continue,
        };
        if ts < since || ts > now {
            continue;
        }
        match event.event_type.as_str() {
            "note" => {
                let tagged = event
                    .payload
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| t.as_str())
                            .any(|t| t == "session_digest")
                    })
                    .unwrap_or(false);
                if !tagged {
                    continue;
                }
                match event
                    .payload
                    .get("session_stats")
                    .and_then(|s| s.get("estimated_cost_usd"))
                    .and_then(|v| v.as_f64())
                {
                    Some(cost) => session_costs.measured.push(cost),
                    None => session_costs.unmeasured += 1,
                }
            }
            "execution_event" => {
                if let Some(cost) = event
                    .payload
                    .get("usage")
                    .and_then(|u| u.get("cost_usd"))
                    .and_then(|v| v.as_f64())
                {
                    execution_costs.measured.push(cost);
                }
            }
            "cycle_telemetry" => {
                if let Some(cost) = event
                    .payload
                    .get("cost")
                    .and_then(|c| c.get("total_usd"))
                    .and_then(|v| v.as_f64())
                {
                    telemetry_costs.measured.push(cost);
                }
            }
            _ => {}
        }
    }

    Ok(DigestInput {
        unratified_decisions: unratified,
        blocked_failed_tasks: blocked_failed,
        ready_tasks: ready,
        session_costs,
        execution_costs,
        telemetry_costs,
    })
}

/// Render the three sections exactly as the shell embeds them: Markdown,
/// nothing before or after, single trailing newline, no trailing spaces.
pub fn render(input: &DigestInput) -> String {
    let mut out = String::new();

    out.push_str("## 例外\n");
    let mut decisions: Vec<&UnratifiedDecision> = input.unratified_decisions.iter().collect();
    decisions.sort_by(|a, b| a.key.cmp(&b.key));
    let mut blocked: Vec<&TaskLine> = input.blocked_failed_tasks.iter().collect();
    blocked.sort_by_key(|t| t.id);
    if decisions.is_empty() && blocked.is_empty() {
        out.push_str("（無）\n");
    } else {
        for d in decisions {
            out.push_str(&format!(
                "- decision `{}` — unratified ({}), recorded {}\n",
                d.key, d.authority, d.ts
            ));
        }
        for t in blocked {
            out.push_str(&format!("- task #{} {} — {}\n", t.id, t.title, t.status));
        }
    }

    out.push_str("## 成本\n");
    out.push_str(&format!(
        "- session_stats.estimated_cost_usd：{}\n",
        session_cost_line(&input.session_costs)
    ));
    out.push_str(&format!(
        "- execution_event.usage.cost_usd：{}\n",
        plain_cost_line(&input.execution_costs)
    ));
    out.push_str(&format!(
        "- cycle_telemetry.cost.total_usd：{}\n",
        plain_cost_line(&input.telemetry_costs)
    ));

    out.push_str("## 明天會做的\n");
    let mut ready: Vec<&TaskLine> = input.ready_tasks.iter().collect();
    ready.sort_by_key(|t| t.id);
    ready.truncate(3);
    if ready.is_empty() {
        out.push_str("（無）\n");
    } else {
        for t in ready {
            let assignee = t.assignee.as_deref().unwrap_or("unassigned");
            out.push_str(&format!("- task #{} {}（{}）\n", t.id, t.title, assignee));
        }
    }

    out
}

fn session_cost_line(c: &CostSamples) -> String {
    if c.measured.is_empty() {
        format!("n/a（0 筆量測，{} 筆未量測）", c.unmeasured)
    } else {
        format!(
            "${:.2}（{} 筆量測，{} 筆未量測）",
            c.measured.iter().sum::<f64>(),
            c.measured.len(),
            c.unmeasured
        )
    }
}

fn plain_cost_line(c: &CostSamples) -> String {
    if c.measured.is_empty() {
        "n/a（0 筆量測）".to_string()
    } else {
        format!(
            "${:.2}（{} 筆）",
            c.measured.iter().sum::<f64>(),
            c.measured.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_input() -> DigestInput {
        DigestInput {
            unratified_decisions: vec![],
            blocked_failed_tasks: vec![],
            ready_tasks: vec![],
            session_costs: CostSamples::default(),
            execution_costs: CostSamples::default(),
            telemetry_costs: CostSamples::default(),
        }
    }

    #[test]
    fn render_empty_input_prints_無_and_na() {
        let out = render(&empty_input());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            vec![
                "## 例外",
                "（無）",
                "## 成本",
                "- session_stats.estimated_cost_usd：n/a（0 筆量測，0 筆未量測）",
                "- execution_event.usage.cost_usd：n/a（0 筆量測）",
                "- cycle_telemetry.cost.total_usd：n/a（0 筆量測）",
                "## 明天會做的",
                "（無）",
            ],
            "full output was: {out:?}"
        );
        assert!(out.ends_with('\n') && !out.ends_with("\n\n"));
    }

    #[test]
    fn render_lists_unratified_decisions_and_blocked_tasks_in_order() {
        let mut input = empty_input();
        input.unratified_decisions = vec![
            UnratifiedDecision {
                key: "deploy.target".into(),
                authority: "agent".into(),
                ts: "2026-09-03T02:00:00Z".into(),
            },
            UnratifiedDecision {
                key: "db.engine".into(),
                authority: "human".into(),
                ts: "2026-09-03T01:00:00Z".into(),
            },
        ];
        input.blocked_failed_tasks = vec![
            TaskLine {
                id: 9,
                title: "later".into(),
                status: "failed".into(),
                assignee: None,
            },
            TaskLine {
                id: 4,
                title: "earlier".into(),
                status: "blocked".into(),
                assignee: None,
            },
        ];
        let out = render(&input);
        let idx = |needle: &str| out.find(needle).unwrap_or(usize::MAX);
        assert!(idx("`db.engine`") < idx("`deploy.target`"), "out={out}");
        assert!(idx("task #4") < idx("task #9"), "out={out}");
        assert!(
            idx("`deploy.target`") < idx("task #4"),
            "decisions before tasks, out={out}"
        );
        assert!(out.contains(
            "- decision `db.engine` — unratified (human), recorded 2026-09-03T01:00:00Z"
        ));
        assert!(out.contains("- task #9 later — failed"));
    }

    #[test]
    fn render_cost_sums_measured_and_counts_unmeasured() {
        let mut input = empty_input();
        input.session_costs = CostSamples {
            measured: vec![1.5, 0.0],
            unmeasured: 1,
        };
        let out = render(&input);
        assert!(
            out.contains("- session_stats.estimated_cost_usd：$1.50（2 筆量測，1 筆未量測）"),
            "out={out}"
        );
    }

    #[test]
    fn render_ready_tasks_max_three() {
        let mut input = empty_input();
        input.ready_tasks = (1..=5)
            .map(|i| TaskLine {
                id: i,
                title: format!("t{i}"),
                status: "ready".into(),
                assignee: Some(format!("w{i}")),
            })
            .collect();
        let out = render(&input);
        let section = out.split("## 明天會做的").nth(1).unwrap_or("");
        let ready_lines = section
            .lines()
            .filter(|l| l.starts_with("- task #"))
            .count();
        assert_eq!(ready_lines, 3, "out={out}");
        assert!(out.contains("- task #1 t1（w1）"));
        assert!(out.contains("- task #3 t3（w3）"));
        assert!(!out.contains("task #4"));
        assert!(!out.contains("task #5"));
    }

    #[test]
    fn parse_since_variants() {
        let now = Utc::now();
        let day_ago = now - Duration::hours(24);
        assert!(
            (parse_since(None, now).unwrap() - day_ago)
                .num_seconds()
                .abs()
                <= 1
        );

        assert!((now - parse_since(Some("36h"), now).unwrap()).num_hours() == 36);
        assert!((now - parse_since(Some("2d"), now).unwrap()).num_days() == 2);

        let fixed = DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            parse_since(Some("2026-09-01T00:00:00Z"), now).unwrap(),
            fixed
        );

        let err = parse_since(Some("soon"), now).unwrap_err().to_string();
        assert!(err.contains("invalid --since"), "err={err}");
        assert!(err.contains("soon"), "err={err}");
    }
}
