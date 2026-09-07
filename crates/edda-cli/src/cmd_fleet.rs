//! GH-1014: fleet health — path-classified merge/issue mix.
//!
//! `edda fleet health` measures queue health mechanically: how much of the
//! merged work in a rolling window was product-class versus mechanism-class,
//! and how fast mechanism issues are being opened. Thresholds come from the
//! ledger (operator-ratified decisions); when no threshold decision exists a
//! built-in default applies. RED freezes mechanism dispatch so the ordering
//! layer has a machine-readable source of truth.
//!
//! Layout follows `cmd_recap_digest`: pure computation and rendering over
//! pre-collected structs, with fixture-only inline tests that never touch the
//! network, the ledger, or the filesystem.

use chrono::{DateTime, Duration, Utc};
use clap::Subcommand;
use serde::Deserialize;
use std::path::Path;

/// Server-side caps on the `gh` list queries.
const PR_CAP: u64 = 200;
const ISSUE_CAP: u64 = 300;

#[derive(Subcommand)]
pub enum FleetCmd {
    /// Fleet health: path-classified merge/issue mix, thresholds, status
    ///
    /// Exit: 0 GREEN, 3 YELLOW, 4 RED; 1 error, 2 usage.
    Health {
        /// Rolling window in days
        #[arg(long, default_value_t = 7)]
        window: u32,
        /// Emit the full health report as JSON
        #[arg(long)]
        json: bool,
        /// Emit one digest-friendly summary line
        #[arg(long)]
        line: bool,
    },
    /// Fleet order: deterministic health-gated queue with lane routing (GH-1015)
    ///
    /// Exit: 0 ok; 1 error, 2 usage.
    Order {
        #[command(flatten)]
        args: crate::cmd_fleet_order::OrderArgs,
    },
}

/// Path class assigned by `classify_path` / `classify_paths`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Product work: `crates/` or `sdk/`
    Product,
    /// Mechanism work: `scripts/`, `docs/fleet/`, `.github/`, `REVIEW.md`
    Mechanism,
    /// Everything else (docs, references, misc)
    Other,
}

#[derive(Debug, Deserialize)]
struct GhFile {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    merged_at: String,
    files: Vec<GhFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhIssue {
    number: u64,
    created_at: String,
    body: String,
}

/// One merged PR in the window, with its changed paths.
pub struct MergedPr {
    // Identity only; kept for consumer/debug fidelity even though compute
    // works purely on paths.
    #[allow(dead_code)]
    pub number: u64,
    #[allow(dead_code)]
    pub merged_at: DateTime<Utc>,
    pub paths: Vec<String>,
}

/// One issue opened in the window, with its body (for surface extraction).
pub struct IssueRow {
    // Identity only; kept for consumer/debug fidelity even though compute
    // works purely on the body's surface paths.
    #[allow(dead_code)]
    pub number: u64,
    pub created_at: DateTime<Utc>,
    pub body: String,
}

/// Operator thresholds. `source` records where they came from: `ledger` when
/// both keys resolved, `default` when neither did, `mixed` otherwise.
#[derive(serde::Serialize)]
pub struct Thresholds {
    pub product_share_floor_pct: f64,
    pub mech_issues_per_day_ceiling: f64,
    pub source: String,
}

#[derive(serde::Serialize)]
pub struct MergedCounts {
    product: u64,
    mechanism: u64,
    other: u64,
    total: u64,
    product_share_pct: Option<f64>,
    /// Rows the query returned before window filtering.
    pub fetched: u64,
    /// True when the fetch hit its server-side cap (200 PRs).
    pub truncated: bool,
}

#[derive(serde::Serialize)]
pub struct IssueCounts {
    product: u64,
    mechanism: u64,
    other: u64,
    total: u64,
    mechanism_per_day: f64,
    mechanism_last_24h: u64,
    /// Rows the query returned before window filtering.
    pub fetched: u64,
    /// True when the fetch hit its server-side cap (300 issues).
    pub truncated: bool,
}

/// The full fleet-health report. Serialized verbatim under `--json`.
#[derive(serde::Serialize)]
pub struct Health {
    window_days: u32,
    merged_prs: MergedCounts,
    issues_opened: IssueCounts,
    thresholds: Thresholds,
    status: String,
    mechanism_dispatch: String,
}

/// Classify one path. Comparison is on the path as given, after trimming
/// whitespace and a leading `./`.
pub fn classify_path(path: &str) -> Class {
    let p = path.trim();
    let p = p.strip_prefix("./").unwrap_or(p);
    if p.starts_with("crates/") || p.starts_with("sdk/") {
        Class::Product
    } else if p.starts_with("scripts/")
        || p.starts_with("docs/fleet/")
        || p.starts_with(".github/")
        || p == "REVIEW.md"
    {
        Class::Mechanism
    } else {
        Class::Other
    }
}

/// Classify a set of paths: the class with the largest count wins; ties
/// resolve Product over Mechanism over Other; an empty slice is Other.
pub fn classify_paths(paths: &[String]) -> Class {
    let mut product = 0usize;
    let mut mechanism = 0usize;
    let mut other = 0usize;
    for path in paths {
        match classify_path(path) {
            Class::Product => product += 1,
            Class::Mechanism => mechanism += 1,
            Class::Other => other += 1,
        }
    }
    if product == 0 && mechanism == 0 && other == 0 {
        return Class::Other;
    }
    if product >= mechanism && product >= other {
        Class::Product
    } else if mechanism >= other {
        Class::Mechanism
    } else {
        Class::Other
    }
}

/// Extract path-like backticked tokens from an issue body's
/// `## Predicted surface` section, in order. A token counts when it contains
/// a `/` or a `.`. Non-path words (e.g. `none`) are skipped.
pub fn surface_paths(issue_body: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut in_section = false;
    for line in issue_body.lines() {
        if line.starts_with("## Predicted surface") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if line.starts_with("## ") {
            break;
        }
        for token in line.split('`').skip(1).step_by(2) {
            let token = token.trim();
            if !token.is_empty() && (token.contains('/') || token.contains('.')) {
                result.push(token.to_string());
            }
        }
    }
    result
}

/// Pure, deterministic, network-free health computation. `fetched_prs` and
/// `fetched_issues` are the raw row counts each `gh` query returned before
/// window filtering; a fetch is `truncated` when it hit its cap.
pub fn compute(
    merged: &[MergedPr],
    issues: &[IssueRow],
    now: DateTime<Utc>,
    window_days: u32,
    thresholds: Thresholds,
    fetched_prs: u64,
    fetched_issues: u64,
) -> Health {
    let mut merged_counts = [0u64; 3];
    for pr in merged {
        merged_counts[classify_paths(&pr.paths) as usize] += 1;
    }
    let merged_total: u64 = merged_counts.iter().sum();

    let mut issue_counts = [0u64; 3];
    let mut mechanism_last_24h = 0u64;
    let day_ago = now - Duration::hours(24);
    for issue in issues {
        let class = classify_paths(&surface_paths(&issue.body));
        issue_counts[class as usize] += 1;
        if class == Class::Mechanism && issue.created_at >= day_ago {
            mechanism_last_24h += 1;
        }
    }
    let issue_total: u64 = issue_counts.iter().sum();

    let product_share_pct = if merged_total > 0 {
        Some(merged_counts[0] as f64 / merged_total as f64 * 100.0)
    } else {
        None
    };
    let mechanism_per_day = issue_counts[1] as f64 / window_days as f64;

    let status = if is_red(product_share_pct, mechanism_per_day, &thresholds) {
        "RED"
    } else if is_yellow(product_share_pct, mechanism_per_day, &thresholds) {
        "YELLOW"
    } else {
        "GREEN"
    };
    let mechanism_dispatch = if status == "RED" { "freeze" } else { "open" };

    Health {
        window_days,
        merged_prs: MergedCounts {
            product: merged_counts[0],
            mechanism: merged_counts[1],
            other: merged_counts[2],
            total: merged_total,
            product_share_pct,
            fetched: fetched_prs,
            truncated: fetched_prs >= PR_CAP,
        },
        issues_opened: IssueCounts {
            product: issue_counts[0],
            mechanism: issue_counts[1],
            other: issue_counts[2],
            total: issue_total,
            mechanism_per_day,
            mechanism_last_24h,
            fetched: fetched_issues,
            truncated: fetched_issues >= ISSUE_CAP,
        },
        thresholds,
        status: status.to_string(),
        mechanism_dispatch: mechanism_dispatch.to_string(),
    }
}

fn is_red(share: Option<f64>, per_day: f64, thresholds: &Thresholds) -> bool {
    matches!(share, Some(s) if s < thresholds.product_share_floor_pct)
        || per_day > thresholds.mech_issues_per_day_ceiling
}

fn is_yellow(share: Option<f64>, per_day: f64, thresholds: &Thresholds) -> bool {
    matches!(share, Some(s) if s < thresholds.product_share_floor_pct * 1.2)
        || per_day > thresholds.mech_issues_per_day_ceiling * 0.8
}

/// Read the operator-ratified threshold decisions from the ledger. A key is
/// resolved when the decision exists and its value parses as f64. A ledger
/// open failure never fails the command: both defaults apply.
fn read_thresholds(repo_root: &Path) -> Thresholds {
    const SHARE_KEY: &str = "fleet.health.product-share-floor";
    const RATE_KEY: &str = "fleet.health.mech-issues-per-day-ceiling";

    let mut share_resolved = false;
    let mut rate_resolved = false;
    let mut share = 50.0;
    let mut rate = 9.0;

    if let Ok(ledger) = edda_ledger::Ledger::open(repo_root) {
        if let Ok(branch) = ledger.head_branch() {
            if let Ok(Some(decision)) = ledger.find_active_decision(&branch, SHARE_KEY) {
                if let Ok(v) = decision.value.trim().parse::<f64>() {
                    share = v;
                    share_resolved = true;
                }
            }
            if let Ok(Some(decision)) = ledger.find_active_decision(&branch, RATE_KEY) {
                if let Ok(v) = decision.value.trim().parse::<f64>() {
                    rate = v;
                    rate_resolved = true;
                }
            }
        }
    }

    let source = match (share_resolved, rate_resolved) {
        (true, true) => "ledger",
        (false, false) => "default",
        _ => "mixed",
    };
    Thresholds {
        product_share_floor_pct: share,
        mech_issues_per_day_ceiling: rate,
        source: source.to_string(),
    }
}

pub(crate) fn gh_output(repo_root: &Path, args: &[&str]) -> Vec<u8> {
    let output = match std::process::Command::new("gh")
        .args(args)
        .current_dir(repo_root)
        .output()
    {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            eprintln!(
                "Error: gh {} failed: {}",
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("Error: gh {} failed: {err}", args.first().unwrap_or(&""));
            std::process::exit(1);
        }
    };
    output.stdout
}

fn collect_merged(
    repo_root: &Path,
    window_start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> (Vec<MergedPr>, u64) {
    let search = format!("merged:>={}", window_start.format("%Y-%m-%d"));
    let stdout = gh_output(
        repo_root,
        &[
            "pr",
            "list",
            "--state",
            "merged",
            "--search",
            &search,
            "--limit",
            "200",
            "--json",
            "number,mergedAt,files",
        ],
    );
    match parse_merged(&stdout, window_start, now) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("Error: gh pr list output unparseable: {err}");
            std::process::exit(1);
        }
    }
}

/// Parse `gh pr list --json number,mergedAt,files` output and keep the rows
/// whose `mergedAt` falls inside `[window_start, now]`. Returns the kept PRs
/// and the fetched row count before filtering.
pub fn parse_merged(
    bytes: &[u8],
    window_start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> anyhow::Result<(Vec<MergedPr>, u64)> {
    let prs: Vec<GhPr> = serde_json::from_slice(bytes)?;
    let fetched = prs.len() as u64;
    let kept = prs
        .into_iter()
        .filter_map(|pr| {
            let merged_at = DateTime::parse_from_rfc3339(&pr.merged_at)
                .ok()?
                .with_timezone(&Utc);
            if merged_at < window_start || merged_at > now {
                return None;
            }
            Some(MergedPr {
                number: pr.number,
                merged_at,
                paths: pr.files.into_iter().map(|f| f.path).collect(),
            })
        })
        .collect();
    Ok((kept, fetched))
}

fn collect_issues(
    repo_root: &Path,
    window_start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> (Vec<IssueRow>, u64) {
    let search = format!("created:>={}", window_start.format("%Y-%m-%d"));
    let stdout = gh_output(
        repo_root,
        &[
            "issue",
            "list",
            "--state",
            "all",
            "--search",
            &search,
            "--limit",
            "300",
            "--json",
            "number,createdAt,body",
        ],
    );
    match parse_issues(&stdout, window_start, now) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("Error: gh issue list output unparseable: {err}");
            std::process::exit(1);
        }
    }
}

/// Parse `gh issue list --json number,createdAt,body` output and keep the
/// rows whose `createdAt` falls inside `[window_start, now]`. Returns the
/// kept issues and the fetched row count before filtering.
pub fn parse_issues(
    bytes: &[u8],
    window_start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> anyhow::Result<(Vec<IssueRow>, u64)> {
    let issues: Vec<GhIssue> = serde_json::from_slice(bytes)?;
    let fetched = issues.len() as u64;
    let kept = issues
        .into_iter()
        .filter_map(|issue| {
            let created_at = DateTime::parse_from_rfc3339(&issue.created_at)
                .ok()?
                .with_timezone(&Utc);
            if created_at < window_start || created_at > now {
                return None;
            }
            Some(IssueRow {
                number: issue.number,
                created_at,
                body: issue.body,
            })
        })
        .collect();
    Ok((kept, fetched))
}

fn print_and_exit(output: &str, code: i32) -> ! {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    if let Err(err) = stdout
        .write_all(output.as_bytes())
        .and_then(|()| stdout.flush())
    {
        eprintln!("Error: stdout write failed: {err}");
        std::process::exit(1);
    }
    std::process::exit(code);
}

fn exit_code(status: &str) -> i32 {
    match status {
        "YELLOW" => 3,
        "RED" => 4,
        _ => 0,
    }
}

fn share_display(share: Option<f64>) -> String {
    match share {
        Some(s) => format!("{s:.1}"),
        None => "n/a".to_string(),
    }
}

/// The default text report. The text, JSON, and one-line formats are
/// intended for the follow-up digest adapter (#1025) and the ordering layer
/// (#1015); no in-repo consumer calls them yet.
pub fn render_text(h: &Health) -> String {
    let window_days = h.window_days;
    let product = h.merged_prs.product;
    let total = h.merged_prs.total;
    let share = share_display(h.merged_prs.product_share_pct);
    let floor = h.thresholds.product_share_floor_pct;
    let mech = h.issues_opened.mechanism;
    let issue_total = h.issues_opened.total;
    let per_day = h.issues_opened.mechanism_per_day;
    let ceiling = h.thresholds.mech_issues_per_day_ceiling;
    let last_24h = h.issues_opened.mechanism_last_24h;
    let source = h.thresholds.source.as_str();
    let status = h.status.as_str();
    let dispatch = h.mechanism_dispatch.as_str();
    let pr_fetched = h.merged_prs.fetched;
    let issue_fetched = h.issues_opened.fetched;
    let mut text = format!(
        "fleet health — last {window_days}d\n\
         merged PRs: product {product}/{total} ({share}% ; floor {floor}%)\n\
         issues opened: mechanism {mech}/{issue_total} ({per_day:.1}/day ; ceiling {ceiling}/day ; last 24h {last_24h})\n\
         sample: PRs {pr_fetched}/{PR_CAP}, issues {issue_fetched}/{ISSUE_CAP}\n\
         thresholds: {source}\n\
         status: {status} — mechanism dispatch {dispatch}"
    );
    if h.merged_prs.truncated || h.issues_opened.truncated {
        text.push_str("\nwarning: sample truncated — do not trust status for the freeze decision");
    }
    text
}

/// One-line summary for digest embedding.
pub fn render_line(h: &Health) -> String {
    let status = h.status.as_str();
    let share = share_display(h.merged_prs.product_share_pct);
    let floor = h.thresholds.product_share_floor_pct;
    let per_day = h.issues_opened.mechanism_per_day;
    let ceiling = h.thresholds.mech_issues_per_day_ceiling;
    let dispatch = h.mechanism_dispatch.as_str();
    let source = h.thresholds.source.as_str();
    let mut line = format!(
        "health: {status} | product share {share}% (floor {floor}%) | mechanism issues {per_day:.1}/day (ceiling {ceiling}/day) | thresholds {source} | mechanism dispatch {dispatch}"
    );
    if h.merged_prs.truncated || h.issues_opened.truncated {
        line.push_str(" | TRUNCATED");
    }
    line
}

/// The live health status, for the ordering layer (GH-1015). Reusing this
/// seam keeps one definition of "mechanism" and one definition of RED.
pub(crate) fn live_health_status(repo_root: &Path, window: u32) -> String {
    let now = Utc::now();
    let window_start = now - Duration::days(i64::from(window));
    let (merged, fetched_prs) = collect_merged(repo_root, window_start, now);
    let (issues, fetched_issues) = collect_issues(repo_root, window_start, now);
    let thresholds = read_thresholds(repo_root);
    compute(
        &merged,
        &issues,
        now,
        window,
        thresholds,
        fetched_prs,
        fetched_issues,
    )
    .status
}

/// CLI entry point.
pub fn run(cmd: FleetCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        FleetCmd::Health { window, json, line } => run_health(window, json, line, repo_root),
        FleetCmd::Order { args } => crate::cmd_fleet_order::run(args, repo_root),
    }
}

/// Prints, then exits with the status code: 0 GREEN, 3 YELLOW, 4 RED;
/// 1 error, 2 usage.
fn run_health(window: u32, json: bool, line: bool, repo_root: &Path) -> anyhow::Result<()> {
    if json && line {
        eprintln!("Error: --json and --line are mutually exclusive");
        std::process::exit(2);
    }
    if window == 0 {
        eprintln!("Error: --window must be at least 1");
        std::process::exit(2);
    }

    let now = Utc::now();
    let window_start = now - Duration::days(i64::from(window));
    let (merged, fetched_prs) = collect_merged(repo_root, window_start, now);
    let (issues, fetched_issues) = collect_issues(repo_root, window_start, now);
    let thresholds = read_thresholds(repo_root);
    let health = compute(
        &merged,
        &issues,
        now,
        window,
        thresholds,
        fetched_prs,
        fetched_issues,
    );

    let code = exit_code(&health.status);
    if json {
        let rendered = serde_json::to_string_pretty(&health).map_err(anyhow::Error::from)?;
        print_and_exit(&format!("{rendered}\n"), code);
    } else if line {
        print_and_exit(&format!("{}\n", render_line(&health)), code);
    } else {
        print_and_exit(&format!("{}\n", render_text(&health)), code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap()
    }

    fn days_ago(n: i64) -> DateTime<Utc> {
        now() - Duration::days(n)
    }

    fn pr(number: u64, paths: &[&str]) -> MergedPr {
        MergedPr {
            number,
            merged_at: days_ago(1),
            paths: paths.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn product_prs(count: u64) -> Vec<MergedPr> {
        (1..=count)
            .map(|i| pr(i, &["crates/edda-cli/src/main.rs"]))
            .collect()
    }

    fn mechanism_prs(count: u64) -> Vec<MergedPr> {
        (1..=count)
            .map(|i| pr(i, &["scripts/fleet/next-issue.sh"]))
            .collect()
    }

    fn issue(number: u64, body: &str, created_at: DateTime<Utc>) -> IssueRow {
        IssueRow {
            number,
            created_at,
            body: body.to_string(),
        }
    }

    fn mech_issue_body() -> String {
        "## Predicted surface\n\
         `scripts/fleet/next-issue.sh`\n\
         prose mentions `none` and no other tokens\n\
         ## doneWhen\n\
         done.\n"
            .to_string()
    }

    fn defaults() -> Thresholds {
        Thresholds {
            product_share_floor_pct: 50.0,
            mech_issues_per_day_ceiling: 9.0,
            source: "default".to_string(),
        }
    }

    #[test]
    fn classify_path_rules() {
        assert_eq!(classify_path("crates/edda-cli/src/main.rs"), Class::Product);
        assert_eq!(classify_path("sdk/python/x.py"), Class::Product);
        assert_eq!(
            classify_path("scripts/fleet/next-issue.sh"),
            Class::Mechanism
        );
        assert_eq!(classify_path("docs/fleet/rules.md"), Class::Mechanism);
        assert_eq!(classify_path(".github/workflows/ci.yml"), Class::Mechanism);
        assert_eq!(classify_path("REVIEW.md"), Class::Mechanism);
        assert_eq!(classify_path("docs/reference/cli.md"), Class::Other);
        assert_eq!(classify_path("./crates/a.rs"), Class::Product);
    }

    #[test]
    fn classify_paths_majority_tie_and_empty() {
        let majority = vec![
            "crates/a.rs".to_string(),
            "crates/b.rs".to_string(),
            "scripts/c.sh".to_string(),
        ];
        assert_eq!(classify_paths(&majority), Class::Product);

        let tie = vec!["crates/a.rs".to_string(), "scripts/b.sh".to_string()];
        assert_eq!(classify_paths(&tie), Class::Product);

        assert_eq!(classify_paths(&[]), Class::Other);
    }

    #[test]
    fn surface_paths_extracts_paths_in_order() {
        let body = "## What happened\n\
                    prose before\n\
                    ## Predicted surface\n\
                    `crates/edda-cli/src/cmd_fleet.rs`\n\
                    `none` is not a path\n\
                    more prose `docs/fleet/rules.md` inline\n\
                    ## doneWhen\n\
                    done.\n";
        assert_eq!(
            surface_paths(body),
            vec![
                "crates/edda-cli/src/cmd_fleet.rs".to_string(),
                "docs/fleet/rules.md".to_string(),
            ]
        );
    }

    #[test]
    fn compute_green_with_healthy_mix() {
        let merged: Vec<MergedPr> = product_prs(8).into_iter().chain(mechanism_prs(2)).collect();
        let issues = vec![issue(1, &mech_issue_body(), days_ago(2))];
        let h = compute(
            &merged,
            &issues,
            now(),
            7,
            defaults(),
            merged.len() as u64,
            issues.len() as u64,
        );
        assert_eq!(h.status, "GREEN");
        assert_eq!(h.mechanism_dispatch, "open");
        assert_eq!(h.merged_prs.product, 8);
        assert_eq!(h.merged_prs.total, 10);
    }

    #[test]
    fn compute_red_when_product_share_below_floor() {
        let merged: Vec<MergedPr> = product_prs(2).into_iter().chain(mechanism_prs(8)).collect();
        let h = compute(&merged, &[], now(), 7, defaults(), merged.len() as u64, 0);
        assert_eq!(h.status, "RED");
        assert_eq!(h.mechanism_dispatch, "freeze");
    }

    #[test]
    fn compute_red_when_mechanism_issue_rate_exceeds_ceiling() {
        let issues: Vec<IssueRow> = (1..=10)
            .map(|i| issue(i, &mech_issue_body(), now() - Duration::hours(i as i64)))
            .collect();
        let h = compute(
            &product_prs(10),
            &issues,
            now(),
            1,
            defaults(),
            10,
            issues.len() as u64,
        );
        assert!(h.issues_opened.mechanism_per_day > 9.0);
        assert_eq!(h.status, "RED");
        assert_eq!(h.mechanism_dispatch, "freeze");
    }

    #[test]
    fn compute_yellow_between_floor_and_warning_band() {
        let merged: Vec<MergedPr> = product_prs(11)
            .into_iter()
            .chain(mechanism_prs(9))
            .collect();
        let h = compute(&merged, &[], now(), 7, defaults(), merged.len() as u64, 0);
        let share = h.merged_prs.product_share_pct.unwrap();
        assert!((share - 55.0).abs() < 1e-9);
        assert_eq!(h.status, "YELLOW");
        assert_eq!(h.mechanism_dispatch, "open");
    }

    #[test]
    fn compute_zero_merged_prs_share_is_none_and_not_red_on_share() {
        let h = compute(&[], &[], now(), 7, defaults(), 0, 0);
        assert_eq!(h.merged_prs.product_share_pct, None);
        assert_ne!(h.status, "RED");
    }

    #[test]
    fn health_json_serializes_required_keys() {
        let merged: Vec<MergedPr> = product_prs(8).into_iter().chain(mechanism_prs(2)).collect();
        let issues = vec![issue(1, &mech_issue_body(), days_ago(2))];
        let h = compute(
            &merged,
            &issues,
            now(),
            7,
            defaults(),
            merged.len() as u64,
            issues.len() as u64,
        );
        let json = serde_json::to_string(&h).unwrap();
        for key in [
            "status",
            "mechanism_dispatch",
            "merged_prs",
            "issues_opened",
            "thresholds",
            "window_days",
        ] {
            assert!(json.contains(&format!("\"{key}\"")), "missing key: {key}");
        }
    }

    // Inline fixtures shaped exactly like `gh pr list --json number,mergedAt,files`
    // and `gh issue list --json number,createdAt,body` output.
    // now() = 2026-09-06T12:00:00Z; window_start = now() - 7d = 2026-08-30T12:00:00Z.
    const PRS_JSON: &str = r#"[
        {"number": 1, "mergedAt": "2026-09-05T12:00:00Z",
         "files": [{"path": "crates/edda-cli/src/main.rs"}, {"path": "sdk/python/x.py"}]},
        {"number": 2, "mergedAt": "2026-08-30T11:59:59Z",
         "files": [{"path": "scripts/keep.sh"}]},
        {"number": 3, "mergedAt": "2026-09-06T12:00:01Z",
         "files": [{"path": "docs/after.md"}]}
    ]"#;

    const ISSUES_JSON: &str = r###"[
        {"number": 10, "createdAt": "2026-09-05T12:00:00Z",
         "body": "## Predicted surface\n`scripts/fleet/next-issue.sh`\n## doneWhen\ndone.\n"},
        {"number": 11, "createdAt": "2026-08-30T11:59:59Z",
         "body": "## Predicted surface\n`none`\n"},
        {"number": 12, "createdAt": "2026-09-06T12:00:01Z",
         "body": "## Predicted surface\n`none`\n"}
    ]"###;

    fn window_start() -> DateTime<Utc> {
        now() - Duration::days(7)
    }

    #[test]
    fn parse_merged_keeps_only_rows_inside_the_window() {
        let (prs, fetched) = parse_merged(PRS_JSON.as_bytes(), window_start(), now()).unwrap();
        assert_eq!(fetched, 3);
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 1);
    }

    #[test]
    fn parse_merged_kept_paths_classify_product() {
        let (prs, _) = parse_merged(PRS_JSON.as_bytes(), window_start(), now()).unwrap();
        assert_eq!(prs[0].paths.len(), 2);
        assert_eq!(classify_path(&prs[0].paths[0]), Class::Product);
        assert_eq!(classify_path(&prs[0].paths[1]), Class::Product);
        assert_eq!(classify_paths(&prs[0].paths), Class::Product);
    }

    #[test]
    fn parse_issues_keeps_only_rows_inside_the_window() {
        let (issues, fetched) =
            parse_issues(ISSUES_JSON.as_bytes(), window_start(), now()).unwrap();
        assert_eq!(fetched, 3);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 10);
        assert_eq!(
            classify_paths(&surface_paths(&issues[0].body)),
            Class::Mechanism
        );
    }

    #[test]
    fn parse_malformed_json_is_err() {
        let bad: &[u8] = br#"[{"number": 1, "mergedAt": "#;
        assert!(parse_merged(bad, window_start(), now()).is_err());
        assert!(parse_issues(bad, window_start(), now()).is_err());
    }
}
