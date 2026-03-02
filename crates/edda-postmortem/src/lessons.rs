//! Lesson management: persist lessons and maintain CLAUDE.md integration.
//!
//! Output hierarchy:
//!   - Rules: Hook (block/auto-run), 100% compliance
//!   - **Lessons**: CLAUDE.md auto-maintained paragraph, ~90% compliance
//!   - Observations: `edda ask` on-demand, no enforcement
//!
//! Lessons are stored per-project in state/lessons.json and optionally
//! synced to a managed section in the project's CLAUDE.md.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::analyzer::Lesson;

/// Persistent store for lessons.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LessonsStore {
    pub lessons: Vec<StoredLesson>,
    #[serde(default)]
    pub last_updated: Option<String>,
}

/// A lesson with persistence metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredLesson {
    pub id: String,
    pub text: String,
    pub severity: String,
    pub tags: Vec<String>,
    pub source_session: String,
    pub source_trigger: String,
    pub created_at: String,
    /// Number of times this lesson pattern was seen.
    pub occurrences: u64,
}

/// CLAUDE.md managed section markers.
const CLAUDE_MD_SECTION_START: &str = "<!-- edda:lessons:start -->";
const CLAUDE_MD_SECTION_END: &str = "<!-- edda:lessons:end -->";

impl LessonsStore {
    /// Load from disk, returning default if not found.
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist to disk atomically.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        edda_store::write_atomic(path, json.as_bytes())
    }

    /// Resolve the lessons path for a project.
    pub fn project_path(project_id: &str) -> PathBuf {
        edda_store::project_dir(project_id)
            .join("state")
            .join("lessons.json")
    }

    /// Load project-scoped lessons.
    pub fn load_project(project_id: &str) -> Self {
        Self::load(&Self::project_path(project_id))
    }

    /// Save project-scoped lessons.
    pub fn save_project(&self, project_id: &str) -> anyhow::Result<()> {
        self.save(&Self::project_path(project_id))
    }

    /// Add lessons from a post-mortem analysis.
    /// Deduplicates by checking if a similar lesson already exists (same tags + trigger).
    pub fn add_lessons(&mut self, lessons: &[Lesson], session_id: &str) {
        let now = now_rfc3339();
        for lesson in lessons {
            // Check for duplicate: same trigger + similar tags
            if let Some(existing) = self
                .lessons
                .iter_mut()
                .find(|l| l.source_trigger == lesson.source_trigger && l.tags == lesson.tags)
            {
                existing.occurrences += 1;
                existing.text = lesson.text.clone(); // Update text with latest
                continue;
            }

            self.lessons.push(StoredLesson {
                id: lesson.id.clone(),
                text: lesson.text.clone(),
                severity: format!("{:?}", lesson.severity).to_lowercase(),
                tags: lesson.tags.clone(),
                source_session: session_id.to_string(),
                source_trigger: lesson.source_trigger.clone(),
                created_at: now.clone(),
                occurrences: 1,
            });
        }
        self.last_updated = Some(now);
    }

    /// Get the top N lessons by occurrence count, for CLAUDE.md sync.
    pub fn top_lessons(&self, n: usize) -> Vec<&StoredLesson> {
        let mut sorted: Vec<&StoredLesson> = self.lessons.iter().collect();
        sorted.sort_by(|a, b| b.occurrences.cmp(&a.occurrences));
        sorted.truncate(n);
        sorted
    }

    /// Render lessons as a markdown section for CLAUDE.md.
    pub fn render_claude_md_section(&self, max_lessons: usize) -> String {
        let top = self.top_lessons(max_lessons);
        if top.is_empty() {
            return String::new();
        }

        let mut lines = vec![
            CLAUDE_MD_SECTION_START.to_string(),
            "## Learned Lessons (edda L3)".to_string(),
            String::new(),
        ];

        for lesson in &top {
            let severity_prefix = match lesson.severity.as_str() {
                "high" => "[HIGH] ",
                "medium" => "[MED] ",
                _ => "",
            };
            lines.push(format!(
                "- {severity_prefix}{} (seen {}x)",
                lesson.text, lesson.occurrences
            ));
        }

        lines.push(String::new());
        lines.push(CLAUDE_MD_SECTION_END.to_string());
        lines.join("\n")
    }

    /// Sync lessons to a CLAUDE.md file, replacing the managed section.
    ///
    /// If the file doesn't have the managed section markers, appends them.
    /// Returns true if the file was modified.
    pub fn sync_to_claude_md(
        &self,
        claude_md_path: &Path,
        max_lessons: usize,
    ) -> anyhow::Result<bool> {
        let section = self.render_claude_md_section(max_lessons);
        if section.is_empty() {
            return Ok(false);
        }

        let content = fs::read_to_string(claude_md_path).unwrap_or_default();

        let new_content = if let (Some(start_pos), Some(end_pos)) = (
            content.find(CLAUDE_MD_SECTION_START),
            content.find(CLAUDE_MD_SECTION_END),
        ) {
            // Replace existing section
            let end_pos = end_pos + CLAUDE_MD_SECTION_END.len();
            format!(
                "{}{}{}",
                &content[..start_pos],
                section,
                &content[end_pos..]
            )
        } else {
            // Append new section
            if content.is_empty() {
                section
            } else {
                format!("{}\n\n{}", content.trim_end(), section)
            }
        };

        if new_content == content {
            return Ok(false);
        }

        edda_store::write_atomic(claude_md_path, new_content.as_bytes())?;
        Ok(true)
    }
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{Lesson, LessonSeverity};

    fn make_lesson(text: &str, trigger: &str, tags: &[&str]) -> Lesson {
        Lesson {
            id: format!("lesson_{}", text.len()),
            text: text.to_string(),
            severity: LessonSeverity::Medium,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            source_trigger: trigger.to_string(),
        }
    }

    #[test]
    fn add_lessons_deduplicates() {
        let mut store = LessonsStore::default();
        let lessons = vec![make_lesson("Test lesson", "session_failures", &["failure"])];

        store.add_lessons(&lessons, "s1");
        assert_eq!(store.lessons.len(), 1);
        assert_eq!(store.lessons[0].occurrences, 1);

        // Add same lesson again -> increment occurrences
        store.add_lessons(&lessons, "s2");
        assert_eq!(store.lessons.len(), 1);
        assert_eq!(store.lessons[0].occurrences, 2);
    }

    #[test]
    fn different_lessons_not_deduplicated() {
        let mut store = LessonsStore::default();
        store.add_lessons(&[make_lesson("Lesson A", "trigger_a", &["tag_a"])], "s1");
        store.add_lessons(&[make_lesson("Lesson B", "trigger_b", &["tag_b"])], "s2");
        assert_eq!(store.lessons.len(), 2);
    }

    #[test]
    fn top_lessons_sorted_by_occurrence() {
        let mut store = LessonsStore::default();
        store.lessons.push(StoredLesson {
            id: "l1".into(),
            text: "Low".into(),
            severity: "low".into(),
            tags: vec![],
            source_session: "s1".into(),
            source_trigger: "t1".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            occurrences: 1,
        });
        store.lessons.push(StoredLesson {
            id: "l2".into(),
            text: "High".into(),
            severity: "high".into(),
            tags: vec![],
            source_session: "s2".into(),
            source_trigger: "t2".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            occurrences: 5,
        });

        let top = store.top_lessons(1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].text, "High");
    }

    #[test]
    fn render_claude_md_section_empty_when_no_lessons() {
        let store = LessonsStore::default();
        assert!(store.render_claude_md_section(5).is_empty());
    }

    #[test]
    fn render_claude_md_section_with_lessons() {
        let mut store = LessonsStore::default();
        store.lessons.push(StoredLesson {
            id: "l1".into(),
            text: "Always run tests".into(),
            severity: "high".into(),
            tags: vec![],
            source_session: "s1".into(),
            source_trigger: "t1".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            occurrences: 3,
        });

        let section = store.render_claude_md_section(5);
        assert!(section.contains(CLAUDE_MD_SECTION_START));
        assert!(section.contains(CLAUDE_MD_SECTION_END));
        assert!(section.contains("Always run tests"));
        assert!(section.contains("3x"));
    }

    #[test]
    fn sync_to_claude_md_appends_if_no_section() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(&path, "# My Project\n\nSome content.\n").unwrap();

        let mut store = LessonsStore::default();
        store.lessons.push(StoredLesson {
            id: "l1".into(),
            text: "Test lesson".into(),
            severity: "medium".into(),
            tags: vec![],
            source_session: "s1".into(),
            source_trigger: "t1".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            occurrences: 1,
        });

        let modified = store.sync_to_claude_md(&path, 5).unwrap();
        assert!(modified);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# My Project"));
        assert!(content.contains(CLAUDE_MD_SECTION_START));
        assert!(content.contains("Test lesson"));
    }

    #[test]
    fn sync_to_claude_md_replaces_existing_section() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let initial = format!(
            "# Project\n\n{}\nOld lessons\n{}\n\nMore content.",
            CLAUDE_MD_SECTION_START, CLAUDE_MD_SECTION_END
        );
        fs::write(&path, &initial).unwrap();

        let mut store = LessonsStore::default();
        store.lessons.push(StoredLesson {
            id: "l1".into(),
            text: "New lesson".into(),
            severity: "medium".into(),
            tags: vec![],
            source_session: "s1".into(),
            source_trigger: "t1".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            occurrences: 2,
        });

        let modified = store.sync_to_claude_md(&path, 5).unwrap();
        assert!(modified);

        let content = fs::read_to_string(&path).unwrap();
        assert!(!content.contains("Old lessons"));
        assert!(content.contains("New lesson"));
        assert!(content.contains("More content."));
    }

    #[test]
    fn store_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lessons.json");

        let mut store = LessonsStore::default();
        store.add_lessons(&[make_lesson("Test", "trigger", &["tag"])], "s1");
        store.save(&path).unwrap();

        let loaded = LessonsStore::load(&path);
        assert_eq!(loaded.lessons.len(), 1);
    }
}
