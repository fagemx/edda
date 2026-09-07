//! GH-1015: renderers for `edda fleet order`.
//!
//! Three shapes over the same [`Queue`]: a text report, the board-comment
//! markdown that replaces the hand-written #932 plan, and (in `cli`) JSON.
//! Every one of them prints the score decomposition, because a bare rank the
//! operator cannot check is a rank they cannot veto.

use super::{Queue, Row, Score, Status};

/// Every non-zero score component, in the order [`Score`] declares them.
pub fn decomposition(score: &Score) -> String {
    let parts = [
        ("class", score.class),
        ("ready", score.readiness),
        ("priority", score.priority),
        ("red-run blocker", score.blocker),
        ("collision", score.collision),
        ("stale", score.freshness),
        ("frozen", score.frozen),
    ];
    let rendered: Vec<String> = parts
        .iter()
        .filter(|(_, value)| *value != 0)
        .map(|(name, value)| format!("{name} {value:+}"))
        .collect();
    if rendered.is_empty() {
        "0".to_string()
    } else {
        rendered.join(", ")
    }
}

/// Everything about a row that its rank, class, lane and score do not say.
pub fn notes(row: &Row) -> String {
    let mut notes = Vec::new();
    if let Some(reason) = &row.hold_reason {
        notes.push(reason.clone());
    }
    if row.status == Status::Frozen {
        notes.push("mechanism dispatch frozen (health RED, no red-run evidence)".to_string());
    }
    for finding in &row.freshness {
        notes.push(format!(
            "{} {} {}: {}",
            finding.verdict.as_str().to_uppercase(),
            finding.kind,
            finding.target,
            finding.note
        ));
    }
    if !row.pending_checks.is_empty() {
        notes.push(format!("pending checks: {}", row.pending_checks.join(", ")));
    }
    notes.join("; ")
}

/// Board-comment markdown, shaped like the hand-written #932 plan it replaces.
pub fn render_markdown(queue: &Queue) -> String {
    let mut out = format!(
        "### Fleet order — {} (health {}, mechanism dispatch {})\n\n",
        queue.generated_at, queue.health_status, queue.mechanism_dispatch
    );
    out.push_str(&format!(
        "{} open issues; flash lane cap {} surface files ({}). Ranked by score \
         descending, ties by issue number.\n\n",
        queue.total_issues, queue.flash_max_surface_files, queue.flash_cap_source
    ));
    out.push_str("| # | issue | score | class | lane | status | score components | notes |\n");
    out.push_str("|--:|-------|------:|-------|------|--------|------------------|-------|\n");
    for row in &queue.rows {
        out.push_str(&format!(
            "| {} | #{} | {} | {} | {} | {} | {} | {} |\n",
            row.rank,
            row.number,
            row.score.total,
            row.class,
            row.lane.as_str(),
            row.status.as_str(),
            decomposition(&row.score),
            escape_cell(&notes(row))
        ));
    }
    out
}

/// A pipe inside a cell would split the column; nothing else in these strings
/// is markdown-active.
fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// The default text report.
pub fn render_text(queue: &Queue) -> String {
    let mut out = format!(
        "fleet order — {} open issues | health {} (mechanism dispatch {}) | flash cap {} files ({})\n",
        queue.total_issues,
        queue.health_status,
        queue.mechanism_dispatch,
        queue.flash_max_surface_files,
        queue.flash_cap_source
    );
    out.push_str("rank  issue  score  class      lane        status    detail\n");
    for row in &queue.rows {
        let note = notes(row);
        let detail = if note.is_empty() {
            decomposition(&row.score)
        } else {
            format!("{} — {note}", decomposition(&row.score))
        };
        out.push_str(&format!(
            "{:>4}  {:>5}  {:>5}  {:<9}  {:<10}  {:<8}  {detail}\n",
            row.rank,
            format!("#{}", row.number),
            row.score.total,
            row.class,
            row.lane.as_str(),
            row.status.as_str()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd_fleet_order::{Lane, Score};

    fn score() -> Score {
        Score {
            class: 40,
            readiness: 25,
            priority: 15,
            blocker: 0,
            collision: -10,
            freshness: 0,
            frozen: 0,
            total: 70,
        }
    }

    fn row() -> Row {
        Row {
            rank: 1,
            number: 671,
            title: "committed mirror".to_string(),
            class: "product".to_string(),
            surface: vec!["crates/edda-cli/src/main.rs".to_string()],
            lane: Lane::Flash,
            lane_reasons: vec!["surface 1 files <= flash cap 3, no scripts/ path".to_string()],
            pending_checks: vec!["brief-render".to_string()],
            status: Status::Ready,
            hold_reason: None,
            collides_with: vec![685],
            freshness: Vec::new(),
            score: score(),
        }
    }

    fn queue() -> Queue {
        Queue {
            generated_at: "2026-09-07T12:00:00Z".to_string(),
            health_status: "GREEN".to_string(),
            mechanism_dispatch: "open".to_string(),
            flash_max_surface_files: 3,
            flash_cap_source: "default".to_string(),
            total_issues: 1,
            rows: vec![row()],
        }
    }

    #[test]
    fn decomposition_shows_signed_non_zero_components_only() {
        assert_eq!(
            decomposition(&score()),
            "class +40, ready +25, priority +15, collision -10"
        );
        let zeroed = Score {
            class: 0,
            readiness: 0,
            priority: 0,
            blocker: 0,
            collision: 0,
            freshness: 0,
            frozen: 0,
            total: 0,
        };
        assert_eq!(decomposition(&zeroed), "0");
    }

    #[test]
    fn markdown_is_a_pasteable_board_table() {
        let rendered = render_markdown(&queue());
        assert!(rendered.starts_with(
            "### Fleet order — 2026-09-07T12:00:00Z (health GREEN, mechanism dispatch open)"
        ));
        assert!(rendered.contains(
            "| 1 | #671 | 70 | product | flash | ready | class +40, ready +25, priority +15, \
             collision -10 | pending checks: brief-render |"
        ));
        // Every table row has the same cell count as the header.
        let pipes = |line: &str| line.matches('|').count();
        let header = rendered
            .lines()
            .find(|l| l.starts_with("| # |"))
            .expect("header row");
        for line in rendered.lines().filter(|l| l.starts_with("| 1 ")) {
            assert_eq!(pipes(line), pipes(header));
        }
    }

    #[test]
    fn a_pipe_in_a_note_cannot_split_the_column() {
        let mut queue = queue();
        queue.rows[0].hold_reason = Some("blocked by a | b".to_string());
        let rendered = render_markdown(&queue);
        assert!(rendered.contains("blocked by a \\| b"));
        let header = rendered
            .lines()
            .find(|l| l.starts_with("| # |"))
            .expect("header row");
        let row = rendered
            .lines()
            .find(|l| l.starts_with("| 1 "))
            .expect("data row");
        assert_eq!(
            row.matches('|').count() - row.matches("\\|").count(),
            header.matches('|').count()
        );
    }

    #[test]
    fn text_report_carries_the_header_and_one_line_per_row() {
        let rendered = render_text(&queue());
        assert!(rendered.starts_with(
            "fleet order — 1 open issues | health GREEN (mechanism dispatch open) | \
             flash cap 3 files (default)\n"
        ));
        assert_eq!(rendered.lines().count(), 3);
        assert!(rendered.lines().nth(2).expect("row").contains("#671"));
    }
}
