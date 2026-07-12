//! Doctrine candidates: export postmortem findings into a havamal incubation
//! file for **human** promotion.
//!
//! Red line (tested, not commented): the machine never writes main doctrine.
//! This module only ever opens the single configured incubation file, and only
//! rewrites the managed block inside it — bytes outside the block are
//! preserved exactly. Candidates carry provenance (session, trigger, evidence)
//! and a dedup key so re-runs never duplicate. Promotion out of incubation is
//! always a human edit (havamal maintenance sweep).
//!
//! Opt-in: wiring reads `EDDA_INCUBATION_PATH`; unset ⇒ this module is never
//! called (zero behavior change — same shape as `EDDA_LLM_API_KEY`).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::analyzer::{LessonSeverity, PostMortemResult};

pub const CANDIDATES_SECTION_START: &str = "<!-- edda:candidates:start -->";
pub const CANDIDATES_SECTION_END: &str = "<!-- edda:candidates:end -->";

/// Minimum rule-proposal confidence to graduate into a doctrine candidate.
const MIN_PROPOSAL_CONFIDENCE: f64 = 0.7;
/// Hard cap of candidates appended per postmortem run (anti-flooding).
pub const MAX_CANDIDATES_PER_RUN: usize = 3;

/// A doctrine candidate bound for the incubation file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctrineCandidate {
    /// Provenance/dedup key: sha256(trigger + maxim), first 12 hex chars.
    pub key: String,
    pub name: String,
    pub maxim: String,
    pub session_id: String,
    pub trigger: String,
    pub evidence: Vec<String>,
    pub created_at: String,
}

/// Resolve the incubation path from the env value. `None`/empty ⇒ feature off.
/// Relative paths resolve against `cwd` (the session's repo root).
pub fn resolve_incubation_path(env_val: Option<&str>, cwd: &Path) -> Option<PathBuf> {
    let raw = env_val?.trim();
    if raw.is_empty() {
        return None;
    }
    let p = Path::new(raw);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    })
}

/// Pick at most `max` candidates from a postmortem result:
/// high-severity lessons first, then rule proposals with confidence ≥ 0.7.
pub fn select_candidates(result: &PostMortemResult, max: usize) -> Vec<DoctrineCandidate> {
    let mut out: Vec<DoctrineCandidate> = Vec::new();

    for lesson in &result.lessons {
        if out.len() >= max {
            break;
        }
        if lesson.severity != LessonSeverity::High {
            continue;
        }
        let key = provenance_key(&lesson.source_trigger, &lesson.text);
        out.push(DoctrineCandidate {
            name: format!("{} ({})", lesson.source_trigger, &key[..6]),
            maxim: lesson.text.clone(),
            session_id: result.session_id.clone(),
            trigger: lesson.source_trigger.clone(),
            evidence: if lesson.tags.is_empty() {
                vec!["(stats-level; event-id refs are v2)".to_string()]
            } else {
                lesson.tags.clone()
            },
            created_at: result.analyzed_at.clone(),
            key,
        });
    }

    for proposal in &result.rule_proposals {
        if out.len() >= max {
            break;
        }
        if proposal.confidence < MIN_PROPOSAL_CONFIDENCE {
            continue;
        }
        let maxim = format!("{} (when: {})", proposal.action, proposal.trigger);
        let key = provenance_key(&proposal.trigger, &proposal.action);
        out.push(DoctrineCandidate {
            name: format!("{} ({})", proposal.trigger, &key[..6]),
            maxim,
            session_id: result.session_id.clone(),
            trigger: proposal.trigger.clone(),
            evidence: proposal.evidence.clone(),
            created_at: result.analyzed_at.clone(),
            key,
        });
    }

    out
}

/// Append new candidates into the managed block of the incubation file.
///
/// - Creates the file (with a minimal header + block) when missing.
/// - Dedup: a candidate whose key already appears anywhere in the file is
///   skipped — re-runs are idempotent.
/// - When nothing new survives dedup, the file is not rewritten at all.
/// - Bytes outside the managed block are preserved exactly (red-line test).
///
/// Returns how many candidates were appended.
pub fn sync_candidates_to_incubation(
    path: &Path,
    candidates: &[DoctrineCandidate],
) -> io::Result<usize> {
    sync_candidates_to_incubation_with_hints(path, candidates, &[])
}

/// Same as [`sync_candidates_to_incubation`] but appends per-candidate
/// "Related in doctrine (machine hint, not judgment)" blocks derived from
/// `doctrine_entries` (see [`crate::sign_check`]). Passing an empty slice
/// reproduces the base behavior exactly — sign-check is opt-in, off by default.
pub fn sync_candidates_to_incubation_with_hints(
    path: &Path,
    candidates: &[DoctrineCandidate],
    doctrine_entries: &[crate::sign_check::DoctrineEntry],
) -> io::Result<usize> {
    let existing = match fs::read_to_string(path) {
        Ok(content) => Some(content),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };

    let fresh: Vec<&DoctrineCandidate> = candidates
        .iter()
        .filter(|c| {
            existing
                .as_deref()
                .map(|content| !content.contains(&format!("key: {}", c.key)))
                .unwrap_or(true)
        })
        .collect();

    if fresh.is_empty() {
        return Ok(0);
    }

    let rendered: String = fresh
        .iter()
        .map(|c| {
            let base = render_candidate(c);
            let hint = crate::sign_check::render_related_hint(&c.maxim, doctrine_entries);
            if hint.is_empty() {
                base
            } else {
                // Splice hint before the trailing newline of the base entry.
                let mut merged = base.trim_end_matches('\n').to_string();
                merged.push_str(&hint);
                merged.push('\n');
                merged
            }
        })
        .collect();

    let new_content = match existing {
        None => format!(
            "# Incubation (Candidates)\n\n\
             > Machine candidates below await human review. Promote, archive, or delete —\n\
             > edda never writes main doctrine; signing is always a human edit.\n\n\
             {start}\n{rendered}{end}\n",
            start = CANDIDATES_SECTION_START,
            end = CANDIDATES_SECTION_END,
            rendered = rendered,
        ),
        Some(content) => {
            match (
                content.find(CANDIDATES_SECTION_START),
                content.find(CANDIDATES_SECTION_END),
            ) {
                (Some(_), Some(end_pos)) => {
                    // Splice new entries right before the END marker; everything
                    // else — including prior entries inside the block — stays.
                    format!("{}{}{}", &content[..end_pos], rendered, &content[end_pos..])
                }
                _ => {
                    // No block yet: append one at the end; existing bytes untouched.
                    let sep = if content.ends_with('\n') { "" } else { "\n" };
                    format!(
                        "{content}{sep}\n{start}\n{rendered}{end}\n",
                        content = content,
                        sep = sep,
                        start = CANDIDATES_SECTION_START,
                        end = CANDIDATES_SECTION_END,
                        rendered = rendered,
                    )
                }
            }
        }
    };

    fs::write(path, new_content)?;
    Ok(fresh.len())
}

fn render_candidate(c: &DoctrineCandidate) -> String {
    let evidence = if c.evidence.is_empty() {
        "(stats-level; event-id refs are v2)".to_string()
    } else {
        c.evidence.join("; ")
    };
    format!(
        "\n## Candidate: {name}\n\
         <!-- key: {key} · session: {session} · trigger: {trigger} · at: {at} -->\n\n\
         - **Source:** edda postmortem — session `{session}`, trigger `{trigger}`; evidence: {evidence}\n\
         - **Why it may matter:** {maxim}\n\
         - **Not promoted because:** machine-extracted (deterministic heuristics) — awaiting human sign-off\n\
         - **Revisit after:** next maintenance sweep (30-day noise rule applies)\n",
        name = c.name,
        key = c.key,
        session = c.session_id,
        trigger = c.trigger,
        at = c.created_at,
        evidence = evidence,
        maxim = c.maxim,
    )
}

fn provenance_key(trigger: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(trigger.as_bytes());
    hasher.update(b"|");
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{Lesson, LessonSeverity, PostMortemResult, RuleProposal};
    use crate::rules::RuleCategory;
    use crate::trigger::TriggerReason;

    fn result_with(lessons: Vec<Lesson>, proposals: Vec<RuleProposal>) -> PostMortemResult {
        PostMortemResult {
            session_id: "sess-1".into(),
            triggers: vec![TriggerReason::SessionFailures],
            lessons,
            rule_proposals: proposals,
            analyzed_at: "2026-07-08T00:00:00Z".into(),
        }
    }

    fn lesson(text: &str, severity: LessonSeverity) -> Lesson {
        Lesson {
            id: "lesson_x".into(),
            text: text.into(),
            severity,
            tags: vec!["failure".into()],
            source_trigger: "session_failures".into(),
        }
    }

    fn proposal(action: &str, confidence: f64) -> RuleProposal {
        RuleProposal {
            trigger: "command_failure:npm".into(),
            action: action.into(),
            anchor_file: None,
            category: RuleCategory::Workflow,
            confidence,
            evidence: vec!["Failed command: npm test".into()],
        }
    }

    #[test]
    fn resolve_path_none_or_empty_is_off() {
        let cwd = Path::new("/repo");
        assert_eq!(resolve_incubation_path(None, cwd), None);
        assert_eq!(resolve_incubation_path(Some(""), cwd), None);
        assert_eq!(resolve_incubation_path(Some("   "), cwd), None);
    }

    #[test]
    fn resolve_path_relative_joins_cwd_absolute_kept() {
        let cwd = Path::new("/repo");
        assert_eq!(
            resolve_incubation_path(Some("docs/incubation.md"), cwd),
            Some(PathBuf::from("/repo").join("docs/incubation.md"))
        );
        let abs = if cfg!(windows) {
            "C:\\x\\inc.md"
        } else {
            "/x/inc.md"
        };
        assert_eq!(
            resolve_incubation_path(Some(abs), cwd),
            Some(PathBuf::from(abs))
        );
    }

    #[test]
    fn select_takes_high_lessons_and_confident_proposals_capped() {
        let result = result_with(
            vec![
                lesson("high one", LessonSeverity::High),
                lesson("low one", LessonSeverity::Low),
                lesson("high two", LessonSeverity::High),
            ],
            vec![proposal("confident", 0.8), proposal("weak", 0.5)],
        );
        let picked = select_candidates(&result, MAX_CANDIDATES_PER_RUN);
        assert_eq!(picked.len(), 3);
        assert!(picked[0].maxim.contains("high one"));
        assert!(picked[1].maxim.contains("high two"));
        assert!(picked[2].maxim.contains("confident"));
        // deterministic keys: same input, same key
        let again = select_candidates(&result, MAX_CANDIDATES_PER_RUN);
        assert_eq!(picked[0].key, again[0].key);
    }

    #[test]
    fn select_respects_hard_cap() {
        let result = result_with(
            (0..5)
                .map(|i| lesson(&format!("high {i}"), LessonSeverity::High))
                .collect(),
            vec![],
        );
        assert_eq!(select_candidates(&result, 3).len(), 3);
    }

    #[test]
    fn sync_creates_file_with_block_and_four_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incubation.md");
        let result = result_with(
            vec![lesson("watch the baseline", LessonSeverity::High)],
            vec![],
        );
        let cands = select_candidates(&result, 3);

        let n = sync_candidates_to_incubation(&path, &cands).unwrap();
        assert_eq!(n, 1);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains(CANDIDATES_SECTION_START));
        assert!(content.contains(CANDIDATES_SECTION_END));
        assert!(content.contains("## Candidate:"));
        assert!(content.contains("**Source:**"));
        assert!(content.contains("**Why it may matter:** watch the baseline"));
        assert!(content.contains("**Not promoted because:**"));
        assert!(content.contains("**Revisit after:**"));
        assert!(content.contains("session `sess-1`"));
    }

    #[test]
    fn sync_is_idempotent_no_rewrite_on_second_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incubation.md");
        let result = result_with(vec![lesson("only once", LessonSeverity::High)], vec![]);
        let cands = select_candidates(&result, 3);

        assert_eq!(sync_candidates_to_incubation(&path, &cands).unwrap(), 1);
        let first = fs::read_to_string(&path).unwrap();
        assert_eq!(sync_candidates_to_incubation(&path, &cands).unwrap(), 0);
        let second = fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "no-op run must not rewrite the file");
    }

    #[test]
    fn sync_preserves_bytes_outside_managed_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incubation.md");
        let prefix = "# My Incubation\n\nhand-written intro — do not touch\n\n";
        let block = format!("{}\n{}\n", CANDIDATES_SECTION_START, CANDIDATES_SECTION_END);
        let suffix = "\n## Recently promoted\n\n- 2026-07-01 — old note stays\n";
        fs::write(&path, format!("{prefix}{block}{suffix}")).unwrap();

        let result = result_with(vec![lesson("spliced in", LessonSeverity::High)], vec![]);
        let n = sync_candidates_to_incubation(&path, &select_candidates(&result, 3)).unwrap();
        assert_eq!(n, 1);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(prefix), "bytes before block unchanged");
        assert!(content.ends_with(suffix), "bytes after block unchanged");
        assert!(content.contains("spliced in"));
        let start = content.find(CANDIDATES_SECTION_START).unwrap();
        let end = content.find(CANDIDATES_SECTION_END).unwrap();
        let inside = &content[start..end];
        assert!(
            inside.contains("## Candidate:"),
            "entry landed inside block"
        );
    }

    #[test]
    fn sync_with_empty_hints_matches_base_behavior_byte_for_byte() {
        use tempfile::tempdir;
        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();
        let result = result_with(vec![lesson("shared maxim", LessonSeverity::High)], vec![]);
        let cands = select_candidates(&result, 3);

        // Path 1: base API.
        sync_candidates_to_incubation(&dir_a.path().join("i.md"), &cands).unwrap();
        // Path 2: hints API with empty slice.
        sync_candidates_to_incubation_with_hints(&dir_b.path().join("i.md"), &cands, &[]).unwrap();

        let a = fs::read_to_string(dir_a.path().join("i.md")).unwrap();
        let b = fs::read_to_string(dir_b.path().join("i.md")).unwrap();
        assert_eq!(
            a, b,
            "empty hint slice = zero behavior change (opt-in guard)"
        );
    }

    #[test]
    fn sync_with_matching_hint_appends_related_block_inside_candidate() {
        use crate::sign_check::parse_entries;
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let path = dir.path().join("incubation.md");
        // Candidate maxim contains "shared family" — align a doctrine entry to it.
        let entries = parse_entries("### FM-9: shared family topic\nbody\n", "failure-memory.md");
        let result = result_with(
            vec![lesson(
                "shared family topic (surfaced)",
                LessonSeverity::High,
            )],
            vec![],
        );
        let cands = select_candidates(&result, 3);
        sync_candidates_to_incubation_with_hints(&path, &cands, &entries).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("Related in doctrine"),
            "hint block present"
        );
        assert!(content.contains("FM-9"), "specific matching heading named");
        assert!(
            content.contains("not judgment"),
            "machine-hint red-line phrasing kept"
        );
    }

    #[test]
    fn sync_appends_block_when_file_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incubation.md");
        let original = "# Existing incubation without block\n";
        fs::write(&path, original).unwrap();

        let result = result_with(vec![lesson("new block", LessonSeverity::High)], vec![]);
        sync_candidates_to_incubation(&path, &select_candidates(&result, 3)).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(original), "existing bytes untouched");
        assert!(content.contains(CANDIDATES_SECTION_START));
    }
}
