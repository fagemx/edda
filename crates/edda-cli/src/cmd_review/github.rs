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

/// Run `gh` for a write whose stdout is not JSON — a label edit, a comment
/// post, and a status post each print a bare URL or nothing on success, so
/// none of them can go through [`gh`], which parses stdout as JSON. Only
/// the exit status is meaningful here; stdout is discarded.
pub(crate) fn gh_write(repo: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("gh")
        .args(args)
        .current_dir(repo)
        .output()
        .context("run gh")?;
    if !output.status.success() {
        bail!("gh: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

/// The PR's current head SHA — a lighter read than [`resolve_pr`], which
/// also fetches the head and base commits into private refs for `edda
/// review`'s own diffing. `edda review deliver`'s moved-head check only
/// ever compares this string; it has no need to make the commit locally
/// resolvable.
pub(crate) fn pr_head(repo: &Path, number: u64) -> Result<String> {
    let value = gh(
        repo,
        &["pr", "view", &number.to_string(), "--json", "headRefOid"],
    )?;
    let head = value["headRefOid"].as_str().context("PR head missing")?;
    anyhow::ensure!(
        head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid PR head SHA"
    );
    Ok(head.to_owned())
}

pub(crate) struct PrSubject {
    pub head: String,
    pub base: String,
    pub issue: Option<u64>,
}

/// Fetch one remote ref into a review-private ref, and resolve that.
///
/// `FETCH_HEAD` lives in the **common** git directory, so the main checkout
/// and every linked worktree share one cell. Two `edda review` processes
/// fetching at the same time overwrite each other's between one process's
/// fetch and its `rev-parse`, and the loser resolves whatever the other left
/// behind — a foreign commit, or the multi-line value git reports as
/// `fatal: Needed a single revision` (GH-997). The fleet launches per-PR
/// review lanes into one checkout and serialises none of them.
///
/// Naming a private destination removes the shared cell rather than guarding
/// it: concurrent reviews of different PRs never name the same ref, and two
/// reviews of the same PR fetch the same objects to it. `--force` is required
/// because a PR head that was rebased or force-pushed moves the ref
/// non-fast-forward. The ref is left in place: it costs one file, and it
/// keeps the reviewed objects resolvable for a SHA-pinned verdict.
fn fetch_private_ref(repo: &Path, number: u64, role: &str, remote_ref: &str) -> Result<String> {
    let private = format!("refs/edda/review/pr{number}/{role}");
    git(
        repo,
        &[
            "fetch",
            "--no-tags",
            "--force",
            "origin",
            &format!("{remote_ref}:{private}"),
        ],
    )?;
    commit(repo, &private)
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
        if fetch_private_ref(repo, number, "head", &format!("pull/{number}/head"))? != expected {
            continue;
        }
        let base = value["baseRefName"].as_str().context("PR base missing")?;
        git(repo, &["check-ref-format", &format!("refs/heads/{base}")])?;
        let base_sha = fetch_private_ref(repo, number, "base", &format!("refs/heads/{base}"))?;
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
    use crate::cmd_review::git::testrepo;

    #[test]
    fn closing_keywords_ignore_mentions_and_use_first_closing_issue() {
        assert_eq!(closing_issue("Issue: #1\nCloses #652. Fixes #3"), Some(652));
        assert_eq!(closing_issue("encloses #9; related #652"), None);
        assert_eq!(closing_issue("resolved #7"), Some(7));
    }

    /// Two review lanes in one checkout, interleaved at the point that used
    /// to lose the subject: lane 1 fetches, lane 2 fetches, lane 1 resolves.
    /// Driving the interleaving directly rather than racing two threads makes
    /// the regression deterministic — the old code loses on every run, not on
    /// an unlucky one.
    #[test]
    fn a_concurrent_fetch_cannot_redirect_a_resolved_subject() {
        let (_origin_temp, origin) = testrepo::init();
        let pr1 = testrepo::commit_file(&origin, "pr1.txt", "one\n", "pr 1");
        testrepo::run(&origin, &["update-ref", "refs/pull/1/head", &pr1]);
        let pr2 = testrepo::commit_file(&origin, "pr2.txt", "two\n", "pr 2");
        testrepo::run(&origin, &["update-ref", "refs/pull/2/head", &pr2]);
        assert_ne!(pr1, pr2);

        let (_work_temp, work) = testrepo::init();
        testrepo::run(
            &work,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );

        // Lane 1 fetches its PR.
        let resolved = fetch_private_ref(&work, 1, "head", "pull/1/head").expect("lane 1 fetch");
        assert_eq!(resolved, pr1);

        // Lane 2 fetches its own, moving the cell both lanes used to read.
        fetch_private_ref(&work, 2, "head", "pull/2/head").expect("lane 2 fetch");
        assert_eq!(
            commit(&work, "FETCH_HEAD").expect("shared cell"),
            pr2,
            "precondition: the shared FETCH_HEAD really did move under lane 1"
        );

        // Lane 1 resolves its subject. Anything reading the shared cell now
        // answers pr2; lane 1's own ref still answers pr1.
        assert_eq!(
            commit(&work, "refs/edda/review/pr1/head").expect("lane 1 subject"),
            pr1,
            "lane 1 must resolve its own PR, not whatever lane 2 last fetched"
        );
    }

    /// A rebased or force-pushed head moves the private ref non-fast-forward;
    /// a second review of the same PR must follow it rather than fail.
    #[test]
    fn a_force_pushed_head_moves_the_private_ref() {
        let (_origin_temp, origin) = testrepo::init();
        let first = testrepo::commit_file(&origin, "pr.txt", "first\n", "round 1");
        testrepo::run(&origin, &["update-ref", "refs/pull/7/head", &first]);

        let (_work_temp, work) = testrepo::init();
        testrepo::run(
            &work,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        assert_eq!(
            fetch_private_ref(&work, 7, "head", "pull/7/head").expect("round 1"),
            first
        );

        testrepo::run(&origin, &["reset", "-q", "--hard", "HEAD~1"]);
        let rewritten = testrepo::commit_file(&origin, "pr.txt", "rewritten\n", "round 2");
        testrepo::run(&origin, &["update-ref", "refs/pull/7/head", &rewritten]);
        assert_eq!(
            fetch_private_ref(&work, 7, "head", "pull/7/head").expect("round 2"),
            rewritten
        );
    }
}
