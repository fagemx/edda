//! GH-1015: collection and CLI entry point for `edda fleet order`.
//!
//! Everything impure lives here: `gh`, `git`, the ledger, and clap. Each is
//! read exactly once into an [`OrderInput`], so the ranker itself stays a pure
//! function that a fixture can drive.

use super::{
    compute_order, render_markdown, render_text, OpenIssue, OrderInput, FLASH_CAP_DEFAULT,
    FLASH_CAP_KEY,
};
use chrono::Utc;
use std::collections::BTreeSet;
use std::path::Path;

/// Server-side cap on the open-issue query.
const OPEN_ISSUE_CAP: u64 = 400;

/// Stack for the clap introspection thread; see [`collect_known_verbs`].
const VERB_TABLE_STACK_BYTES: usize = 32 * 1024 * 1024;

/// Flags for `edda fleet order`.
#[derive(clap::Args)]
pub struct OrderArgs {
    /// Read open issues from a JSON fixture instead of querying `gh`
    #[arg(long, value_name = "PATH")]
    pub issues: Option<std::path::PathBuf>,
    /// Use this health status (GREEN/YELLOW/RED) instead of computing it
    #[arg(long, value_name = "STATUS")]
    pub health_status: Option<String>,
    /// Rolling window in days for the health computation
    #[arg(long, default_value_t = 7)]
    pub window: u32,
    /// Emit the queue as JSON
    #[arg(long)]
    pub json: bool,
    /// Emit the board-comment markdown rendering
    #[arg(long)]
    pub markdown: bool,
    /// Keep only the top N rows (ranks are assigned before truncation)
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

/// Parse `gh issue list --json number,title,body,labels` output.
pub fn parse_open_issues(bytes: &[u8]) -> anyhow::Result<Vec<OpenIssue>> {
    Ok(serde_json::from_slice(bytes)?)
}

/// CLI entry point. Exit: 0 ok; 1 error, 2 usage.
pub fn run(args: OrderArgs, repo_root: &Path) -> anyhow::Result<()> {
    if args.json && args.markdown {
        eprintln!("Error: --json and --markdown are mutually exclusive");
        std::process::exit(2);
    }
    if args.window == 0 {
        eprintln!("Error: --window must be at least 1");
        std::process::exit(2);
    }
    let health_status = match &args.health_status {
        Some(status) => match normalize_status(status) {
            Some(status) => status,
            None => {
                eprintln!("Error: --health-status must be GREEN, YELLOW, or RED");
                std::process::exit(2);
            }
        },
        None => crate::cmd_fleet::live_health_status(repo_root, args.window),
    };
    let issues = match &args.issues {
        Some(path) => {
            let bytes = std::fs::read(path)
                .map_err(|err| anyhow::anyhow!("reading {}: {err}", path.display()))?;
            parse_open_issues(&bytes)
                .map_err(|err| anyhow::anyhow!("parsing {}: {err}", path.display()))?
        }
        None => collect_open_issues(repo_root),
    };
    let (flash_max_surface_files, flash_cap_source) = read_flash_cap(repo_root);

    let mut queue = compute_order(&OrderInput {
        issues,
        tree_paths: collect_tree_paths(repo_root),
        known_verbs: collect_known_verbs(),
        health_status,
        flash_max_surface_files,
        flash_cap_source,
        now: Utc::now(),
    });
    if let Some(limit) = args.limit {
        queue.rows.truncate(limit);
    }

    let rendered = if args.json {
        format!("{}\n", serde_json::to_string_pretty(&queue)?)
    } else if args.markdown {
        render_markdown(&queue)
    } else {
        render_text(&queue)
    };
    use std::io::Write;
    let mut stdout = std::io::stdout();
    stdout.write_all(rendered.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

/// Accept any casing of the three health statuses; reject everything else.
fn normalize_status(status: &str) -> Option<String> {
    let upper = status.trim().to_ascii_uppercase();
    matches!(upper.as_str(), "GREEN" | "YELLOW" | "RED").then_some(upper)
}

/// Read the flash lane's surface-file ceiling from the ledger. An unopenable
/// ledger or an unparseable value falls back to the built-in default.
fn read_flash_cap(repo_root: &Path) -> (usize, String) {
    if let Ok(ledger) = edda_ledger::Ledger::open(repo_root) {
        if let Ok(branch) = ledger.head_branch() {
            if let Ok(Some(decision)) = ledger.find_active_decision(&branch, FLASH_CAP_KEY) {
                if let Ok(value) = decision.value.trim().parse::<usize>() {
                    return (value, "ledger".to_string());
                }
            }
        }
    }
    (FLASH_CAP_DEFAULT, "default".to_string())
}

/// Every path tracked at the pinned tree. A scan that cannot see the tree must
/// not report: a `git` failure is fatal rather than an empty set, because an
/// empty set would FAIL every referenced path in every issue.
fn collect_tree_paths(repo_root: &Path) -> BTreeSet<String> {
    match std::process::Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "HEAD"])
        .current_dir(repo_root)
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim_end_matches('\r').to_string())
            .filter(|line| !line.is_empty())
            .collect(),
        Ok(output) => {
            eprintln!(
                "Error: git ls-tree failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("Error: git ls-tree failed: {err}");
            std::process::exit(1);
        }
    }
}

/// This binary's own top-level verbs — the pinned tree's, by construction.
/// Introspection, not `edda <verb> --help`: the `edda` on `PATH` may predate
/// the tree being ranked, which is exactly how #1015 earned a false FAIL.
///
/// `Cli::command()` rebuilds the whole command tree, and this CLI's tree is
/// deep enough that doing so from inside a subcommand overflows the main
/// thread's stack in a debug build (measured: `edda fleet order` aborted
/// before printing anything). Building it on a thread with its own stack keeps
/// the introspection here, where the property belongs. An unbuildable table
/// exits rather than degrading to an empty set, which would FAIL every command
/// mention in every issue.
fn collect_known_verbs() -> BTreeSet<String> {
    let built = std::thread::Builder::new()
        .stack_size(VERB_TABLE_STACK_BYTES)
        .spawn(|| {
            use clap::CommandFactory;
            crate::Cli::command()
                .get_subcommands()
                .map(|sub| sub.get_name().to_string())
                .collect::<BTreeSet<String>>()
        })
        .map(|handle| handle.join());
    match built {
        Ok(Ok(verbs)) => verbs,
        _ => {
            eprintln!("Error: could not build this binary's verb table");
            std::process::exit(1);
        }
    }
}

fn collect_open_issues(repo_root: &Path) -> Vec<OpenIssue> {
    let limit = OPEN_ISSUE_CAP.to_string();
    let stdout = crate::cmd_fleet::gh_output(
        repo_root,
        &[
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            &limit,
            "--json",
            "number,title,body,labels",
        ],
    );
    match parse_open_issues(&stdout) {
        Ok(issues) => issues,
        Err(err) => {
            eprintln!("Error: gh issue list output unparseable: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_open_issues_reads_the_gh_shape() {
        let bytes = br###"[
            {"number": 1015, "title": "fleet order",
             "body": "## doneWhen\n- ranked\n",
             "labels": [{"name": "fleet:ready"}, {"name": "P1"}]},
            {"number": 1016, "title": "no labels", "body": "", "labels": []}
        ]"###;
        let issues = parse_open_issues(bytes).expect("fixture parses");
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].number, 1015);
        assert_eq!(issues[0].labels[1].name, "P1");
        assert!(issues[1].labels.is_empty());
    }

    #[test]
    fn parse_open_issues_defaults_absent_optional_fields() {
        let issues = parse_open_issues(br#"[{"number": 7}]"#).expect("minimal row parses");
        assert_eq!(issues[0].number, 7);
        assert!(issues[0].title.is_empty());
        assert!(issues[0].body.is_empty());
        assert!(issues[0].labels.is_empty());
    }

    #[test]
    fn parse_open_issues_rejects_a_non_list() {
        assert!(parse_open_issues(br#"{"number": 7}"#).is_err());
    }

    #[test]
    fn normalize_status_accepts_any_casing_and_nothing_else() {
        assert_eq!(normalize_status(" red ").as_deref(), Some("RED"));
        assert_eq!(normalize_status("Yellow").as_deref(), Some("YELLOW"));
        assert_eq!(normalize_status("GREEN").as_deref(), Some("GREEN"));
        assert!(normalize_status("amber").is_none());
        assert!(normalize_status("").is_none());
    }

    /// The verb table `order` judges commands against must be this binary's,
    /// so `fleet` is present the moment the tree defines it.
    #[test]
    fn known_verbs_come_from_this_binarys_command_tree() {
        let verbs = collect_known_verbs();
        assert!(verbs.contains("fleet"));
        assert!(verbs.contains("ask"));
        assert!(!verbs.contains("definitely-not-a-verb"));
    }
}
