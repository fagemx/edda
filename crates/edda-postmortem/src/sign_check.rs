//! Sign-check: surface doctrine entries that share a family with a machine
//! candidate — so at sign time the operator sees related existing scars
//! *next to* the candidate, and can judge for themselves whether it duplicates
//! or contradicts what's already in doctrine.
//!
//! Vocabulary alignment (Foundry q328 review verdict): reuses
//! [`scars::normalize_prefix`] and [`scars::PREFIX_LEN`] verbatim. We do NOT
//! introduce a rival "contradiction" concept — same `by_label`/family key,
//! same rules. The machine only hints; the human judges.
//!
//! Contract:
//! - Read failure-memory.md, layer-1-ideology.md, layer-6-heart-methods.md
//!   from a doctrine dir; parse `### <heading>` entries.
//! - Compute family key for each entry (first line after heading).
//! - For a candidate maxim, compute its family key and find entries with the
//!   same key.
//! - Render a "Related in doctrine (machine hint, not judgment)" markdown
//!   block, or empty string when no related entries exist.
//!
//! Opt-in: this module is only invoked when both `EDDA_INCUBATION_PATH` and
//! `EDDA_DOCTRINE_PATH` are set. Unset ⇒ zero behavior change (candidate
//! render stays exactly as SELECTOR3 shipped it).

use crate::scars::normalize_prefix;
use std::fs;
use std::path::{Path, PathBuf};

/// Doctrine files scanned for related entries. Ordered by scan priority.
pub const DOCTRINE_FILES: &[&str] = &[
    "failure-memory.md",
    "layer-1-ideology.md",
    "layer-6-heart-methods.md",
];

/// A single doctrine entry parsed from a doctrine markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctrineEntry {
    pub file: String,
    pub heading: String,
    pub first_line: String,
    pub family_key: String,
}

/// Parse `### <heading>` entries from a doctrine file's markdown.
/// Family key is computed from the **heading** (with any `FM-N:` / `6.N:` / `L1.N:`
/// numeric prefix stripped) — humans identify a scar by its heading, not its body,
/// and candidate maxims (from `Lesson.text`) are one-line summaries that align
/// with headings, not with bullet bodies.
/// `first_line` is stored for display context only; it does not drive matching.
pub fn parse_entries(body: &str, file_label: &str) -> Vec<DoctrineEntry> {
    let mut out = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(heading) = line.strip_prefix("### ") {
            // Find first non-empty non-heading line after the heading (for display).
            let mut j = i + 1;
            let mut first_line = String::new();
            while j < lines.len() {
                let candidate = lines[j].trim();
                if candidate.is_empty() {
                    j += 1;
                    continue;
                }
                if candidate.starts_with('#') {
                    break;
                }
                let cleaned = candidate
                    .trim_start_matches(|c: char| matches!(c, '-' | '*' | '>' | ' '))
                    .to_string();
                first_line = cleaned;
                break;
            }
            let heading_trimmed = heading.trim().to_string();
            let family_key = normalize_prefix(&strip_entry_number(&heading_trimmed));
            if !family_key.is_empty() {
                out.push(DoctrineEntry {
                    file: file_label.to_string(),
                    heading: heading_trimmed,
                    first_line,
                    family_key,
                });
            }
        }
        i += 1;
    }
    out
}

/// Strip a leading entry number like `FM-5:`, `6.3:`, `L1.2:` from a heading,
/// so the family key is derived from the maxim itself, not the numbering scheme.
fn strip_entry_number(heading: &str) -> String {
    let colon = match heading.find(':') {
        Some(i) => i,
        None => return heading.to_string(),
    };
    let prefix = &heading[..colon];
    // Prefix is an entry number if it's short and consists only of alphanumerics,
    // dots, and dashes (no whitespace) — matches FM-5, 6.3, L1.2, etc.
    if prefix.len() <= 8
        && !prefix.is_empty()
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        heading[colon + 1..].trim().to_string()
    } else {
        heading.to_string()
    }
}

/// Load all parseable entries from a doctrine directory.
/// Missing directory or missing files are silently skipped (best-effort).
pub fn load_doctrine_entries(doctrine_dir: &Path) -> Vec<DoctrineEntry> {
    let root = resolve_doctrine_root(doctrine_dir);
    let mut out = Vec::new();
    for name in DOCTRINE_FILES {
        let path = root.join(name);
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        out.extend(parse_entries(&body, name));
    }
    out
}

/// Mirror `havamal check`'s `resolveDoctrineDir`: prefer `<dir>/references`
/// when a state-snapshot.md lives there (havamal convention), else `<dir>`.
fn resolve_doctrine_root(doctrine_dir: &Path) -> PathBuf {
    let refs = doctrine_dir.join("references");
    if refs.join("state-snapshot.md").exists() {
        refs
    } else {
        doctrine_dir.to_path_buf()
    }
}

/// Minimum overlap length before two family keys are considered related.
/// Guards against the `""` empty key matching everything and single-word noise.
const MIN_FAMILY_OVERLAP: usize = 12;

/// Find entries whose family key relates to the candidate's.
/// Vocabulary alignment: family key = `normalize_prefix(...)`, same function
/// scars.rs uses. Match rule: keys equal OR one is a prefix of the other
/// (candidate maxims often carry trailing qualifiers like "(machine-surfaced)"
/// that make them longer than the canonical heading; either direction is OK).
pub fn related_entries<'a>(
    candidate_maxim: &str,
    entries: &'a [DoctrineEntry],
) -> Vec<&'a DoctrineEntry> {
    let key = normalize_prefix(candidate_maxim);
    if key.is_empty() {
        return Vec::new();
    }
    entries
        .iter()
        .filter(|e| family_keys_related(&key, &e.family_key))
        .collect()
}

fn family_keys_related(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let (shorter, longer) = if a.len() < b.len() { (a, b) } else { (b, a) };
    // Prefix-relation match with minimum-overlap guard so "the" doesn't match everything.
    shorter.len() >= MIN_FAMILY_OVERLAP && longer.starts_with(shorter)
}

/// Render a markdown hint block ("Related in doctrine (machine hint, not
/// judgment)") for a candidate, or empty string when no related entries.
pub fn render_related_hint(candidate_maxim: &str, entries: &[DoctrineEntry]) -> String {
    let related = related_entries(candidate_maxim, entries);
    if related.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n- **Related in doctrine (machine hint, not judgment)**: same family key as—\n",
    );
    for entry in related {
        out.push_str(&format!("  - `{}` → {}\n", entry.file, entry.heading));
    }
    out.push_str("  (machine surfaces same-family entries; the operator judges duplicate/contradict/refine at sign time.)\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_entries_extracts_heading_and_first_line() {
        let body = "\
# Failure Memory\n\
\n\
### FM-1: watch out for silent walls\n\
- **Temptation:** retrying without new signal.\n\
- **How it failed:** three silent failures in a row.\n\
\n\
### FM-2: unrelated ticket\n\
- Just a note.\n\
";
        let entries = parse_entries(body, "failure-memory.md");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].heading, "FM-1: watch out for silent walls");
        assert!(entries[0].first_line.contains("retrying without new signal"));
    }

    #[test]
    fn related_entries_matches_family_key_of_candidate() {
        // Family key comes from heading (FM-N: prefix stripped), matching how
        // candidate maxims read (Lesson.text is a one-liner, not a bullet body).
        let entries = parse_entries(
            "### FM-5: Retrying into a silent wall\n**Temptation:** ...\n",
            "failure-memory.md",
        );
        let related = related_entries("Retrying into a silent wall (machine-surfaced)", &entries);
        assert_eq!(related.len(), 1);
        assert!(related[0].heading.contains("Retrying"));
    }

    #[test]
    fn unrelated_candidate_returns_empty_hint() {
        let entries = parse_entries(
            "### FM-5: Retrying into a silent wall\n**Temptation:** ...\n",
            "failure-memory.md",
        );
        let hint = render_related_hint("A completely different maxim about caching", &entries);
        assert_eq!(hint, "");
    }

    #[test]
    fn related_candidate_renders_hint_block_naming_files() {
        let entries = parse_entries(
            "### FM-5: Retrying into a silent wall\n**Temptation:** ...\n",
            "failure-memory.md",
        );
        let hint = render_related_hint("Retrying into a silent wall", &entries);
        assert!(hint.contains("Related in doctrine"));
        assert!(hint.contains("failure-memory.md"));
        assert!(hint.contains("Retrying"));
        // Machine-hint-not-judgment framing MUST be present (red-line phrasing).
        assert!(hint.contains("not judgment"));
        assert!(hint.contains("operator judges"));
    }

    #[test]
    fn strip_entry_number_handles_common_forms() {
        assert_eq!(strip_entry_number("FM-5: Retrying"), "Retrying");
        assert_eq!(strip_entry_number("6.3: Paid lesson"), "Paid lesson");
        assert_eq!(strip_entry_number("L1.2: The belief"), "The belief");
        // Not a number: keep as-is (defensive — arbitrary long prefixes stay).
        assert_eq!(strip_entry_number("Something: with colon"), "Something: with colon");
        // No colon: keep as-is.
        assert_eq!(strip_entry_number("No colon here"), "No colon here");
    }

    #[test]
    fn load_doctrine_entries_handles_missing_dir_gracefully() {
        let dir = tempdir().unwrap();
        // No files — should return empty, not error.
        let entries = load_doctrine_entries(dir.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn load_doctrine_entries_reads_files_from_references_subdir() {
        let dir = tempdir().unwrap();
        let refs = dir.path().join("references");
        fs::create_dir_all(&refs).unwrap();
        fs::write(refs.join("state-snapshot.md"), "# State").unwrap();
        fs::write(
            refs.join("failure-memory.md"),
            "### FM-1: sample\n- **Temptation:** the fix looked easy.\n",
        )
        .unwrap();
        let entries = load_doctrine_entries(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file, "failure-memory.md");
    }

    #[test]
    fn vocabulary_alignment_family_key_uses_normalize_prefix() {
        // Two texts that scars.rs normalize_prefix collapses to the same key
        // MUST be treated as the same family by sign_check — no rival concept.
        // Case + whitespace variation (safe: no clause-break chars).
        let a = "Retrying   Into a Silent WALL";
        let b = "retrying into a silent wall";
        assert_eq!(normalize_prefix(a), normalize_prefix(b),
            "scars.rs normalize_prefix is the single family key vocabulary");
        let entries = parse_entries(
            &format!("### FM-x: {a}\nbody\n"),
            "failure-memory.md",
        );
        let related = related_entries(b, &entries);
        assert_eq!(related.len(), 1, "case/whitespace-normalized keys collide as scars.rs guarantees");
    }
}
