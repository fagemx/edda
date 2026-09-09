//! `edda ratify` — confer authority on a decision.
//!
//! Three forms, mutually exclusive, distinguishable forever after in
//! `edda log` and `edda ask` by the prefix each writes into `ratified_by`:
//!
//! | form | `ratified_by` | who is asserting |
//! |---|---|---|
//! | `edda ratify <KEY> [--by WHO]` | `<WHO>` or the session label | a person (GH-401) |
//! | `edda ratify <KEY> --evidence pr#N@sha` | `evidence:pr#N@sha` | a merged PR (GH-764) |
//! | `edda ratify --by-rule <RULE>` | `rule:<RULE>` | a rule in this binary (GH-761) |
//!
//! The typed prefixes are the point of both issues: before them, "a merged
//! PR made this binding" and "a controller's regex made this binding" were
//! both free text in `--by`, so the ledger's authority tier was only as
//! trustworthy as whatever string a caller happened to spell.
//!
//! Exit codes follow `claim-check.exit-codes=0/1/2`: 0 success — including
//! the no-op when a key is already binding; 1 the named key has no active
//! decision; 2 malformed input.

mod evidence;
mod rule;
#[cfg(test)]
mod tests;

pub use evidence::parse_evidence;
pub use rule::{evaluate, validate_citation, Candidate, Outcome, Verdict, RULE_CITED_AUTHORITY};

use anyhow::Context;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

#[derive(clap::Args, Debug)]
pub struct RatifyArgs {
    /// Decision key to ratify (e.g. "db.engine"). Omit it with --by-rule.
    pub key: Option<String>,
    /// Optional note recorded with the ratification
    #[arg(long)]
    pub note: Option<String>,
    /// Who ratified — recorded for audit; self-asserted, not verified
    /// (identity enforcement is a policy-layer concern). Defaults to the
    /// resolved session label.
    #[arg(long, conflicts_with_all = ["evidence", "by_rule"])]
    pub by: Option<String>,
    /// Evidence form: the merged PR that made this decision binding, as
    /// `pr#<N>@<40-hex sha>` (GH-764)
    #[arg(long, conflicts_with_all = ["by", "by_rule"])]
    pub evidence: Option<String>,
    /// Rule form: sweep every active, unratified decision with a named rule
    /// (today only `cited-authority`) (GH-761)
    #[arg(long = "by-rule", conflicts_with_all = ["by", "evidence"])]
    pub by_rule: Option<String>,
    /// With --by-rule: print the table without writing anything
    #[arg(long = "dry-run", requires = "by_rule")]
    pub dry_run: bool,
    /// With --by-rule: only judge these keys (repeatable). Bounds the sweep
    /// (GH-1066) — with this set, the unbounded-sweep size refusal below
    /// does not apply.
    #[arg(long = "key", requires = "by_rule")]
    pub keys: Vec<String>,
    /// With --by-rule: only judge decisions recorded on or after this date
    /// (`YYYY-MM-DD`). Bounds the sweep the same way `--key` does (GH-1066).
    #[arg(long, requires = "by_rule")]
    pub since: Option<String>,
    /// With --by-rule and neither --key nor --since: proceed even though the
    /// ratify set exceeds the safety threshold (GH-1066). Without it, an
    /// unbounded sweep that would ratify more than the threshold prints the
    /// table and counts, and writes nothing.
    #[arg(long, requires = "by_rule")]
    pub yes: bool,
    /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
    #[arg(long)]
    pub session: Option<String>,
}

/// Print a usage error and exit 2. Malformed input is not a runtime failure
/// to be wrapped in an anyhow chain — the caller mistyped, and a script
/// reading exit codes needs to tell that apart from "no such key" (1).
pub(crate) fn usage_exit(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(2)
}

pub fn run(repo_root: &Path, args: &RatifyArgs) -> anyhow::Result<()> {
    match (args.key.as_deref(), args.by_rule.as_deref()) {
        (Some(key), None) => ratify_one(repo_root, key, args),
        (None, Some(rule)) => sweep(repo_root, rule, args),
        (Some(_), Some(_)) => usage_exit(
            "--by-rule sweeps every eligible decision — drop the key argument, or drop --by-rule",
        ),
        (None, None) => usage_exit(
            "edda ratify needs a decision key, or --by-rule <RULE> (e.g. cited-authority)",
        ),
    }
}

/// One key, ratified by a person (`--by`) or by a merged PR (`--evidence`).
///
/// Ratification is a separate append-only fact (`decision_ratify` event),
/// never a mutation of the decision, so authority is conferred by a
/// deliberate act and is fully auditable via `ratified_by`.
///
/// Identity is not cryptographically enforced here — a session can record
/// any `ratified_by`. That enforcement is a policy-layer concern (GH-401
/// scope); this layer delivers the separation of act, the rendering split,
/// and the audit trail.
fn ratify_one(repo_root: &Path, key: &str, args: &RatifyArgs) -> anyhow::Result<()> {
    let key = key.trim();
    // Identity is only resolved when it is actually the answer: an evidence
    // or `--by` ratification says who conferred authority without consulting
    // the session at all.
    let ratified_by = match (args.evidence.as_deref(), args.by.as_deref()) {
        (Some(raw), _) => match parse_evidence(raw) {
            Ok(e) => e.ratified_by(),
            Err(msg) => usage_exit(&msg),
        },
        (None, Some(by)) => by.to_string(),
        (None, None) => {
            let project_id = edda_store::project_id(repo_root);
            crate::cmd_bridge::resolve_session_id(args.session.as_deref(), &project_id, "cli")?.1
        }
    };

    let ledger = edda_ledger::Ledger::open(repo_root).context("cmd_ratify: opening ledger")?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;

    // Only an existing active decision can be ratified. Exit 1: the key is
    // absent, which a caller must be able to distinguish from a typo (2).
    let Some(decision) = ledger.find_active_decision(&branch, key)? else {
        anyhow::bail!("no active decision for key '{key}' — nothing to ratify (see `edda ask`)");
    };

    // Already binding → exit 0 without writing. Re-ratifying would append a
    // second fact asserting what the ledger already says, so a post-merge
    // hook that runs twice on the same PR stays a no-op the second time.
    if let Some(info) = ledger.ratified_decisions_map()?.get(&decision.event_id) {
        println!(
            "'{key}' is already binding (by {}) — nothing written.",
            info.ratified_by
        );
        return Ok(());
    }

    append_ratify(&ledger, &branch, key, &ratified_by, args.note.as_deref())?;

    println!("Ratified '{key}' (by {ratified_by}) — now binding.");
    if let Some(n) = &args.note {
        println!("  note: {n}");
    }
    Ok(())
}

fn append_ratify(
    ledger: &edda_ledger::Ledger,
    branch: &str,
    key: &str,
    ratified_by: &str,
    note: Option<&str>,
) -> anyhow::Result<()> {
    let parent_hash = ledger.last_event_hash()?;
    let event = edda_core::event::new_decision_ratify_event(
        branch,
        parent_hash.as_deref(),
        key,
        ratified_by,
        note,
    )?;
    ledger.append_event(&event)?;
    let _ = edda_derive::rebuild_branch(ledger, branch);
    Ok(())
}

/// Default cap on an unbounded `--by-rule` sweep's ratify count before it
/// refuses without `--yes` (GH-1066): a sweep nobody scoped is a sweep
/// nobody dares run, so the default protects against acting on more than a
/// screenful of decisions sight-unseen. `--key`/`--since` bound the sweep
/// deliberately, so the threshold does not apply once either is given.
const RATIFY_THRESHOLD: usize = 20;

/// `--since <date>` must be `YYYY-MM-DD`: cheap to check at the boundary, so
/// a typo is a usage error (exit 2) rather than a silently-wrong comparison.
fn validate_since(s: &str) -> String {
    let bytes = s.as_bytes();
    let ok = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => true,
            _ => b.is_ascii_digit(),
        });
    if !ok {
        usage_exit(&format!("--since must be YYYY-MM-DD — got '{s}'"));
    }
    s.to_string()
}

/// `--by-rule <RULE>`: evaluate every active decision on this branch and
/// print one row per unratified key. `--dry-run` prints the same table and
/// writes nothing, so the operator can read what the machine is about to do
/// on their behalf before it does it.
///
/// `--key`/`--since` bound *which* verdicts are shown and acted on, not what
/// the rule sees while deciding: same-key and domain-guard comparisons
/// (rule.rs) still see every active decision on the branch, so scoping a
/// sweep can never blind it to an older, still-relevant ruling.
fn sweep(repo_root: &Path, rule_name: &str, args: &RatifyArgs) -> anyhow::Result<()> {
    if rule_name != RULE_CITED_AUTHORITY {
        usage_exit(&format!(
            "unknown rule '{rule_name}' — known rules: {RULE_CITED_AUTHORITY}"
        ));
    }
    let since = args.since.as_deref().map(validate_since);

    let ledger = edda_ledger::Ledger::open(repo_root).context("cmd_ratify: opening ledger")?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;

    let (candidates, binding) = collect(&ledger, &branch)?;
    let dates: HashMap<&str, &str> = candidates
        .iter()
        .map(|c| (c.key.as_str(), c.date.as_str()))
        .collect();
    let all_verdicts = evaluate(&candidates, &binding);

    let bounded = !args.keys.is_empty() || since.is_some();
    let verdicts: Vec<Verdict> = all_verdicts
        .into_iter()
        .filter(|v| args.keys.is_empty() || args.keys.iter().any(|k| k == &v.key))
        .filter(|v| {
            since
                .as_deref()
                .is_none_or(|s| dates.get(v.key.as_str()).is_some_and(|d| *d >= s))
        })
        .collect();

    let ratify_count = verdicts.iter().filter(|v| v.is_ratify()).count();
    if !args.dry_run && !bounded && !args.yes && ratify_count > RATIFY_THRESHOLD {
        print_table(&verdicts, rule_name, true);
        println!(
            "refusing: {ratify_count} decisions would ratify unscoped (threshold {RATIFY_THRESHOLD}) — bound with --key/--since, or pass --yes; nothing written."
        );
        return Ok(());
    }

    print_table(&verdicts, rule_name, args.dry_run);

    if args.dry_run {
        return Ok(());
    }
    for v in verdicts.iter().filter(|v| v.is_ratify()) {
        let Outcome::Ratify { citation } = &v.outcome else {
            continue;
        };
        let note = match &args.note {
            Some(n) => format!("{n} (cites {citation})"),
            None => format!("cites {citation}"),
        };
        append_ratify(
            &ledger,
            &branch,
            &v.key,
            &format!("rule:{rule_name}"),
            Some(&note),
        )?;
    }
    Ok(())
}

/// Build the rule's input from the ledger: every active decision on this
/// branch, its ledger order, its structured citations, and whether it is
/// already ratified — plus the set of keys currently binding, which is the
/// "cites a binding decision" arm of the rule.
fn collect(
    ledger: &edda_ledger::Ledger,
    branch: &str,
) -> anyhow::Result<(Vec<Candidate>, BTreeSet<String>)> {
    let ratified = ledger.ratified_decisions_map()?;
    let active: Vec<_> = ledger
        .active_decisions(None, None, None, None)?
        .into_iter()
        .filter(|d| d.branch == branch)
        .collect();

    // `cites` lives in the decision event payload, not in the projected
    // row, so it needs no schema migration and old ledgers stay readable.
    let wanted: BTreeSet<&str> = active.iter().map(|d| d.event_id.as_str()).collect();
    let mut cites_by_event: HashMap<String, Vec<String>> = HashMap::new();
    for e in ledger.iter_events_by_type("note")? {
        if !wanted.contains(e.event_id.as_str()) {
            continue;
        }
        if let Some(list) = e.payload["decision"]["cites"].as_array() {
            let cites: Vec<String> = list
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if !cites.is_empty() {
                cites_by_event.insert(e.event_id.clone(), cites);
            }
        }
    }

    let binding: BTreeSet<String> = active
        .iter()
        .filter(|d| ratified.contains_key(&d.event_id))
        .map(|d| d.key.clone())
        .collect();

    let mut candidates = Vec::with_capacity(active.len());
    for d in active {
        candidates.push(Candidate {
            cites: cites_by_event.remove(&d.event_id).unwrap_or_default(),
            ratified: ratified.contains_key(&d.event_id),
            order: ledger.rowid_for_event_id(&d.event_id)?.unwrap_or(0),
            date: d.ts.clone().unwrap_or_default(),
            key: d.key,
            reason: d.reason,
        });
    }
    candidates.sort_by_key(|c| c.order);
    // Every active row reaches the rule, same-key predecessors included:
    // `active_decisions` can return more than one active row for a `(branch,
    // key)` — `Ledger::ratified_decisions_map` groups them for exactly that
    // reason. Before GH-1066 this function collapsed to "keep only the
    // latest row per key" so an older row was invisible to the rule and its
    // supersession was silently assumed rather than shown; now the rule's
    // own same-key check (rule.rs) holds every predecessor explicitly and
    // names why, and only the newest row in a same-key chain can ever clear
    // that check and reach the citation check — so the write loop below
    // still appends at most one ratify event per key.
    Ok((candidates, binding))
}

fn print_table(verdicts: &[Verdict], rule_name: &str, dry_run: bool) {
    if verdicts.is_empty() {
        println!("No active unratified decisions — rule '{rule_name}' had nothing to judge.");
        return;
    }
    let width = verdicts
        .iter()
        .map(|v| v.key.len())
        .max()
        .unwrap_or(3)
        .max(3);
    println!("{:<width$}  {:<6}  why", "key", "action", width = width);
    for v in verdicts {
        let (action, why) = v.columns();
        println!("{:<width$}  {action:<6}  {why}", v.key, width = width);
    }
    let ratify_count = verdicts.iter().filter(|v| v.is_ratify()).count();
    let held = verdicts.len() - ratify_count;
    if dry_run {
        println!("dry run — would ratify {ratify_count}, hold {held} (rule: {rule_name}); nothing written.");
    } else {
        println!("ratified {ratify_count}, held {held} (rule: {rule_name}).");
    }
}
