use globset::Glob;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Pattern {
    pub id: String,
    pub trigger: PatternTrigger,
    pub rule: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub examples: Option<PatternExamples>,
    #[serde(default)]
    pub metadata: PatternMetadata,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PatternTrigger {
    pub file_glob: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PatternExamples {
    pub bad: Option<String>,
    pub good: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PatternMetadata {
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub hit_count: u64,
    #[serde(default)]
    pub last_triggered: Option<String>,
    #[serde(default = "default_active")]
    pub status: String,
}

fn default_active() -> String {
    "active".to_string()
}

/// Load all active patterns from `.edda/patterns/*.json`.
/// Skips files starting with `_` (reserved for index/metadata).
/// Returns empty vec if directory doesn't exist.
pub fn load_patterns(patterns_dir: &Path) -> Vec<Pattern> {
    let entries = match fs::read_dir(patterns_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    let mut patterns = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('_') {
                continue;
            }
        }
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(pat) = serde_json::from_str::<Pattern>(&content) {
                if pat.metadata.status == "active" {
                    patterns.push(pat);
                }
            }
        }
    }
    patterns
}

/// Match patterns against a file path. Returns matched patterns.
pub fn match_patterns<'a>(patterns: &'a [Pattern], file_path: &str) -> Vec<&'a Pattern> {
    // Normalize path separators for cross-platform matching
    let normalized = file_path.replace('\\', "/");

    patterns
        .iter()
        .filter(|pat| {
            pat.trigger.file_glob.iter().any(|glob_str| {
                Glob::new(glob_str)
                    .ok()
                    .map(|g| {
                        let matcher = g.compile_matcher();
                        if matcher.is_match(&normalized) {
                            return true;
                        }
                        // Also try matching against just the file name
                        let file_name = Path::new(&normalized)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        matcher.is_match(file_name)
                    })
                    .unwrap_or(false)
            })
        })
        .collect()
}

/// Render matched patterns as markdown for additionalContext injection.
/// Respects budget_chars limit.
pub fn render_pattern_context(
    matched: &[&Pattern],
    _file_path: &str,
    budget_chars: usize,
) -> Option<String> {
    if matched.is_empty() {
        return None;
    }

    let mut output = String::from("## Pattern Reminders\n\n");

    for pat in matched {
        let mut entry = format!("**{}**: {}", pat.id, pat.rule);
        if !pat.source.is_empty() {
            entry.push_str(&format!(" _(source: {})_", pat.source));
        }
        entry.push('\n');

        if let Some(ref examples) = pat.examples {
            if let Some(ref bad) = examples.bad {
                entry.push_str(&format!("  - Bad: {}\n", bad));
            }
            if let Some(ref good) = examples.good {
                entry.push_str(&format!("  - Good: {}\n", good));
            }
        }
        entry.push('\n');

        if output.len() + entry.len() > budget_chars {
            break;
        }
        output.push_str(&entry);
    }

    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pattern(id: &str, globs: &[&str], rule: &str) -> Pattern {
        Pattern {
            id: id.to_string(),
            trigger: PatternTrigger {
                file_glob: globs.iter().map(|s| s.to_string()).collect(),
                keywords: vec![],
            },
            rule: rule.to_string(),
            source: String::new(),
            examples: None,
            metadata: PatternMetadata {
                created_at: String::new(),
                hit_count: 0,
                last_triggered: None,
                status: "active".to_string(),
            },
        }
    }

    #[test]
    fn match_glob_test_files() {
        let patterns = vec![sample_pattern(
            "test-no-db",
            &["**/*.test.*"],
            "no direct DB",
        )];
        let matched = match_patterns(&patterns, "src/foo.test.ts");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "test-no-db");
    }

    #[test]
    fn no_match_non_test_files() {
        let patterns = vec![sample_pattern(
            "test-no-db",
            &["**/*.test.*"],
            "no direct DB",
        )];
        let matched = match_patterns(&patterns, "src/foo.ts");
        assert!(matched.is_empty());
    }

    #[test]
    fn render_respects_budget() {
        let p1 = sample_pattern("p1", &["**/*"], "rule 1");
        let p2 = sample_pattern("p2", &["**/*"], "rule 2");
        let patterns = vec![&p1, &p2];
        // Very small budget — should only fit header + p1
        let result = render_pattern_context(&patterns, "foo.rs", 60);
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("p1"));
    }

    #[test]
    fn load_skips_underscore_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Write a normal pattern
        let pat = sample_pattern("test1", &["**/*.test.*"], "rule");
        std::fs::write(
            dir.join("test1.json"),
            serde_json::to_string_pretty(&pat).unwrap(),
        )
        .unwrap();
        // Write an _index.json (should be skipped)
        std::fs::write(dir.join("_index.json"), "{}").unwrap();

        let loaded = load_patterns(dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "test1");
    }

    #[test]
    fn load_from_nonexistent_dir() {
        let loaded = load_patterns(Path::new("/nonexistent/patterns"));
        assert!(loaded.is_empty());
    }

    #[test]
    fn match_windows_path() {
        let patterns = vec![sample_pattern(
            "test-no-db",
            &["**/*.test.*"],
            "no direct DB",
        )];
        let matched = match_patterns(&patterns, "C:\\project\\src\\foo.test.ts");
        assert_eq!(matched.len(), 1);
    }
}
