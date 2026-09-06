use super::git::{commit, git, git_ok, resolve_base};
use anyhow::{bail, Result};
use edda_core::{ReviewRefs, ReviewVerdictPayload};
use edda_ledger::Ledger;
use std::path::Path;

#[derive(Debug, Clone)]
pub(crate) struct Subject {
    pub base_sha: String,
    pub head_sha: String,
    pub files: Vec<String>,
    pub lines: usize,
}

pub(crate) fn resolve_subject(cwd: &Path, base: Option<&str>, head: &str) -> Result<Subject> {
    let head_sha = commit(cwd, head)?;
    let base_ref = resolve_base(cwd, base)?;
    let base_sha = git(cwd, &["merge-base", &base_ref, &head_sha])?;
    let range = format!("{base_sha}..{head_sha}");
    let files = git(
        cwd,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-only",
            "-z",
            &range,
            "--",
        ],
    )?
    .split('\0')
    .filter(|s| !s.is_empty())
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if files.is_empty() {
        bail!("empty diff: no committed changes to review");
    }
    let stats = git(
        cwd,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--numstat",
            &range,
            "--",
        ],
    )?;
    let lines = stats
        .lines()
        .map(|line| {
            line.split('\t')
                .take(2)
                .filter_map(|v| v.parse::<usize>().ok())
                .sum::<usize>()
        })
        .sum();
    Ok(Subject {
        head_sha,
        base_sha,
        files,
        lines,
    })
}

pub(crate) fn history(
    ledger: &Ledger,
    repo: &Path,
    subject: &Subject,
    pr: Option<u64>,
) -> Result<(ReviewRefs, Option<ReviewVerdictPayload>)> {
    let mut candidates = ledger.iter_events_by_type("review_verdict")?;
    candidates.sort_by(|a, b| a.ts.cmp(&b.ts));
    for event in candidates.iter().rev() {
        let prior: ReviewVerdictPayload = serde_json::from_value(event.payload.clone())?;
        if prior.verdict == "unreviewed" {
            continue;
        }
        // An explicit PR identity never resumes a different PR's conversation.
        if pr.is_some() && prior.refs.pr != pr {
            continue;
        }
        let head = commit(repo, &prior.subject.head_sha);
        if head.as_ref().is_ok_and(|head| head == &subject.head_sha) {
            // A second verdict on the identical immutable subject is another
            // round of the same review, never a replacement of the first.
            return Ok((
                ReviewRefs {
                    pr,
                    issue: None,
                    round: Some(prior.refs.round.unwrap_or(0).saturating_add(1)),
                    supersedes: None,
                    previous: Some(event.event_id.clone()),
                    history_rewritten: false,
                },
                Some(prior),
            ));
        }
        let in_range = match head {
            Ok(head) => {
                git_ok(
                    repo,
                    &["merge-base", "--is-ancestor", &head, &subject.head_sha],
                )? && !git_ok(
                    repo,
                    &["merge-base", "--is-ancestor", &head, &subject.base_sha],
                )?
            }
            Err(_) => false,
        };
        if in_range || (pr.is_some() && prior.refs.pr == pr) {
            return Ok((
                ReviewRefs {
                    pr,
                    issue: None,
                    round: Some(prior.refs.round.unwrap_or(0).saturating_add(1)),
                    supersedes: in_range.then(|| event.event_id.clone()),
                    previous: (!in_range).then(|| event.event_id.clone()),
                    history_rewritten: !in_range,
                },
                Some(prior),
            ));
        }
    }
    Ok((
        ReviewRefs {
            pr,
            issue: None,
            round: Some(1),
            supersedes: None,
            previous: None,
            history_rewritten: false,
        },
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd_review::git::{testrepo, WorktreeGuard};
    #[test]
    fn committed_head_of_caller_worktree_and_empty_diff() {
        let (_temp, root) = testrepo::init();
        assert!(resolve_subject(&root, None, "HEAD")
            .unwrap_err()
            .to_string()
            .contains("empty diff"));
        testrepo::run(&root, &["checkout", "-qb", "feature"]);
        let head = testrepo::commit_file(&root, "b.txt", "b", "feature");
        let s = resolve_subject(&root, None, "HEAD").unwrap();
        assert_eq!(s.head_sha, head);
        assert_eq!(s.files, ["b.txt"]);
        assert!(super::commit(&root, "--output=evil").is_err());
    }
    #[test]
    fn scratch_refuses_existing_and_cleans_only_owned_checkout() {
        let (_temp, root) = testrepo::init();
        let head = commit(&root, "HEAD").unwrap();
        let destination = root.join("review");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("keep"), "user data").unwrap();
        assert!(WorktreeGuard::create(&root, &destination, &head, false).is_err());
        assert!(destination.join("keep").exists());
        let owned = root.join("owned");
        {
            let _guard = WorktreeGuard::create(&root, &owned, &head, false).unwrap();
        }
        assert!(!owned.exists());
    }
}
