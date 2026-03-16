use crate::state::load_state;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Anchor {
    Default,
    Topic(String),
    Project(String),
    Week,
    Since(String),
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedAnchor {
    pub anchor_type: String,
    pub project_filter: Option<String>,
    pub time_filter: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub topic_query: Option<String>,
    pub session_ids: Vec<String>,
}

pub fn resolve_anchor(
    anchor: &Anchor,
    edda_root: &PathBuf,
    opts: &crate::RecapOptions,
) -> Result<ResolvedAnchor> {
    match anchor {
        Anchor::Default => resolve_default_anchor(edda_root),
        Anchor::Topic(query) => resolve_topic_anchor(query, opts),
        Anchor::Project(project) => resolve_project_anchor(project, edda_root),
        Anchor::Week => resolve_week_anchor(opts),
        Anchor::Since(date_str) => resolve_since_anchor(date_str, opts),
        Anchor::All => resolve_all_anchor(opts),
    }
}

fn resolve_default_anchor(edda_root: &Path) -> Result<ResolvedAnchor> {
    let state = load_state(edda_root)?;
    let now = Utc::now();

    let (start_time, session_ids) = match state {
        Some(state) => {
            let last_ts = chrono::DateTime::parse_from_rfc3339(&state.last_recap.timestamp)
                .with_context(|| "Failed to parse last_recap timestamp")?
                .with_timezone(&Utc);
            (last_ts, state.last_recap.sessions_covered)
        }
        None => {
            let start = now - Duration::hours(24);
            (start, vec![])
        }
    };

    Ok(ResolvedAnchor {
        anchor_type: "default".to_string(),
        project_filter: None,
        time_filter: Some((start_time, now)),
        topic_query: None,
        session_ids,
    })
}

fn resolve_topic_anchor(query: &str, opts: &crate::RecapOptions) -> Result<ResolvedAnchor> {
    let mut resolved = ResolvedAnchor {
        anchor_type: "topic".to_string(),
        project_filter: opts.project.clone(),
        time_filter: None,
        topic_query: Some(query.to_string()),
        session_ids: vec![],
    };

    if opts.week {
        let now = Utc::now();
        let week_ago = now - Duration::weeks(1);
        resolved.time_filter = Some((week_ago, now));
    } else if let Some(ref since) = opts.since {
        let start = chrono::DateTime::parse_from_rfc3339(since)
            .with_context(|| format!("Failed to parse --since date: {}", since))?
            .with_timezone(&Utc);
        resolved.time_filter = Some((start, Utc::now()));
    }

    Ok(resolved)
}

fn resolve_project_anchor(project: &str, _edda_root: &PathBuf) -> Result<ResolvedAnchor> {
    Ok(ResolvedAnchor {
        anchor_type: "project".to_string(),
        project_filter: Some(project.to_string()),
        time_filter: None,
        topic_query: None,
        session_ids: vec![],
    })
}

fn resolve_week_anchor(opts: &crate::RecapOptions) -> Result<ResolvedAnchor> {
    let now = Utc::now();
    let week_ago = now - Duration::weeks(1);

    Ok(ResolvedAnchor {
        anchor_type: "week".to_string(),
        project_filter: opts.project.clone(),
        time_filter: Some((week_ago, now)),
        topic_query: opts.query.clone(),
        session_ids: vec![],
    })
}

fn resolve_since_anchor(date_str: &str, opts: &crate::RecapOptions) -> Result<ResolvedAnchor> {
    let start = chrono::DateTime::parse_from_rfc3339(date_str)
        .with_context(|| format!("Failed to parse --since date: {}", date_str))?
        .with_timezone(&Utc);

    Ok(ResolvedAnchor {
        anchor_type: "since".to_string(),
        project_filter: opts.project.clone(),
        time_filter: Some((start, Utc::now())),
        topic_query: opts.query.clone(),
        session_ids: vec![],
    })
}

fn resolve_all_anchor(opts: &crate::RecapOptions) -> Result<ResolvedAnchor> {
    let time_filter = if opts.week {
        let now = Utc::now();
        Some((now - Duration::weeks(1), now))
    } else if let Some(ref since) = opts.since {
        let start = chrono::DateTime::parse_from_rfc3339(since)
            .with_context(|| format!("Failed to parse --since date: {}", since))?
            .with_timezone(&Utc);
        Some((start, Utc::now()))
    } else {
        None
    };

    Ok(ResolvedAnchor {
        anchor_type: "all".to_string(),
        project_filter: None,
        time_filter,
        topic_query: opts.query.clone(),
        session_ids: vec![],
    })
}

impl Anchor {
    pub fn from_options(opts: &crate::RecapOptions) -> Self {
        if let Some(ref query) = opts.query {
            Anchor::Topic(query.clone())
        } else if let Some(ref project) = opts.project {
            Anchor::Project(project.clone())
        } else if opts.week {
            Anchor::Week
        } else if let Some(ref since) = opts.since {
            Anchor::Since(since.clone())
        } else if opts.all {
            Anchor::All
        } else {
            Anchor::Default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RecapOptions;

    fn default_opts() -> RecapOptions {
        RecapOptions::default()
    }

    #[test]
    fn test_from_options_query() {
        let opts = RecapOptions {
            query: Some("auth flow".into()),
            ..default_opts()
        };
        let anchor = Anchor::from_options(&opts);
        assert!(matches!(anchor, Anchor::Topic(q) if q == "auth flow"));
    }

    #[test]
    fn test_from_options_project() {
        let opts = RecapOptions {
            project: Some("edda".into()),
            ..default_opts()
        };
        let anchor = Anchor::from_options(&opts);
        assert!(matches!(anchor, Anchor::Project(p) if p == "edda"));
    }

    #[test]
    fn test_from_options_week() {
        let opts = RecapOptions {
            week: true,
            ..default_opts()
        };
        let anchor = Anchor::from_options(&opts);
        assert!(matches!(anchor, Anchor::Week));
    }

    #[test]
    fn test_from_options_since() {
        let opts = RecapOptions {
            since: Some("2026-01-01T00:00:00Z".into()),
            ..default_opts()
        };
        let anchor = Anchor::from_options(&opts);
        assert!(matches!(anchor, Anchor::Since(s) if s == "2026-01-01T00:00:00Z"));
    }

    #[test]
    fn test_from_options_all() {
        let opts = RecapOptions {
            all: true,
            ..default_opts()
        };
        let anchor = Anchor::from_options(&opts);
        assert!(matches!(anchor, Anchor::All));
    }

    #[test]
    fn test_from_options_default() {
        let anchor = Anchor::from_options(&default_opts());
        assert!(matches!(anchor, Anchor::Default));
    }

    #[test]
    fn test_resolve_default_no_state() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = default_opts();
        let resolved = resolve_anchor(&Anchor::Default, &tmp.path().to_path_buf(), &opts).unwrap();

        assert_eq!(resolved.anchor_type, "default");
        assert!(resolved.project_filter.is_none());
        assert!(resolved.topic_query.is_none());
        assert!(resolved.session_ids.is_empty());

        let (start, end) = resolved.time_filter.expect("should have time filter");
        assert!(start < end);
        let duration = end - start;
        let expected = chrono::Duration::hours(24);
        assert!(
            (duration - expected).num_seconds().abs() < 5,
            "duration should be ~24h, got {}s",
            duration.num_seconds()
        );
    }

    #[test]
    fn test_resolve_project_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = default_opts();
        let resolved = resolve_anchor(
            &Anchor::Project("myproject".into()),
            &tmp.path().to_path_buf(),
            &opts,
        )
        .unwrap();

        assert_eq!(resolved.anchor_type, "project");
        assert_eq!(resolved.project_filter, Some("myproject".into()));
        assert!(resolved.time_filter.is_none());
    }

    #[test]
    fn test_resolve_week_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = default_opts();
        let resolved = resolve_anchor(&Anchor::Week, &tmp.path().to_path_buf(), &opts).unwrap();

        assert_eq!(resolved.anchor_type, "week");
        let (start, end) = resolved.time_filter.expect("should have time filter");
        assert!(start < end);
        let duration = end - start;
        let expected = chrono::Duration::weeks(1);
        assert!(
            (duration - expected).num_seconds().abs() < 5,
            "duration should be ~1 week"
        );
    }
}
