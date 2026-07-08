//! Decision signals: timestamped supersede / binding-conflict markers recorded
//! at `edda decide` time, counted back by session window at postmortem time.
//!
//! Why: the two natural High-severity postmortem paths (decision reversal,
//! multi-agent conflict) starved because their inputs were hardcoded to
//! `0`/`false` at the SessionEnd wiring (flywheel drill 1 finding). The CLI
//! already *detects* both cases when a decide lands — this module just gives
//! those detections a durable, windowable trace.
//!
//! Storage: `<project state>/decision_signals.jsonl`, append-only, one JSON
//! object per line: `{"ts": "<rfc3339>", "kind": "superseded"|"binding_conflict",
//! "key": "<decision key>"}`. Recording is best-effort — a failed append must
//! never block a decide.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// What the decide path observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    /// A prior decision for the same key (different value) was superseded.
    Superseded,
    /// A peer holds a conflicting binding for the same key.
    BindingConflict,
}

impl SignalKind {
    fn as_str(self) -> &'static str {
        match self {
            SignalKind::Superseded => "superseded",
            SignalKind::BindingConflict => "binding_conflict",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SignalLine {
    ts: String,
    kind: String,
    key: String,
}

/// Windowed counts consumed by the postmortem wiring.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SignalCounts {
    pub superseded: u64,
    pub conflicts: u64,
}

/// Signals file for a project (lives next to lessons/rules state).
pub fn signals_path(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join("decision_signals.jsonl")
}

/// Append one signal for `project_id` (creates parent dirs on first use).
pub fn record_decision_signal(project_id: &str, kind: SignalKind, key: &str) -> io::Result<()> {
    record_signal_at(&signals_path(project_id), kind, key)
}

/// Append one signal to an explicit file path (testable core).
pub fn record_signal_at(path: &Path, kind: SignalKind, key: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = SignalLine {
        ts: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default(),
        kind: kind.as_str().to_string(),
        key: key.to_string(),
    };
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{}", serde_json::to_string(&line)?)?;
    Ok(())
}

/// Count signals for `project_id` whose ts falls inside `[first_ts, last_ts]`.
/// `None` bounds are open-ended. Missing file ⇒ zeros; malformed lines skipped.
pub fn count_signals_between(
    project_id: &str,
    first_ts: Option<&str>,
    last_ts: Option<&str>,
) -> SignalCounts {
    count_signals_at(&signals_path(project_id), first_ts, last_ts)
}

/// Windowed count against an explicit file path (testable core).
pub fn count_signals_at(
    path: &Path,
    first_ts: Option<&str>,
    last_ts: Option<&str>,
) -> SignalCounts {
    let mut counts = SignalCounts::default();
    let Ok(content) = fs::read_to_string(path) else {
        return counts;
    };
    let lo = first_ts.and_then(parse_ts);
    let hi = last_ts.and_then(parse_ts);

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(sig) = serde_json::from_str::<SignalLine>(line) else {
            continue;
        };
        let Some(ts) = parse_ts(&sig.ts) else {
            continue;
        };
        if lo.map(|l| ts < l).unwrap_or(false) || hi.map(|h| ts > h).unwrap_or(false) {
            continue;
        }
        match sig.kind.as_str() {
            "superseded" => counts.superseded += 1,
            "binding_conflict" => counts.conflicts += 1,
            _ => {}
        }
    }
    counts
}

fn parse_ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_lines(path: &Path, lines: &[&str]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, lines.join("\n")).unwrap();
    }

    #[test]
    fn record_then_count_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("decision_signals.jsonl");
        record_signal_at(&path, SignalKind::Superseded, "db.engine").unwrap();
        record_signal_at(&path, SignalKind::BindingConflict, "db.engine").unwrap();
        record_signal_at(&path, SignalKind::Superseded, "auth.method").unwrap();

        let counts = count_signals_at(&path, None, None);
        assert_eq!(counts.superseded, 2);
        assert_eq!(counts.conflicts, 1);
    }

    #[test]
    fn missing_file_counts_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.jsonl");
        assert_eq!(count_signals_at(&path, None, None), SignalCounts::default());
    }

    #[test]
    fn window_filters_outside_signals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sig.jsonl");
        write_lines(
            &path,
            &[
                r#"{"ts":"2026-07-08T01:00:00Z","kind":"superseded","key":"a"}"#,
                r#"{"ts":"2026-07-08T02:30:00Z","kind":"superseded","key":"b"}"#,
                r#"{"ts":"2026-07-08T02:45:00.5Z","kind":"binding_conflict","key":"c"}"#,
                r#"{"ts":"2026-07-08T04:00:00Z","kind":"superseded","key":"d"}"#,
            ],
        );
        let counts = count_signals_at(
            &path,
            Some("2026-07-08T02:00:00Z"),
            Some("2026-07-08T03:00:00Z"),
        );
        assert_eq!(counts.superseded, 1, "only the in-window supersede");
        assert_eq!(counts.conflicts, 1, "fractional-second ts inside window");

        let open_start = count_signals_at(&path, None, Some("2026-07-08T03:00:00Z"));
        assert_eq!(open_start.superseded, 2);
    }

    #[test]
    fn malformed_and_unknown_lines_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sig.jsonl");
        write_lines(
            &path,
            &[
                "not json",
                r#"{"ts":"garbage","kind":"superseded","key":"a"}"#,
                r#"{"ts":"2026-07-08T01:00:00Z","kind":"weird","key":"a"}"#,
                r#"{"ts":"2026-07-08T01:00:00Z","kind":"superseded","key":"a"}"#,
                "",
            ],
        );
        let counts = count_signals_at(&path, None, None);
        assert_eq!(counts.superseded, 1);
        assert_eq!(counts.conflicts, 0);
    }
}
