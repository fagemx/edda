use super::evidence::{spec_trust, SpecOrigin};
use super::git::{commit, git};
use anyhow::{bail, Context, Result};
use edda_core::ReviewSpec;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

pub(crate) fn gh(repo: &Path, args: &[&str]) -> Result<Value> {
    let output = Command::new("gh")
        .args(args)
        .current_dir(repo)
        .output()
        .context("run gh")?;
    if !output.status.success() {
        bail!("gh: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub(crate) struct PrSubject {
    pub head: String,
    pub base: String,
    pub issue: Option<u64>,
}

pub(crate) fn resolve_pr(repo: &Path, number: u64) -> Result<PrSubject> {
    for _ in 0..2 {
        let value = gh(
            repo,
            &[
                "pr",
                "view",
                &number.to_string(),
                "--json",
                "headRefOid,baseRefName,body",
            ],
        )?;
        let expected = value["headRefOid"].as_str().context("PR head missing")?;
        if expected.len() != 40 || !expected.bytes().all(|v| v.is_ascii_hexdigit()) {
            bail!("invalid PR head SHA");
        }
        git(
            repo,
            &[
                "fetch",
                "--no-tags",
                "origin",
                &format!("pull/{number}/head"),
            ],
        )?;
        if commit(repo, "FETCH_HEAD")? != expected {
            continue;
        }
        let base = value["baseRefName"].as_str().context("PR base missing")?;
        git(repo, &["check-ref-format", &format!("refs/heads/{base}")])?;
        git(
            repo,
            &[
                "fetch",
                "--no-tags",
                "origin",
                &format!("refs/heads/{base}"),
            ],
        )?;
        let base_sha = commit(repo, "FETCH_HEAD")?;
        return Ok(PrSubject {
            head: expected.into(),
            base: base_sha,
            issue: closing_issue(value["body"].as_str().unwrap_or("")),
        });
    }
    bail!("PR changed during fetch; retry review")
}

pub(crate) fn closing_issue(body: &str) -> Option<u64> {
    let words = body.split_whitespace().collect::<Vec<_>>();
    words.windows(2).find_map(|pair| {
        let keyword = pair[0]
            .trim_matches(|c: char| !c.is_ascii_alphabetic())
            .to_ascii_lowercase();
        if ![
            "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
        ]
        .contains(&keyword.as_str())
        {
            return None;
        }
        let issue = pair[1]
            .trim_end_matches(|c: char| c.is_ascii_punctuation() && c != '#')
            .strip_prefix('#')?;
        issue.parse().ok()
    })
}

pub(crate) fn load_spec(
    repo: &Path,
    cwd: &Path,
    explicit: Option<&str>,
    inferred: Option<u64>,
    trust: bool,
) -> Result<(ReviewSpec, String, Option<u64>)> {
    if let Some(path) = explicit.filter(|s| !s.starts_with('#')) {
        let text =
            std::fs::read_to_string(cwd.join(path)).with_context(|| format!("read spec {path}"))?;
        return Ok((
            ReviewSpec {
                mode: "spec-backed".into(),
                source: path.into(),
                trust: spec_trust(&SpecOrigin::Path, false).into(),
            },
            text,
            None,
        ));
    }
    let issue = match explicit {
        Some(value) => Some(
            value
                .strip_prefix('#')
                .context("expected #issue")?
                .parse::<u64>()?,
        ),
        None => inferred,
    };
    let Some(number) = issue else {
        return Ok((
            ReviewSpec {
                mode: "convention-only".into(),
                source: "none".into(),
                trust: spec_trust(&SpecOrigin::None, false).into(),
            },
            String::new(),
            None,
        ));
    };
    let value = gh(
        repo,
        &[
            "issue",
            "view",
            &number.to_string(),
            "--json",
            "body,author",
        ],
    )?;
    let body = value["body"]
        .as_str()
        .context("issue body missing")?
        .to_owned();
    let origin = if explicit.is_some() {
        SpecOrigin::ExplicitIssue
    } else {
        let login = value["author"]["login"]
            .as_str()
            .context("issue author missing")?;
        let permission = gh(
            repo,
            &[
                "api",
                &format!("repos/{{owner}}/{{repo}}/collaborators/{login}/permission"),
            ],
        );
        SpecOrigin::PrDerived {
            author_perm: permission
                .ok()
                .and_then(|v| v["permission"].as_str().map(str::to_owned)),
        }
    };
    Ok((
        ReviewSpec {
            mode: "spec-backed".into(),
            source: format!("issue#{number}"),
            trust: spec_trust(&origin, trust).into(),
        },
        body,
        Some(number),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_keywords_ignore_mentions_and_use_first_closing_issue() {
        assert_eq!(closing_issue("Issue: #1\nCloses #652. Fixes #3"), Some(652));
        assert_eq!(closing_issue("encloses #9; related #652"), None);
        assert_eq!(closing_issue("resolved #7"), Some(7));
    }
}
