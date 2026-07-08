//! Real scars: analyzer input source that reads `review` / `friction` notes
//! from the workspace ledger, and emits High-severity lessons only when the
//! same problem shows up **twice or more** — havamal's "no scar, no doctrine"
//! rule made mechanical.
//!
//! Why: SELECTOR2 opened the two natural High paths (supersede / conflict),
//! but analyzer output is still generic self-help ("break the task down").
//! The ledger's highest-judgment content — human-written review findings and
//! friction notes — was never read. This module reads it, groups by problem
//! family, and only proposes candidates when a family recurs.
//!
//! Assumptions (queue 324 goal — operator can override at gate):
//! - **Same problem** = same tag family (`review` or `friction`) + normalized
//!   first-line prefix equal (case-fold, whitespace-collapse, strip trailing
//!   punctuation, prefix limit 80 chars).
//! - **Recurrence window** = 30 days from the most recent occurrence.
//! - **Per-run cap** stays at 3; real-scar lessons take priority slots ahead
//!   of statistical heuristic lessons.

use crate::analyzer::{Lesson, LessonSeverity};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

pub const RECURRENCE_WINDOW_DAYS: i64 = 30;
/// Family-key length: short enough to catch "same problem, different wording",
/// long enough to distinguish unrelated tickets. Bumped down from 80 after a
/// real friction pair collided at the semicolon (queue 324 test fixture).
/// `pub` since q328 SIGNCHECK so sign_check reuses the exact same family key
/// (single vocabulary, no rival contradiction concept).
pub const PREFIX_LEN: usize = 40;

/// One ledger note fed into the scar analyzer.
#[derive(Debug, Clone)]
pub struct LedgerNote {
    pub ts: String,
    pub tag: String,
    pub text: String,
}

/// Recurring-problem analyzer output — folded into the main postmortem lessons
/// as High severity (single-family cap: one lesson per recurring family).
pub fn analyze_recurring_scars(notes: &[LedgerNote], now: OffsetDateTime) -> Vec<Lesson> {
    let cutoff = now - Duration::days(RECURRENCE_WINDOW_DAYS);
    let mut by_family: HashMap<(String, String), Vec<&LedgerNote>> = HashMap::new();

    for note in notes {
        let Some(ts) = OffsetDateTime::parse(&note.ts, &Rfc3339).ok() else {
            continue;
        };
        if ts < cutoff {
            continue;
        }
        if !is_scar_tag(&note.tag) {
            continue;
        }
        let prefix = normalize_prefix(&note.text);
        if prefix.is_empty() {
            continue;
        }
        by_family
            .entry((note.tag.clone(), prefix))
            .or_default()
            .push(note);
    }

    let mut lessons = Vec::new();
    for ((tag, prefix), hits) in by_family {
        if hits.len() < 2 {
            continue;
        }
        // Stable-ish ordering: newest first for the referenced excerpt.
        let mut ordered = hits.clone();
        ordered.sort_by(|a, b| b.ts.cmp(&a.ts));
        let latest = &ordered[0];
        let excerpt = excerpt_of(&latest.text);
        let text = format!(
            "Recurring {tag} ({n}x): \"{excerpt}\". Second occurrence of the same problem — havamal rule says a repeated scar is doctrine material.",
            tag = tag,
            n = hits.len(),
            excerpt = excerpt,
        );
        lessons.push(Lesson {
            id: new_lesson_id(&tag, &prefix),
            text,
            severity: LessonSeverity::High,
            tags: vec![tag.clone(), "recurring".into()],
            source_trigger: format!("recurring_{tag}"),
        });
    }

    // Stable output order by lesson id (families otherwise iterate arbitrarily).
    lessons.sort_by(|a, b| a.id.cmp(&b.id));
    lessons
}

fn is_scar_tag(tag: &str) -> bool {
    matches!(tag, "review" | "friction")
}

/// Case-fold, whitespace-collapse, cut at first clause break (`;`, `—`,
/// full-stop or Chinese `。`), strip trailing punctuation, then take up to
/// PREFIX_LEN chars. Two writeups of the same scar rarely share a full
/// sentence, but the clause-head is stable.
pub fn normalize_prefix(text: &str) -> String {
    let first_line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let lower = first_line.to_lowercase();
    let collapsed: String = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    let clause: String = collapsed
        .chars()
        .take_while(|c| !matches!(c, ';' | '.' | '。' | '—' | '~'))
        .collect();
    let clause = clause.replace(" - ", " "); // ASCII dash between spaces = clause break too
    let stripped = clause
        .trim_end_matches(|c: char| c.is_ascii_punctuation() || c == '。' || c == '、')
        .trim()
        .to_string();
    stripped.chars().take(PREFIX_LEN).collect()
}

fn excerpt_of(text: &str) -> String {
    let first_line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let collapsed: String = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let limited: String = collapsed.chars().take(140).collect();
    limited.replace('"', "'")
}

fn new_lesson_id(tag: &str, prefix: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(tag.as_bytes());
    hasher.update(b"|");
    hasher.update(prefix.as_bytes());
    format!("lesson_scar_{}", hex::encode(hasher.finalize())[..12].to_string())
}

/// JSONL row shape used by `edda log --json` output (only the fields we need).
#[derive(Debug, Deserialize)]
struct LogRow {
    ts: String,
    payload: Option<LogPayload>,
}

#[derive(Debug, Deserialize)]
struct LogPayload {
    #[serde(default)]
    text: String,
    #[serde(default)]
    tags: Vec<String>,
}

/// Read notes from a `edda log --json`-style JSONL file (row-per-event).
/// Malformed lines skipped; missing file ⇒ empty. Kept as a testable path
/// alongside `notes_from_events` (which reads structured Events directly).
pub fn read_ledger_notes(path: &Path) -> Vec<LedgerNote> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<LogRow>(line) else {
            continue;
        };
        let Some(payload) = row.payload else { continue };
        for tag in &payload.tags {
            if is_scar_tag(tag) {
                out.push(LedgerNote {
                    ts: row.ts.clone(),
                    tag: tag.clone(),
                    text: payload.text.clone(),
                });
            }
        }
    }
    out
}

/// Extract scar notes from workspace-ledger note events.
/// One event carrying multiple scar tags fans out to multiple LedgerNotes
/// (a friction that also carries `review` counts as both — that's their real
/// classification, we don't second-guess the writer).
pub fn notes_from_events(events: &[edda_core::types::Event]) -> Vec<LedgerNote> {
    let mut out = Vec::new();
    for event in events {
        if event.event_type != "note" {
            continue;
        }
        let text = event
            .payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let tags = event
            .payload
            .get("tags")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for tag in tags {
            let Some(tag_str) = tag.as_str() else { continue };
            if is_scar_tag(tag_str) {
                out.push(LedgerNote {
                    ts: event.ts.clone(),
                    tag: tag_str.to_string(),
                    text: text.to_string(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(ts: &str, tag: &str, text: &str) -> LedgerNote {
        LedgerNote {
            ts: ts.into(),
            tag: tag.into(),
            text: text.into(),
        }
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-07-08T12:00:00Z", &Rfc3339).unwrap()
    }

    #[test]
    fn single_occurrence_yields_no_lesson() {
        let notes = vec![note(
            "2026-07-08T01:00:00Z",
            "friction",
            "worktree has no .edda, executor cannot inject context",
        )];
        assert!(analyze_recurring_scars(&notes, now()).is_empty(),
            "one hit is not a scar (havamal 二犯規矩)");
    }

    #[test]
    fn two_occurrences_same_prefix_yields_high_lesson_with_excerpt() {
        let notes = vec![
            note("2026-07-05T01:00:00Z", "friction",
                "Worktree has no .edda; executor cannot inject context on start"),
            note("2026-07-08T01:00:00Z", "friction",
                "worktree has no .edda — executor context broken again in nested session"),
        ];
        let lessons = analyze_recurring_scars(&notes, now());
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].severity, LessonSeverity::High);
        assert!(lessons[0].tags.contains(&"friction".to_string()));
        assert!(lessons[0].tags.contains(&"recurring".to_string()));
        assert!(lessons[0].text.contains("2x"), "count in text");
        assert!(lessons[0].text.to_lowercase().contains("worktree has no .edda"),
            "candidate cites the scar text verbatim (not template)");
    }

    #[test]
    fn out_of_window_occurrences_dont_count() {
        let notes = vec![
            note("2026-05-01T00:00:00Z", "friction", "old worktree issue in stale window"),
            note("2026-07-08T00:00:00Z", "friction", "old worktree issue back in fresh window"),
        ];
        assert!(analyze_recurring_scars(&notes, now()).is_empty(),
            "one in-window hit only (窗外的不算)");
    }

    #[test]
    fn non_scar_tags_ignored() {
        let notes = vec![
            note("2026-07-08T01:00:00Z", "session", "same prefix same day tag session"),
            note("2026-07-08T02:00:00Z", "session", "same prefix same day tag session"),
        ];
        assert!(analyze_recurring_scars(&notes, now()).is_empty(),
            "session tag is not a scar family");
    }

    #[test]
    fn prefix_normalization_matches_case_and_whitespace_variants() {
        let notes = vec![
            note("2026-07-08T01:00:00Z", "review",
                "  Findings: dispatch layer authored discipline text\n\nmore detail"),
            note("2026-07-08T02:00:00Z", "review",
                "FINDINGS:   dispatch  layer authored discipline text!"),
        ];
        let lessons = analyze_recurring_scars(&notes, now());
        assert_eq!(lessons.len(), 1, "case+whitespace+punct differences collapse to one family");
    }

    #[test]
    fn malformed_lines_and_missing_ts_skipped() {
        let notes = vec![
            note("garbage-ts", "friction", "hit a"),
            note("2026-07-08T01:00:00Z", "friction", "hit b"),
            note("2026-07-08T02:00:00Z", "friction", "hit b"),
        ];
        let lessons = analyze_recurring_scars(&notes, now());
        assert_eq!(lessons.len(), 1, "garbage-ts note dropped, valid pair remains");
    }
}
