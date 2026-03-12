use clap::Subcommand;
use edda_core::event::{new_pr_event, PrEventParams};
use edda_ledger::Ledger;
use serde::Deserialize;
use std::path::Path;
use std::process::Command;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Subcommand)]
pub enum PrsCmd {
    /// Scan recent PRs and record them as events
    Scan {
        /// Maximum number of PRs to scan
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Only scan PRs in specific state (open, closed, merged, all)
        #[arg(long, default_value = "all")]
        state: String,
    },
}

pub fn run_prs(cmd: PrsCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        PrsCmd::Scan { limit, state } => scan_prs(repo_root, limit, &state),
    }
}

#[derive(Debug, Deserialize)]
struct GhPr {
    number: u64,
    state: String,
    merged_at: Option<String>,
    created_at: String,
    author: GhAuthor,
    title: String,
    reviews: Vec<GhReview>,
}

#[derive(Debug, Deserialize)]
struct GhAuthor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhReview {
    state: String,
    _author: GhAuthor,
}

fn scan_prs(repo_root: &Path, limit: usize, state: &str) -> anyhow::Result<()> {
    let gh_available = Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !gh_available {
        anyhow::bail!("gh CLI not found. Please install GitHub CLI: https://cli.github.com/");
    }

    println!(
        "Scanning recent PRs (state: {}, limit: {})...",
        state, limit
    );

    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            state,
            "--limit",
            &limit.to_string(),
            "--json",
            "number,state,mergedAt,createdAt,author,title,reviews",
        ])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh pr list failed: {}", stderr);
    }

    let prs: Vec<GhPr> = serde_json::from_slice(&output.stdout)?;

    if prs.is_empty() {
        println!("No PRs found.");
        return Ok(());
    }

    println!("Found {} PRs", prs.len());

    let ledger = Ledger::open(repo_root)?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    let mut recorded = 0;
    let mut skipped = 0;

    for pr in prs {
        let pr_status = determine_pr_status(&pr);
        let review_result = determine_review_result(&pr.reviews);
        let blocker_count = count_blockers(&pr.reviews);
        let time_to_merge_hours = calculate_time_to_merge(&pr);

        let params = PrEventParams {
            branch: branch.clone(),
            parent_hash: parent_hash.clone(),
            pr_number: pr.number,
            pr_status: pr_status.clone(),
            review_result: review_result.clone(),
            blocker_count,
            time_to_merge_hours,
            created_at: pr.created_at.clone(),
            merged_at: pr.merged_at.clone(),
            author: pr.author.login.clone(),
            title: pr.title.clone(),
        };

        let event = new_pr_event(&params)?;

        match ledger.append_event(&event) {
            Ok(_) => {
                println!(
                    "  PR #{}: {} [{}] - {}",
                    pr.number,
                    pr_status,
                    review_result.as_deref().unwrap_or("no review"),
                    pr.title
                );
                recorded += 1;
            }
            Err(e) => {
                eprintln!("  Failed to record PR #{}: {}", pr.number, e);
                skipped += 1;
            }
        }
    }

    println!("\nRecorded {} PR events, skipped {}", recorded, skipped);
    Ok(())
}

fn determine_pr_status(pr: &GhPr) -> String {
    if pr.merged_at.is_some() {
        "merged".to_string()
    } else if pr.state == "CLOSED" {
        "closed".to_string()
    } else {
        "open".to_string()
    }
}

fn determine_review_result(reviews: &[GhReview]) -> Option<String> {
    let mut approved = false;
    let mut changes_requested = false;

    for review in reviews {
        match review.state.as_str() {
            "APPROVED" => approved = true,
            "CHANGES_REQUESTED" => changes_requested = true,
            _ => {}
        }
    }

    if changes_requested {
        Some("changes_requested".to_string())
    } else if approved {
        Some("approved".to_string())
    } else {
        None
    }
}

fn count_blockers(reviews: &[GhReview]) -> u32 {
    reviews
        .iter()
        .filter(|r| r.state == "CHANGES_REQUESTED")
        .count() as u32
}

fn calculate_time_to_merge(pr: &GhPr) -> Option<f64> {
    if let (Some(merged_at), created_at) = (&pr.merged_at, &pr.created_at) {
        let merged = OffsetDateTime::parse(merged_at, &Rfc3339).ok()?;
        let created = OffsetDateTime::parse(created_at, &Rfc3339).ok()?;
        let duration = merged - created;
        Some(duration.whole_seconds() as f64 / 3600.0)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_pr_status() {
        let mut pr = GhPr {
            number: 1,
            state: "OPEN".to_string(),
            merged_at: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            author: GhAuthor {
                login: "test".to_string(),
            },
            title: "Test PR".to_string(),
            reviews: vec![],
        };
        assert_eq!(determine_pr_status(&pr), "open");

        pr.state = "CLOSED".to_string();
        assert_eq!(determine_pr_status(&pr), "closed");

        pr.merged_at = Some("2024-01-02T00:00:00Z".to_string());
        assert_eq!(determine_pr_status(&pr), "merged");
    }

    #[test]
    fn test_determine_review_result() {
        let reviews = vec![];
        assert_eq!(determine_review_result(&reviews), None);

        let reviews = vec![GhReview {
            state: "APPROVED".to_string(),
            author: GhAuthor {
                login: "reviewer1".to_string(),
            },
        }];
        assert_eq!(
            determine_review_result(&reviews),
            Some("approved".to_string())
        );

        let reviews = vec![
            GhReview {
                state: "CHANGES_REQUESTED".to_string(),
                author: GhAuthor {
                    login: "reviewer1".to_string(),
                },
            },
            GhReview {
                state: "APPROVED".to_string(),
                author: GhAuthor {
                    login: "reviewer2".to_string(),
                },
            },
        ];
        assert_eq!(
            determine_review_result(&reviews),
            Some("changes_requested".to_string())
        );
    }

    #[test]
    fn test_count_blockers() {
        let reviews = vec![
            GhReview {
                state: "CHANGES_REQUESTED".to_string(),
                author: GhAuthor {
                    login: "reviewer1".to_string(),
                },
            },
            GhReview {
                state: "APPROVED".to_string(),
                author: GhAuthor {
                    login: "reviewer2".to_string(),
                },
            },
            GhReview {
                state: "CHANGES_REQUESTED".to_string(),
                author: GhAuthor {
                    login: "reviewer3".to_string(),
                },
            },
        ];
        assert_eq!(count_blockers(&reviews), 2);
    }
}
