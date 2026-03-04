use crate::state::load_state;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

fn resolve_default_anchor(edda_root: &PathBuf) -> Result<ResolvedAnchor> {
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
