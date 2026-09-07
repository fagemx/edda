//! GH-1015: collection and CLI entry point for `edda fleet order`.
//!
//! Everything impure lives here: `gh`, `git`, the ledger, clap, and the two
//! flash criteria that can only be decided by running something. Each is
//! resolved exactly once into an [`OrderInput`], so the ranker itself stays a
//! pure function that a fixture can drive.

use super::{
    compute_order, issue_labels, non_flash_reason, render_markdown, render_text, FlashCheck,
    OpenIssue, OrderInput, CHECK_BRIEF_RENDER, CHECK_DISPATCH_DRY_RUN, FLASH_CAP_DEFAULT,
    FLASH_CAP_KEY,
};
use crate::cmd_fleet::surface_paths;
use chrono::Utc;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
    // A fixture's issue numbers are synthetic, so running the two subprocess
    // criteria against them would ask GitHub about issues the fixture invented.
    // The checks are skipped there instead, and every affected row says
    // `<check> not evaluated` and routes `strong` — never a silent flash.
    let (issues, run_flash_checks) = match &args.issues {
        Some(path) => {
            let bytes = std::fs::read(path)
                .map_err(|err| anyhow::anyhow!("reading {}: {err}", path.display()))?;
            let issues = parse_open_issues(&bytes)
                .map_err(|err| anyhow::anyhow!("parsing {}: {err}", path.display()))?;
            (issues, false)
        }
        None => (collect_open_issues(repo_root), true),
    };
    let (flash_max_surface_files, flash_cap_source) = read_flash_cap(repo_root);
    let flash_checks = if run_flash_checks {
        collect_flash_checks(repo_root, &issues, flash_max_surface_files)
    } else {
        BTreeMap::new()
    };

    let mut queue = compute_order(&OrderInput {
        issues,
        flash_checks,
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

/// Renders in flight at once. Each is a process spawn plus one `gh issue view`
/// round trip; measured serially over the 60-issue corpus the whole collection
/// took 7m44s, which is not a queue command. They are independent reads that
/// touch nothing in the repo, so they overlap safely — unlike the dry-run,
/// which does `git worktree add` and stays serial below.
const RENDER_CONCURRENCY: usize = 6;

/// Resolve the two subprocess flash criteria, for the issues that can still
/// reach the flash lane. [`non_flash_reason`] has already routed the rest, so
/// spending a process on them would buy a result nothing reads.
///
/// The order is #1015's, and a failed render short-circuits: the dry-run
/// validator has nothing to validate once the brief did not render. Results are
/// keyed by issue number, so thread scheduling cannot reach the output.
fn collect_flash_checks(
    repo_root: &Path,
    issues: &[OpenIssue],
    flash_cap: usize,
) -> BTreeMap<u64, Vec<FlashCheck>> {
    let candidates: Vec<u64> = issues
        .iter()
        .filter(|issue| {
            let surface = surface_paths(&issue.body);
            non_flash_reason(&surface, &issue_labels(issue), &issue.body, flash_cap).is_none()
        })
        .map(|issue| issue.number)
        .collect();

    let mut checks: BTreeMap<u64, Vec<FlashCheck>> = BTreeMap::new();
    std::thread::scope(|scope| {
        for batch in candidates.chunks(RENDER_CONCURRENCY) {
            let handles: Vec<_> = batch
                .iter()
                .map(|&number| {
                    (
                        number,
                        scope.spawn(move || run_brief_render(repo_root, number)),
                    )
                })
                .collect();
            for (number, handle) in handles {
                // A panicked worker decided nothing, and "not decided" is not a
                // pass: it records as a failure and the row routes strong.
                let render = handle.join().unwrap_or_else(|_| {
                    FlashCheck::new(CHECK_BRIEF_RENDER, false, "brief-render worker panicked")
                });
                checks.insert(number, vec![render]);
            }
        }
    });

    for (number, results) in checks.iter_mut() {
        if results.iter().all(|check| check.passed) {
            results.push(run_dispatch_dry_run(repo_root, *number));
        }
    }
    checks
}

/// #885's renderer, invoked the way `next-issue.sh` invokes it. It only prints
/// — no worktree, branch or lane is created — so the arguments below are the
/// names the launch *would* use, not names that exist. Exit 0 is the criterion.
fn run_brief_render(repo_root: &Path, number: u64) -> FlashCheck {
    let worktree = format!("{}-wt-gh{number}", repo_root.display());
    let output = std::process::Command::new("sh")
        .args([
            "scripts/fleet/brief-from-issue.sh",
            &number.to_string(),
            "--lane-name",
            &format!("edda-lane-gh{number}"),
            "--worktree",
            &worktree,
            "--branch",
            &format!("feat/gh{number}"),
        ])
        .current_dir(repo_root)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            FlashCheck::new(CHECK_BRIEF_RENDER, true, "brief-from-issue.sh exited 0")
        }
        Ok(output) => FlashCheck::new(
            CHECK_BRIEF_RENDER,
            false,
            format!(
                "brief-from-issue.sh exited {}: {}",
                exit_code(&output.status),
                first_line([&output.stderr, &output.stdout])
            ),
        ),
        Err(err) => FlashCheck::new(
            CHECK_BRIEF_RENDER,
            false,
            format!("could not run brief-from-issue.sh: {err}"),
        ),
    }
}

/// #945's dry-run validator, which applies a brief's authored span in a
/// throwaway worktree at the pinned base SHA.
///
/// Its precondition is an *authored* brief: a freshly rendered one carries the
/// `<<AUTHORED STEPS>>` placeholder and no `<<AUTHORED: BEGIN>>` span, and the
/// validator refuses it with exit 2. That refusal is not a pass, so an issue
/// whose authored middle nobody has written yet routes `strong` — which is the
/// safe direction, and the note says exactly what is missing.
fn run_dispatch_dry_run(repo_root: &Path, number: u64) -> FlashCheck {
    let Some(path) = authored_brief_path(number) else {
        return FlashCheck::new(
            CHECK_DISPATCH_DRY_RUN,
            false,
            "cannot resolve the fleet scratch directory",
        );
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return FlashCheck::new(
            CHECK_DISPATCH_DRY_RUN,
            false,
            format!("no authored brief at {}", path.display()),
        );
    };
    if text.lines().any(|line| line == "<<AUTHORED STEPS>>") {
        return FlashCheck::new(
            CHECK_DISPATCH_DRY_RUN,
            false,
            format!(
                "brief at {} still carries the <<AUTHORED STEPS>> placeholder; nothing to dry-run",
                path.display()
            ),
        );
    }
    let output = std::process::Command::new("sh")
        .args([
            "scripts/fleet/brief-validate.sh",
            &path.display().to_string(),
        ])
        .current_dir(repo_root)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            FlashCheck::new(CHECK_DISPATCH_DRY_RUN, true, "brief-validate.sh VALID")
        }
        Ok(output) => FlashCheck::new(
            CHECK_DISPATCH_DRY_RUN,
            false,
            format!(
                "brief-validate.sh exited {}: {}",
                exit_code(&output.status),
                first_line([&output.stdout, &output.stderr])
            ),
        ),
        Err(err) => FlashCheck::new(
            CHECK_DISPATCH_DRY_RUN,
            false,
            format!("could not run brief-validate.sh: {err}"),
        ),
    }
}

/// Where `next-issue.sh` writes and re-reads a lane brief. Same env var, same
/// name, so the validator sees the brief the launch would actually use.
fn authored_brief_path(number: u64) -> Option<PathBuf> {
    let scratch = match std::env::var("EDDA_FLEET_SCRATCH") {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => edda_core::paths::home_dir()?.join(".edda").join("fleet"),
    };
    Some(scratch.join(format!("brief-gh{number}.md")))
}

fn exit_code(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "by signal".to_string(),
    }
}

/// The one line worth quoting back into a lane reason: these scripts report
/// their refusal first, and a row note is not a place for a transcript. Streams
/// are searched in the order given — `brief-from-issue.sh` dies on stderr,
/// `brief-validate.sh` prints `INVALID step=<n>` on stdout.
fn first_line(streams: [&[u8]; 2]) -> String {
    for bytes in streams {
        let text = String::from_utf8_lossy(bytes);
        if let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty()) {
            return line.to_string();
        }
    }
    "(no output)".to_string()
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
        Ok(issues) => {
            // A full page is indistinguishable from a truncated corpus, so say
            // so: `total_issues` in the header would otherwise report the
            // prefix as the whole queue. Same signal `edda fleet health` gives
            // for its own PR and issue caps.
            if issues.len() as u64 >= OPEN_ISSUE_CAP {
                eprintln!(
                    "warning: open-issue fetch hit its cap ({OPEN_ISSUE_CAP}) — the queue ranks a \
                     prefix of the corpus, not all of it"
                );
            }
            issues
        }
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
