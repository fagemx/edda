use anyhow::{bail, Result};
use edda_core::model_id::canonical_model_id;
use edda_ledger::Ledger;

#[derive(Debug, Default)]
pub(crate) struct Authors {
    pub sessions: Vec<String>,
    pub models: Vec<String>,
    pub unverifiable: bool,
}

pub(crate) fn authors(
    ledger: &Ledger,
    commits: &[String],
    subjects: &[String],
    trailers: &str,
) -> Result<Authors> {
    let mut authors = Authors::default();
    for event in ledger.iter_events_by_type("note")? {
        let payload = &event.payload;
        if payload["source"] != "bridge:session_digest" {
            continue;
        }
        let Some(made) = payload["session_stats"]["commits_made"].as_array() else {
            continue;
        };
        let hit = made.iter().filter_map(|v| v.as_str()).any(|s| {
            if (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit()) {
                commits.iter().any(|commit| commit.starts_with(s))
            } else {
                subjects.iter().any(|title| title == s)
            }
        });
        if !hit {
            continue;
        }
        if let Some(session) = payload["session_id"].as_str().filter(|s| !s.is_empty()) {
            authors.sessions.push(session.into());
        }
        match payload["session_stats"]["model"]
            .as_str()
            .and_then(canonical_model_id)
        {
            Some(model) => authors.models.push(model),
            None => authors.unverifiable = true,
        }
    }
    for trailer in trailers.lines() {
        let Some((key, value)) = trailer.split_once(':') else {
            continue;
        };
        if key.eq_ignore_ascii_case("Co-Authored-By") {
            match canonical_model_id(value.split('<').next().unwrap_or("").trim()) {
                Some(model) => authors.models.push(model),
                None => authors.unverifiable = true,
            }
        }
    }
    authors.sessions.sort();
    authors.sessions.dedup();
    authors.models.sort();
    authors.models.dedup();
    Ok(authors)
}

pub(crate) fn independence(
    authors: &Authors,
    session: &str,
    observed: Option<&str>,
) -> Result<&'static str> {
    if authors
        .sessions
        .iter()
        .any(|author| same_session(author, session))
    {
        bail!("refused: reviewer uses the same session as an author ({session})");
    }
    let Some(model) = observed.and_then(canonical_model_id) else {
        return Ok("unverified");
    };
    if authors.models.contains(&model) {
        return Ok("same-model");
    }
    if authors.unverifiable || authors.models.is_empty() {
        return Ok("unverified");
    }
    Ok("verified")
}

pub(crate) fn same_session(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_session_is_refused_and_unknown_is_never_verified() {
        let authors = Authors {
            sessions: vec!["author".into()],
            models: vec!["gpt-5.6-sol".into()],
            unverifiable: false,
        };
        assert!(independence(&authors, "author", Some("glm-5.3-flash")).is_err());
        assert_eq!(
            independence(&authors, "reviewer", Some("openai-codex/gpt-5.6-sol")).unwrap(),
            "same-model"
        );
        assert!(same_session(
            "00000000-0000-4000-8000-0000000000AB",
            "00000000-0000-4000-8000-0000000000ab"
        ));
        assert_eq!(
            independence(&authors, "reviewer", Some("unknown")).unwrap(),
            "unverified"
        );
        assert_eq!(
            independence(&authors, "reviewer", Some("glm-5.3-flash")).unwrap(),
            "verified"
        );
    }
}
