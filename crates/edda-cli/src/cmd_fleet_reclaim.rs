//! GH-1093: `edda fleet reclaim` — typed, fail-closed artifact reclamation.
//!
//! This is the product destination `scripts/fleet/reclaim-merged.sh` names in
//! its own header. The judgement the shell encoded as one `if` ladder becomes
//! typed here: every input fact is a struct, every KEEP reason an enum
//! variant, and every ladder arm a pure function from typed facts to a
//! [`Verdict`]. Rendering lives in exactly one place, so the ladder and the
//! printed row cannot drift.
//!
//! Fail-closed is the property the whole design protects. An artifact whose PR
//! state, tree state or live-peer state cannot be established is KEEPped, not
//! reclaimed. A branch is only deleted after a post-delete re-read proves it
//! is gone; a re-read that fails is reported unverified and fails the run
//! (exit 4) rather than manufacturing a receipt from a push's exit code.
//!
//! Exit: 0 ran; 2 usage; 3 the PR table was unreadable; 4 a post-delete
//! re-read could not be verified.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Flags for `edda fleet reclaim`.
#[derive(clap::Args)]
pub struct ReclaimArgs {
    /// Perform removals; without this flag the run is a dry run
    #[arg(long)]
    pub apply: bool,
    /// Keep a worktree directory basename or branch name exactly (repeatable)
    #[arg(long, value_name = "NAME")]
    pub protect: Vec<String>,
    /// Maximum number of PRs to read from `gh`
    #[arg(long, default_value_t = 2000)]
    pub pr_limit: u64,
}

/// What the ladder decided for one worktree or branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Keep(KeepReason),
    Reclaim(ReclaimReason),
}

/// Every reason an item was KEEPped. Parameterised variants carry the fact
/// that produced them; rendering is in [`KeepReason::render`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepReason {
    MainCheckout,
    NestedInMainCheckout,
    RunningFromHere,
    Locked,
    Protected,
    Prunable,
    Detached,
    PrAmbiguous,
    NoPr,
    PrState(String),
    TreeState(String),
    LivenessUnreadable,
    LivePeer(String),
    DefaultBranch,
    CheckedOut,
    LocalAheadOfPr,
    RemoteMovedSinceMerge,
    LocalKept(Box<KeepReason>),
}

/// Why an item is eligible for reclamation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReclaimReason {
    TreeClean,
    LocalTip,
    RemoteTip,
}

impl KeepReason {
    /// The exact reason string the transitional shell printed.
    pub fn render(&self) -> String {
        match self {
            KeepReason::MainCheckout => "main-checkout".to_string(),
            KeepReason::NestedInMainCheckout => "nested-in-main-checkout".to_string(),
            KeepReason::RunningFromHere => "running-from-here".to_string(),
            KeepReason::Locked => "locked".to_string(),
            KeepReason::Protected => "protected".to_string(),
            KeepReason::Prunable => "prunable — run git worktree prune".to_string(),
            KeepReason::Detached => "detached — no branch, no PR to judge by".to_string(),
            KeepReason::PrAmbiguous => "pr-ambiguous — branch name reused across PRs".to_string(),
            KeepReason::NoPr => "no-pr".to_string(),
            KeepReason::PrState(state) => format!("pr-{state}"),
            KeepReason::TreeState(tree) => format!("tree-{tree}"),
            KeepReason::LivenessUnreadable => {
                "liveness-unreadable — live peer state could not be read".to_string()
            }
            KeepReason::LivePeer(name) => format!("live-peer {name}"),
            KeepReason::DefaultBranch => "default-branch".to_string(),
            KeepReason::CheckedOut => "checked-out".to_string(),
            KeepReason::LocalAheadOfPr => "local-ahead-of-pr".to_string(),
            KeepReason::RemoteMovedSinceMerge => "remote-moved-since-merge".to_string(),
            KeepReason::LocalKept(reason) => format!("local-kept — {}", reason.render()),
        }
    }
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Keep(_) => "KEEP",
            Verdict::Reclaim(_) => "RECLAIM",
        }
    }

    pub fn is_reclaim(&self) -> bool {
        matches!(self, Verdict::Reclaim(_))
    }

    pub fn reason(&self) -> String {
        match self {
            Verdict::Keep(reason) => reason.render(),
            Verdict::Reclaim(ReclaimReason::TreeClean) => "pr-merged, tree clean".to_string(),
            Verdict::Reclaim(ReclaimReason::LocalTip) => "pr-merged, tip = merged head".to_string(),
            Verdict::Reclaim(ReclaimReason::RemoteTip) => {
                "pr-merged, remote tip = merged head".to_string()
            }
        }
    }
}

/// The tree state the shell read for each worktree before any verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeState {
    Clean,
    Dirty,
    Missing,
    Error,
}

impl TreeState {
    pub fn as_str(self) -> &'static str {
        match self {
            TreeState::Clean => "clean",
            TreeState::Dirty => "dirty",
            TreeState::Missing => "missing",
            TreeState::Error => "error",
        }
    }
}

/// One row of the PR table, keyed by its head branch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrRow {
    pub number: String,
    pub state: String,
    pub head_oid: String,
    pub merge_oid: String,
}

/// The PR table with its per-branch multiplicity preserved: a branch reused
/// across PRs has no single state, so it is never collapsed to one row.
#[derive(Debug, Clone, Default)]
pub struct PrIndex {
    counts: HashMap<String, usize>,
    first: HashMap<String, PrRow>,
}

impl PrIndex {
    pub fn from_rows(rows: Vec<(String, PrRow)>) -> Self {
        let mut counts = HashMap::new();
        let mut first = HashMap::new();
        for (branch, row) in rows {
            let count = counts.entry(branch.clone()).or_insert(0usize);
            *count += 1;
            if *count == 1 {
                first.insert(branch, row);
            }
        }
        Self { counts, first }
    }

    pub fn count(&self, branch: &str) -> usize {
        self.counts.get(branch).copied().unwrap_or(0)
    }

    pub fn lookup(&self, branch: &str) -> PrLookup<'_> {
        // `first` is the source of truth for "a row was recorded"; the count
        // only distinguishes a unique row from a reused name. Reading it this
        // way keeps the function panic-free without an unreachable branch.
        match self.first.get(branch) {
            None => PrLookup::NoPr,
            Some(row) if self.count(branch) == 1 => PrLookup::Single(row),
            Some(_) => PrLookup::Ambiguous,
        }
    }
}

/// A branch's PR state as the ladder sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrLookup<'a> {
    NoPr,
    Ambiguous,
    Single(&'a PrRow),
}

/// Live peer sessions keyed by the branch they hold. `known == false` means
/// the liveness surface could not be read at all — the fail-closed state.
#[derive(Debug, Clone, Default)]
pub struct LivePeers {
    pub known: bool,
    map: HashMap<String, String>,
}

impl LivePeers {
    pub fn unknown() -> Self {
        Self {
            known: false,
            map: HashMap::new(),
        }
    }

    pub fn get(&self, branch: &str) -> Option<&str> {
        self.map.get(branch).map(String::as_str)
    }
}

/// One record of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
    pub locked: bool,
    pub prunable: bool,
}

/// Typed facts the worktree ladder is total over.
pub struct WorktreeFacts<'a> {
    pub path: &'a str,
    pub branch: Option<&'a str>,
    pub locked: bool,
    pub prunable: bool,
    pub tree: TreeState,
    pub pr: PrLookup<'a>,
    pub live: Option<&'a str>,
    pub liveness_known: bool,
    pub protected: bool,
    pub main_path: &'a str,
    pub self_path: &'a str,
}

/// Typed facts the branch ladders are total over.
pub struct BranchFacts<'a> {
    pub branch: &'a str,
    pub tip: Option<&'a str>,
    pub remote_sha: Option<&'a str>,
    pub checked_out: bool,
    pub pr: PrLookup<'a>,
    pub live: Option<&'a str>,
    pub liveness_known: bool,
    pub default_branch: &'a str,
    pub protected: bool,
}

/// `path` is nested inside `main` (the shell's `"$main"/*` case).
fn is_nested(path: &str, main: &str) -> bool {
    path.strip_prefix(main)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// The final path component, matching the shell's `${path##*/}`.
fn basename(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => &path[index + 1..],
        None => path,
    }
}

fn is_protected(protect: &[String], name: &str) -> bool {
    protect.iter().any(|candidate| candidate == name)
}

/// The worktree ladder, first match wins (mirrors `reclaim-merged.sh`).
pub fn worktree_verdict(f: &WorktreeFacts<'_>) -> Verdict {
    if f.path == f.main_path {
        return Verdict::Keep(KeepReason::MainCheckout);
    }
    if is_nested(f.path, f.main_path) {
        return Verdict::Keep(KeepReason::NestedInMainCheckout);
    }
    if f.path == f.self_path {
        return Verdict::Keep(KeepReason::RunningFromHere);
    }
    if f.locked {
        return Verdict::Keep(KeepReason::Locked);
    }
    if f.protected {
        return Verdict::Keep(KeepReason::Protected);
    }
    if f.prunable || f.tree == TreeState::Missing {
        return Verdict::Keep(KeepReason::Prunable);
    }
    if f.branch.is_none() {
        return Verdict::Keep(KeepReason::Detached);
    }
    match f.pr {
        PrLookup::Ambiguous => return Verdict::Keep(KeepReason::PrAmbiguous),
        PrLookup::NoPr => return Verdict::Keep(KeepReason::NoPr),
        PrLookup::Single(row) if row.state != "MERGED" => {
            return Verdict::Keep(KeepReason::PrState(row.state.clone()));
        }
        PrLookup::Single(_) => {}
    }
    if f.tree != TreeState::Clean {
        return Verdict::Keep(KeepReason::TreeState(f.tree.as_str().to_string()));
    }
    if !f.liveness_known {
        return Verdict::Keep(KeepReason::LivenessUnreadable);
    }
    if let Some(name) = f.live {
        return Verdict::Keep(KeepReason::LivePeer(name.to_string()));
    }
    Verdict::Reclaim(ReclaimReason::TreeClean)
}

/// The local-branch ladder. `live-peer` precedes `checked-out` on purpose: a
/// live owner's branch is normally checked out in its own worktree, and naming
/// the owner is the fact GH-1094 asked to protect.
pub fn local_verdict(f: &BranchFacts<'_>) -> Verdict {
    if f.branch == f.default_branch {
        return Verdict::Keep(KeepReason::DefaultBranch);
    }
    if let Some(name) = f.live {
        return Verdict::Keep(KeepReason::LivePeer(name.to_string()));
    }
    if f.checked_out {
        return Verdict::Keep(KeepReason::CheckedOut);
    }
    if f.protected {
        return Verdict::Keep(KeepReason::Protected);
    }
    match f.pr {
        PrLookup::Ambiguous => return Verdict::Keep(KeepReason::PrAmbiguous),
        PrLookup::NoPr => return Verdict::Keep(KeepReason::NoPr),
        PrLookup::Single(row) if row.state != "MERGED" => {
            return Verdict::Keep(KeepReason::PrState(row.state.clone()));
        }
        PrLookup::Single(row) if f.tip != Some(row.head_oid.as_str()) => {
            return Verdict::Keep(KeepReason::LocalAheadOfPr);
        }
        PrLookup::Single(_) => {}
    }
    if !f.liveness_known {
        return Verdict::Keep(KeepReason::LivenessUnreadable);
    }
    Verdict::Reclaim(ReclaimReason::LocalTip)
}

/// The remote-branch ladder. `local_keep` is the local row's KEEP reason when
/// one exists: whatever kept the local ref is also a reason to leave somewhere
/// to push.
pub fn remote_verdict(f: &BranchFacts<'_>, local_keep: Option<&KeepReason>) -> Verdict {
    if f.branch == f.default_branch {
        return Verdict::Keep(KeepReason::DefaultBranch);
    }
    if f.protected {
        return Verdict::Keep(KeepReason::Protected);
    }
    match f.pr {
        PrLookup::Ambiguous => return Verdict::Keep(KeepReason::PrAmbiguous),
        PrLookup::NoPr => return Verdict::Keep(KeepReason::NoPr),
        PrLookup::Single(row) if row.state != "MERGED" => {
            return Verdict::Keep(KeepReason::PrState(row.state.clone()));
        }
        PrLookup::Single(row) if f.remote_sha != Some(row.head_oid.as_str()) => {
            return Verdict::Keep(KeepReason::RemoteMovedSinceMerge);
        }
        PrLookup::Single(_) => {}
    }
    if !f.liveness_known {
        return Verdict::Keep(KeepReason::LivenessUnreadable);
    }
    if let Some(name) = f.live {
        return Verdict::Keep(KeepReason::LivePeer(name.to_string()));
    }
    if let Some(reason) = local_keep {
        return Verdict::Keep(KeepReason::LocalKept(Box::new(reason.clone())));
    }
    Verdict::Reclaim(ReclaimReason::RemoteTip)
}

/// The `PR` and `STATE` columns: only a branch with exactly one PR row has
/// values; every other multiplicity prints `-`.
fn pr_cells(pr: PrLookup<'_>) -> (String, String) {
    match pr {
        PrLookup::Single(row) => (format!("#{}", row.number), row.state.clone()),
        _ => ("-".to_string(), "-".to_string()),
    }
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// Parse the TSV `gh pr list --jq` emits: branch, number, state, head oid,
/// merge oid.
pub fn parse_pr_rows(bytes: &[u8]) -> Vec<(String, PrRow)> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            let branch = fields.next().unwrap_or("").to_string();
            let row = PrRow {
                number: fields.next().unwrap_or("").to_string(),
                state: fields.next().unwrap_or("").to_string(),
                head_oid: fields.next().unwrap_or("").to_string(),
                merge_oid: fields.next().unwrap_or("-").to_string(),
            };
            (branch, row)
        })
        .collect()
}

/// Parse `git ls-remote --heads origin` into `(name, sha)`.
pub fn parse_remote_refs(bytes: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let sha = fields.next()?;
            let name = fields.next()?.strip_prefix("refs/heads/")?;
            Some((name.to_string(), sha.to_string()))
        })
        .collect()
}

/// A failed `ls-remote` reads as an empty table: no remote branch to reclaim,
/// the conservative answer.
pub fn remote_table_outcome(success: bool, stdout: &[u8]) -> Vec<(String, String)> {
    if success {
        parse_remote_refs(stdout)
    } else {
        Vec::new()
    }
}

pub fn interpret_remote_refs(result: io::Result<Output>) -> Vec<(String, String)> {
    match result {
        Ok(out) => remote_table_outcome(out.status.success(), &out.stdout),
        Err(_) => remote_table_outcome(false, b""),
    }
}

/// Parse the post-delete local re-read: one refname per line.
pub fn parse_local_reread(bytes: &[u8]) -> HashSet<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse the post-delete remote re-read into the set of surviving names.
pub fn parse_remote_reread(bytes: &[u8]) -> HashSet<String> {
    parse_remote_refs(bytes)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// Parse `edda peers --json`. `None` means the table could not be read: it is
/// not JSON, or it has no `sessions` array. That is the fail-closed signal.
pub fn parse_live_peers(bytes: &[u8]) -> Option<LivePeers> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let sessions = value.get("sessions")?.as_array()?;
    let mut map: HashMap<String, String> = HashMap::new();
    for session in sessions {
        if session.get("stale") != Some(&serde_json::Value::Bool(false)) {
            continue;
        }
        let branch = session
            .get("branch")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let label = session
            .get("label")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let name = if label.is_empty() {
            let session_id = session
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            session_id.chars().take(8).collect::<String>()
        } else {
            label.to_string()
        };
        map.entry(branch).or_insert(name);
    }
    Some(LivePeers { known: true, map })
}

/// The shared mapping from an exit status and body to a liveness verdict. A
/// non-zero exit or an unreadable body mean the same thing: unknown.
pub fn classify_peers(success: bool, stdout: &[u8]) -> LivePeers {
    if success {
        parse_live_peers(stdout).unwrap_or_else(LivePeers::unknown)
    } else {
        LivePeers::unknown()
    }
}

/// Interpret a completed `edda peers --json` call, including a spawn failure.
pub fn interpret_peers(result: io::Result<Output>) -> LivePeers {
    match result {
        Ok(out) => classify_peers(out.status.success(), &out.stdout),
        Err(_) => classify_peers(false, b""),
    }
}

/// Parse `git worktree list --porcelain`.
pub fn parse_worktrees(bytes: &[u8]) -> Vec<WorktreeEntry> {
    let mut entries: Vec<WorktreeEntry> = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in String::from_utf8_lossy(bytes).lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            current = Some(WorktreeEntry {
                path: rest.to_string(),
                branch: None,
                head: String::new(),
                locked: false,
                prunable: false,
            });
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            if let Some(entry) = current.as_mut() {
                entry.head = rest.to_string();
            }
        } else if let Some(rest) = line.strip_prefix("branch ") {
            if let Some(entry) = current.as_mut() {
                entry.branch = Some(rest.strip_prefix("refs/heads/").unwrap_or(rest).to_string());
            }
        } else if line.starts_with("locked") {
            if let Some(entry) = current.as_mut() {
                entry.locked = true;
            }
        } else if line.starts_with("prunable") {
            if let Some(entry) = current.as_mut() {
                entry.prunable = true;
            }
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    entries
}

/// The result of a post-delete re-read.
#[derive(Debug)]
pub enum ReRead {
    Ok(HashSet<String>),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteKind {
    Local,
    Remote,
}

/// Turn a post-delete re-read into receipts, KEPT lines and a verify flag.
/// Nothing can be receipted from a re-read that failed.
pub fn classify_deletions(
    list: &[(String, String, String)],
    re_read: ReRead,
    kind: DeleteKind,
) -> (Vec<String>, Vec<String>, bool) {
    let (label, prefix, fail_note) = match kind {
        DeleteKind::Local => (
            "local branch",
            "",
            "git for-each-ref failed re-reading refs/heads after delete",
        ),
        DeleteKind::Remote => (
            "remote branch",
            "origin/",
            "git ls-remote failed re-reading origin after delete",
        ),
    };
    match re_read {
        ReRead::Failed(err) => {
            let kept = list
                .iter()
                .map(|(branch, _, _)| {
                    format!("KEPT {label}\t{prefix}{branch}\tunverified — {fail_note}: {err}")
                })
                .collect();
            (Vec::new(), kept, true)
        }
        ReRead::Ok(present) => {
            let mut receipts = Vec::new();
            let mut kept = Vec::new();
            for (branch, pr, merge) in list {
                if present.contains(branch) {
                    kept.push(format!(
                        "KEPT {label}\t{prefix}{branch}\tstill present after delete"
                    ));
                } else {
                    receipts.push(format!(
                        "reclaimed {label}\t{prefix}{branch}\tpr={pr}\tsquash={merge}"
                    ));
                }
            }
            (receipts, kept, false)
        }
    }
}

/// A reclaimable worktree: the values the worktree receipt needs.
struct WtReclaim {
    path: String,
    branch: String,
    pr: String,
    merge: String,
}

/// A reclaimable branch: the values the branch receipt needs.
struct BrReclaim {
    branch: String,
    pr: String,
    merge: String,
}

struct Ctx<'a> {
    root: &'a Path,
    prs: &'a PrIndex,
    live: &'a LivePeers,
    remote: &'a [(String, String)],
    protect: &'a [String],
    main_path: &'a str,
    self_path: &'a str,
}

impl Ctx<'_> {
    fn worktree_pass(&self, worktrees: &[WorktreeEntry]) -> Vec<WtReclaim> {
        let mut reclaims = Vec::new();
        for worktree in worktrees {
            let tree = tree_state(&worktree.path, self.root);
            let branch = worktree.branch.as_deref();
            let pr = match branch {
                Some(name) => self.prs.lookup(name),
                None => PrLookup::NoPr,
            };
            let live = branch.and_then(|name| self.live.get(name));
            let protected = is_protected(self.protect, basename(&worktree.path))
                || branch.is_some_and(|name| is_protected(self.protect, name));
            let facts = WorktreeFacts {
                path: &worktree.path,
                branch,
                locked: worktree.locked,
                prunable: worktree.prunable,
                tree,
                pr,
                live,
                liveness_known: self.live.known,
                protected,
                main_path: self.main_path,
                self_path: self.self_path,
            };
            let verdict = worktree_verdict(&facts);
            let (pr_display, state_display) = pr_cells(pr);
            println!(
                "{}\tworktree\t{}\t{}\t{}\t{}\t{}\t{}",
                verdict.label(),
                worktree.path,
                branch.unwrap_or("-"),
                pr_display,
                state_display,
                tree.as_str(),
                verdict.reason()
            );
            if verdict.is_reclaim() {
                if let (Some(name), PrLookup::Single(row)) = (branch, pr) {
                    reclaims.push(WtReclaim {
                        path: worktree.path.clone(),
                        branch: name.to_string(),
                        pr: format!("#{}", row.number),
                        merge: row.merge_oid.clone(),
                    });
                }
            }
        }
        reclaims
    }

    fn branch_pass(&self, reclaimed_worktrees: &[WtReclaim]) -> (Vec<BrReclaim>, Vec<BrReclaim>) {
        // The shell read this set through a pipeline (`git worktree list | sed`),
        // so a failed read was tolerated there — keep that tolerance, unlike the
        // first read in `run`, which the shell's `set -e` aborted on.
        let mut checked_out: HashSet<String> = worktree_list_raw(self.root)
            .map(|bytes| parse_worktrees(&bytes))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| entry.branch)
            .collect();
        for reclaim in reclaimed_worktrees {
            checked_out.remove(&reclaim.branch);
        }

        // `git for-each-ref` was a direct command in the shell: `set -e` aborts
        // the whole run when it fails, so branch deletion never starts. Mirror
        // that abort rather than continuing with an empty local table.
        let local = match collect_local_refs(self.root) {
            Ok(rows) => rows,
            Err(detail) => {
                eprintln!("reclaim-merged: git for-each-ref failed — {detail}");
                std::process::exit(1);
            }
        };
        let mut union: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
        for (name, tip) in local {
            union.entry(name).or_default().0 = Some(tip);
        }
        for (name, sha) in self.remote {
            union.entry(name.clone()).or_default().1 = Some(sha.clone());
        }

        let default = default_branch(self.root);
        let mut local_reclaims = Vec::new();
        let mut remote_reclaims = Vec::new();
        for (branch, (tip, remote_sha)) in &union {
            let pr = self.prs.lookup(branch);
            let (pr_display, state_display) = pr_cells(pr);
            let facts = BranchFacts {
                branch,
                tip: tip.as_deref(),
                remote_sha: remote_sha.as_deref(),
                checked_out: checked_out.contains(branch),
                pr,
                live: self.live.get(branch),
                liveness_known: self.live.known,
                default_branch: &default,
                protected: is_protected(self.protect, branch),
            };

            let mut local_keep: Option<KeepReason> = None;
            if let Some(tip) = tip {
                let verdict = local_verdict(&facts);
                println!(
                    "{}\tlocal-branch\t{}\t{}\t{}\t{}\t{}\t{}",
                    verdict.label(),
                    branch,
                    branch,
                    pr_display,
                    state_display,
                    tip,
                    verdict.reason()
                );
                match verdict {
                    Verdict::Keep(reason) => local_keep = Some(reason),
                    Verdict::Reclaim(_) => {
                        if let PrLookup::Single(row) = pr {
                            local_reclaims.push(BrReclaim {
                                branch: branch.clone(),
                                pr: format!("#{}", row.number),
                                merge: row.merge_oid.clone(),
                            });
                        }
                    }
                }
            }

            if let Some(sha) = remote_sha {
                let verdict = remote_verdict(&facts, local_keep.as_ref());
                println!(
                    "{}\tremote-branch\torigin/{}\t{}\t{}\t{}\t{}\t{}",
                    verdict.label(),
                    branch,
                    branch,
                    pr_display,
                    state_display,
                    sha,
                    verdict.reason()
                );
                if verdict.is_reclaim() {
                    if let PrLookup::Single(row) = pr {
                        remote_reclaims.push(BrReclaim {
                            branch: branch.clone(),
                            pr: format!("#{}", row.number),
                            merge: row.merge_oid.clone(),
                        });
                    }
                }
            }
        }
        (local_reclaims, remote_reclaims)
    }
}

fn tree_state(path: &str, root: &Path) -> TreeState {
    if !Path::new(path).is_dir() {
        return TreeState::Missing;
    }
    match Command::new(git_binary())
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
    {
        Ok(out) if out.status.success() => {
            if out.stdout.is_empty() {
                TreeState::Clean
            } else {
                TreeState::Dirty
            }
        }
        _ => TreeState::Error,
    }
}

fn worktree_list_raw(root: &Path) -> Result<Vec<u8>, String> {
    let out = Command::new(git_binary())
        .args(["worktree", "list", "--porcelain"])
        .current_dir(root)
        .output()
        .map_err(|err| err.to_string())?;
    if !out.status.success() {
        return Err(first_line(&out.stderr));
    }
    Ok(out.stdout)
}

fn collect_local_refs(root: &Path) -> Result<Vec<(String, String)>, String> {
    let format = "%(refname:short)\t%(objectname)".to_string();
    let out = Command::new(git_binary())
        .args(["for-each-ref", &format!("--format={format}"), "refs/heads"])
        .current_dir(root)
        .output()
        .map_err(|err| err.to_string())?;
    if !out.status.success() {
        return Err(first_line(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?;
            let tip = fields.next()?;
            Some((name.to_string(), tip.to_string()))
        })
        .collect())
}

fn default_branch(root: &Path) -> String {
    if let Ok(out) = Command::new(git_binary())
        .args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .current_dir(root)
        .output()
    {
        if out.status.success() {
            let value = String::from_utf8_lossy(&out.stdout);
            let value = value.trim();
            if let Some(stripped) = value.strip_prefix("origin/") {
                return stripped.to_string();
            }
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    "main".to_string()
}

fn count_lines(root: &Path, args: &[&str]) -> usize {
    match Command::new(git_binary())
        .args(args)
        .current_dir(root)
        .output()
    {
        Ok(out) => out.stdout.iter().filter(|&&byte| byte == b'\n').count(),
        Err(_) => 0,
    }
}

fn count_worktrees(root: &Path) -> usize {
    count_lines(root, &["worktree", "list"])
}

fn count_branches(root: &Path) -> usize {
    count_lines(root, &["for-each-ref", "--format=x", "refs/heads"])
}

/// Resolve a subprocess binary through an `EDDA_*_BIN` override, falling back
/// to the PATH name the transitional shell used. This mirrors the repo's
/// `EDDA_GH_BIN` / `EDDA_CODEX_BIN` / `EDDA_PI_BIN` convention and is why the
/// cross-platform fixture can point the binary at a stub: Windows PATH search
/// finds `.exe` files only, never a `.bat` stub.
fn binary_override(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(fallback))
}

fn gh_binary() -> PathBuf {
    binary_override("EDDA_GH_BIN", "gh")
}

fn peers_binary() -> PathBuf {
    binary_override("EDDA_PEERS_BIN", "edda")
}

fn git_binary() -> PathBuf {
    binary_override("EDDA_GIT_BIN", "git")
}

fn gh_args(pr_limit: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "list".to_string(),
        "--state".to_string(),
        "all".to_string(),
        "--limit".to_string(),
        pr_limit.to_string(),
        "--json".to_string(),
        "number,state,headRefName,headRefOid,mergeCommit".to_string(),
        "--jq".to_string(),
        r#".[] | [.headRefName, (.number|tostring), .state, .headRefOid, (.mergeCommit.oid // "-")] | @tsv"#
            .to_string(),
    ]
}

/// The shared mapping from a `gh pr list` exit status to rows or the first
/// stderr line. An unreadable table is an error; there is no partial read.
pub fn pr_table_outcome(
    success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<Vec<(String, PrRow)>, String> {
    if success {
        Ok(parse_pr_rows(stdout))
    } else {
        Err(first_line(stderr))
    }
}

fn collect_prs(root: &Path, pr_limit: u64) -> Result<Vec<(String, PrRow)>, String> {
    match Command::new(gh_binary())
        .args(gh_args(pr_limit))
        .current_dir(root)
        .output()
    {
        Ok(out) => pr_table_outcome(out.status.success(), &out.stdout, &out.stderr),
        Err(_) => Err(String::new()),
    }
}

fn worktree_root(repo_root: &Path) -> anyhow::Result<PathBuf> {
    let out = Command::new(git_binary())
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(repo_root)
        .output()
        .map_err(|err| anyhow::anyhow!("git rev-parse --show-toplevel: {err}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git rev-parse --show-toplevel failed: {}",
            first_line(&out.stderr)
        );
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        anyhow::bail!("git rev-parse --show-toplevel returned nothing");
    }
    Ok(PathBuf::from(path))
}

fn read_after_deletion(root: &Path, kind: DeleteKind) -> ReRead {
    match kind {
        DeleteKind::Local => match Command::new(git_binary())
            .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
            .current_dir(root)
            .output()
        {
            Ok(out) if out.status.success() => ReRead::Ok(parse_local_reread(&out.stdout)),
            Ok(out) => ReRead::Failed(first_line(&out.stderr)),
            Err(err) => ReRead::Failed(err.to_string()),
        },
        DeleteKind::Remote => match Command::new(git_binary())
            .args(["ls-remote", "--heads", "origin"])
            .current_dir(root)
            .output()
        {
            Ok(out) if out.status.success() => ReRead::Ok(parse_remote_reread(&out.stdout)),
            Ok(out) => ReRead::Failed(first_line(&out.stderr)),
            Err(err) => ReRead::Failed(err.to_string()),
        },
    }
}

/// Delete in batches of at most 50, then verify by re-reading rather than by
/// trusting the delete command's exit code.
fn run_deletion(root: &Path, list: &[BrReclaim], kind: DeleteKind, verify_failed: &mut bool) {
    if list.is_empty() {
        return;
    }
    for chunk in list.chunks(50) {
        let refs: Vec<&str> = chunk
            .iter()
            .map(|reclaim| reclaim.branch.as_str())
            .collect();
        let mut command = Command::new(git_binary());
        match kind {
            DeleteKind::Local => {
                command.args(["branch", "-D"]);
            }
            DeleteKind::Remote => {
                command.args(["push", "origin", "--delete"]);
            }
        }
        let _ = command.args(&refs).current_dir(root).output();
    }

    let tuples: Vec<(String, String, String)> = list
        .iter()
        .map(|reclaim| {
            (
                reclaim.branch.clone(),
                reclaim.pr.clone(),
                reclaim.merge.clone(),
            )
        })
        .collect();
    let re_read = read_after_deletion(root, kind);
    let (receipts, kept, failed) = classify_deletions(&tuples, re_read, kind);
    for receipt in receipts {
        println!("{receipt}");
    }
    for line in kept {
        eprintln!("{line}");
    }
    if failed {
        *verify_failed = true;
    }
}

/// CLI entry point. Mirrors the transitional shell's output byte-for-byte.
#[allow(clippy::too_many_lines)]
pub fn run(args: ReclaimArgs, repo_root: &Path) -> anyhow::Result<()> {
    let root = worktree_root(repo_root)?;
    let root_str = root.to_string_lossy().into_owned();

    let pr_rows = match collect_prs(&root, args.pr_limit) {
        Ok(rows) => rows,
        Err(detail) => {
            eprintln!("reclaim-merged: gh pr list failed — {detail}");
            std::process::exit(3);
        }
    };
    let prs = PrIndex::from_rows(pr_rows);
    let remote = interpret_remote_refs(
        Command::new(git_binary())
            .args(["ls-remote", "--heads", "origin"])
            .current_dir(&root)
            .output(),
    );
    // The shell ran `git worktree list --porcelain >...` directly, so `set -e`
    // aborted the run when it failed. An unreadable worktree table means the
    // checkout state is unknown; aborting reclaims nothing, like the shell.
    let worktrees = match worktree_list_raw(&root) {
        Ok(bytes) => parse_worktrees(&bytes),
        Err(detail) => {
            eprintln!("reclaim-merged: git worktree list failed — {detail}");
            std::process::exit(1);
        }
    };
    let live = interpret_peers(
        Command::new(peers_binary())
            .args(["peers", "--json"])
            .current_dir(&root)
            .output(),
    );
    if !live.known {
        eprintln!(
            "reclaim-merged: live peer state unavailable — every candidate is KEEP (fail closed)"
        );
    }

    let main_path = worktrees
        .first()
        .map(|entry| entry.path.clone())
        .unwrap_or_default();
    let ctx = Ctx {
        root: &root,
        prs: &prs,
        live: &live,
        remote: &remote,
        protect: &args.protect,
        main_path: &main_path,
        self_path: &root_str,
    };

    let before_worktrees = count_worktrees(&root);
    let before_branches = count_branches(&root);
    println!(
        "reclaim-merged: before  worktrees={before_worktrees}  branches={before_branches}  prs={}",
        prs.count_total()
    );
    if !args.apply {
        println!("reclaim-merged: dry run — nothing is removed without --apply");
    }
    println!();
    println!("VERDICT\tKIND\tITEM\tBRANCH\tPR\tSTATE\tTREE/SHA\tREASON");

    let reclaimed_worktrees = ctx.worktree_pass(&worktrees);

    if args.apply && !reclaimed_worktrees.is_empty() {
        println!();
        for reclaim in &reclaimed_worktrees {
            match Command::new(git_binary())
                .args(["worktree", "remove", &reclaim.path])
                .current_dir(&root)
                .output()
            {
                Ok(out) if out.status.success() => println!(
                    "reclaimed worktree\t{}\tbranch={}\tpr={}\tsquash={}",
                    reclaim.path, reclaim.branch, reclaim.pr, reclaim.merge
                ),
                Ok(out) => eprintln!(
                    "KEPT worktree\t{}\tgit worktree remove refused: {}",
                    reclaim.path,
                    first_line(&out.stderr)
                ),
                Err(err) => eprintln!(
                    "KEPT worktree\t{}\tgit worktree remove refused: {err}",
                    reclaim.path
                ),
            }
        }
    }

    println!();
    let (local_reclaims, remote_reclaims) = ctx.branch_pass(&reclaimed_worktrees);

    let mut verify_failed = false;
    if args.apply {
        println!();
        run_deletion(
            &root,
            &local_reclaims,
            DeleteKind::Local,
            &mut verify_failed,
        );
        run_deletion(
            &root,
            &remote_reclaims,
            DeleteKind::Remote,
            &mut verify_failed,
        );
    }

    println!();
    if args.apply {
        println!(
            "reclaim-merged: after   worktrees={}  branches={} (before: {before_worktrees} / {before_branches})",
            count_worktrees(&root),
            count_branches(&root)
        );
    } else {
        println!(
            "reclaim-merged: dry run — {} worktree(s), {} local branch(es), {} remote branch(es) would be reclaimed by --apply",
            reclaimed_worktrees.len(),
            local_reclaims.len(),
            remote_reclaims.len()
        );
    }

    if verify_failed {
        eprintln!(
            "reclaim-merged: a post-delete re-read could not be verified — see KEPT/unverified lines above"
        );
        std::process::exit(4);
    }
    Ok(())
}

impl PrIndex {
    fn count_total(&self) -> usize {
        self.counts.values().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &str, head: &str) -> PrRow {
        PrRow {
            number: "101".to_string(),
            state: state.to_string(),
            head_oid: head.to_string(),
            merge_oid: "squash".to_string(),
        }
    }

    fn pr_index(entries: &[(&str, &str, &str)]) -> PrIndex {
        // (branch, state, head_oid)
        PrIndex::from_rows(
            entries
                .iter()
                .map(|(branch, state, head)| (branch.to_string(), row(state, head)))
                .collect(),
        )
    }

    fn worktree<'a>(
        path: &'a str,
        branch: Option<&'a str>,
        tree: TreeState,
        pr: PrLookup<'a>,
        main: &'a str,
    ) -> WorktreeFacts<'a> {
        WorktreeFacts {
            path,
            branch,
            locked: false,
            prunable: false,
            tree,
            pr,
            live: None,
            liveness_known: true,
            protected: false,
            main_path: main,
            self_path: "/self",
        }
    }

    fn branch<'a>(
        name: &'a str,
        tip: Option<&'a str>,
        remote: Option<&'a str>,
        pr: PrLookup<'a>,
    ) -> BranchFacts<'a> {
        BranchFacts {
            branch: name,
            tip,
            remote_sha: remote,
            checked_out: false,
            pr,
            live: None,
            liveness_known: true,
            default_branch: "main",
            protected: false,
        }
    }

    #[test]
    fn worktree_unconditional_protections() {
        let prs = pr_index(&[("/x", "MERGED", "abc")]);
        let row = prs.lookup("/x");
        assert_eq!(
            worktree_verdict(&worktree(
                "/main",
                Some("x"),
                TreeState::Clean,
                row,
                "/main"
            )),
            Verdict::Keep(KeepReason::MainCheckout)
        );
        assert_eq!(
            worktree_verdict(&worktree(
                "/main/sub",
                Some("x"),
                TreeState::Clean,
                row,
                "/main"
            )),
            Verdict::Keep(KeepReason::NestedInMainCheckout)
        );
        let mut facts = worktree("/self", Some("x"), TreeState::Clean, row, "/main");
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Keep(KeepReason::RunningFromHere)
        );
        facts.self_path = "/other";
        facts.locked = true;
        assert_eq!(worktree_verdict(&facts), Verdict::Keep(KeepReason::Locked));
        facts.locked = false;
        facts.protected = true;
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Keep(KeepReason::Protected)
        );
    }

    #[test]
    fn worktree_prunable_and_detached() {
        let prs = pr_index(&[("/x", "MERGED", "abc")]);
        let row = prs.lookup("/x");
        let mut facts = worktree("/x", Some("x"), TreeState::Clean, row, "/main");
        facts.prunable = true;
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Keep(KeepReason::Prunable)
        );
        facts.prunable = false;
        facts.tree = TreeState::Missing;
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Keep(KeepReason::Prunable)
        );
        facts.tree = TreeState::Clean;
        facts.branch = None;
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Keep(KeepReason::Detached)
        );
    }

    #[test]
    fn worktree_pr_and_tree_degenerate_forms() {
        let none = PrIndex::default();
        let base = worktree("/x", Some("x"), TreeState::Clean, none.lookup("x"), "/main");
        assert_eq!(worktree_verdict(&base), Verdict::Keep(KeepReason::NoPr));

        let ambiguous = PrIndex::from_rows(vec![
            ("x".to_string(), row("MERGED", "abc")),
            ("x".to_string(), row("OPEN", "def")),
        ]);
        let facts = worktree(
            "/x",
            Some("x"),
            TreeState::Clean,
            ambiguous.lookup("x"),
            "/main",
        );
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Keep(KeepReason::PrAmbiguous)
        );

        let open = pr_index(&[("x", "OPEN", "abc")]);
        let facts = worktree("/x", Some("x"), TreeState::Clean, open.lookup("x"), "/main");
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Keep(KeepReason::PrState("OPEN".to_string()))
        );

        let merged = pr_index(&[("x", "MERGED", "abc")]);
        for (tree, expected) in [(TreeState::Dirty, "dirty"), (TreeState::Error, "error")] {
            let facts = worktree("/x", Some("x"), tree, merged.lookup("x"), "/main");
            assert_eq!(
                worktree_verdict(&facts),
                Verdict::Keep(KeepReason::TreeState(expected.to_string()))
            );
        }
    }

    #[test]
    fn worktree_liveness_and_live_peer() {
        let merged = pr_index(&[("x", "MERGED", "abc")]);
        let mut facts = worktree(
            "/x",
            Some("x"),
            TreeState::Clean,
            merged.lookup("x"),
            "/main",
        );
        facts.liveness_known = false;
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Keep(KeepReason::LivenessUnreadable)
        );
        facts.liveness_known = true;
        facts.live = Some("peer-lane");
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Keep(KeepReason::LivePeer("peer-lane".to_string()))
        );
        facts.live = None;
        assert_eq!(
            worktree_verdict(&facts),
            Verdict::Reclaim(ReclaimReason::TreeClean)
        );
    }

    #[test]
    fn local_ladder_covers_every_arm() {
        let merged = pr_index(&[("x", "MERGED", "abc"), ("main", "MERGED", "abc")]);
        let main = branch("main", Some("abc"), None, merged.lookup("main"));
        assert_eq!(
            local_verdict(&main),
            Verdict::Keep(KeepReason::DefaultBranch)
        );

        let live = BranchFacts {
            checked_out: true,
            live: Some("peer-lane"),
            ..branch("x", Some("abc"), None, merged.lookup("x"))
        };
        assert_eq!(
            local_verdict(&live),
            Verdict::Keep(KeepReason::LivePeer("peer-lane".to_string()))
        );

        let checked_out = BranchFacts {
            checked_out: true,
            ..branch("x", Some("abc"), None, merged.lookup("x"))
        };
        assert_eq!(
            local_verdict(&checked_out),
            Verdict::Keep(KeepReason::CheckedOut)
        );

        let protected = BranchFacts {
            protected: true,
            ..branch("x", Some("abc"), None, merged.lookup("x"))
        };
        assert_eq!(
            local_verdict(&protected),
            Verdict::Keep(KeepReason::Protected)
        );

        let ambiguous = PrIndex::from_rows(vec![
            ("x".to_string(), row("MERGED", "abc")),
            ("x".to_string(), row("OPEN", "def")),
        ]);
        assert_eq!(
            local_verdict(&branch("x", Some("abc"), None, ambiguous.lookup("x"))),
            Verdict::Keep(KeepReason::PrAmbiguous)
        );

        let none = PrIndex::default();
        assert_eq!(
            local_verdict(&branch("x", Some("abc"), None, none.lookup("x"))),
            Verdict::Keep(KeepReason::NoPr)
        );

        let open = pr_index(&[("x", "OPEN", "abc")]);
        assert_eq!(
            local_verdict(&branch("x", Some("abc"), None, open.lookup("x"))),
            Verdict::Keep(KeepReason::PrState("OPEN".to_string()))
        );

        assert_eq!(
            local_verdict(&branch("x", Some("moved"), None, merged.lookup("x"))),
            Verdict::Keep(KeepReason::LocalAheadOfPr)
        );

        let unreadable = BranchFacts {
            liveness_known: false,
            ..branch("x", Some("abc"), None, merged.lookup("x"))
        };
        assert_eq!(
            local_verdict(&unreadable),
            Verdict::Keep(KeepReason::LivenessUnreadable)
        );

        assert_eq!(
            local_verdict(&branch("x", Some("abc"), None, merged.lookup("x"))),
            Verdict::Reclaim(ReclaimReason::LocalTip)
        );
    }

    #[test]
    fn remote_ladder_covers_every_arm() {
        let merged = pr_index(&[("x", "MERGED", "abc"), ("main", "MERGED", "abc")]);
        assert_eq!(
            remote_verdict(
                &branch("main", None, Some("abc"), merged.lookup("main")),
                None
            ),
            Verdict::Keep(KeepReason::DefaultBranch)
        );

        let protected = BranchFacts {
            protected: true,
            ..branch("x", None, Some("abc"), merged.lookup("x"))
        };
        assert_eq!(
            remote_verdict(&protected, None),
            Verdict::Keep(KeepReason::Protected)
        );

        let ambiguous = PrIndex::from_rows(vec![
            ("x".to_string(), row("MERGED", "abc")),
            ("x".to_string(), row("OPEN", "def")),
        ]);
        assert_eq!(
            remote_verdict(&branch("x", None, Some("abc"), ambiguous.lookup("x")), None),
            Verdict::Keep(KeepReason::PrAmbiguous)
        );

        let none = PrIndex::default();
        assert_eq!(
            remote_verdict(&branch("x", None, Some("abc"), none.lookup("x")), None),
            Verdict::Keep(KeepReason::NoPr)
        );

        let open = pr_index(&[("x", "OPEN", "abc")]);
        assert_eq!(
            remote_verdict(&branch("x", None, Some("abc"), open.lookup("x")), None),
            Verdict::Keep(KeepReason::PrState("OPEN".to_string()))
        );

        assert_eq!(
            remote_verdict(&branch("x", None, Some("moved"), merged.lookup("x")), None),
            Verdict::Keep(KeepReason::RemoteMovedSinceMerge)
        );

        let unreadable = BranchFacts {
            liveness_known: false,
            ..branch("x", None, Some("abc"), merged.lookup("x"))
        };
        assert_eq!(
            remote_verdict(&unreadable, None),
            Verdict::Keep(KeepReason::LivenessUnreadable)
        );

        let live = BranchFacts {
            live: Some("peer-lane"),
            ..branch("x", None, Some("abc"), merged.lookup("x"))
        };
        assert_eq!(
            remote_verdict(&live, None),
            Verdict::Keep(KeepReason::LivePeer("peer-lane".to_string()))
        );

        let local_reason = KeepReason::CheckedOut;
        assert_eq!(
            remote_verdict(
                &branch("x", None, Some("abc"), merged.lookup("x")),
                Some(&local_reason)
            ),
            Verdict::Keep(KeepReason::LocalKept(Box::new(KeepReason::CheckedOut)))
        );

        assert_eq!(
            remote_verdict(&branch("x", None, Some("abc"), merged.lookup("x")), None),
            Verdict::Reclaim(ReclaimReason::RemoteTip)
        );
    }

    #[test]
    fn live_peer_naming_label_and_session_fallback() {
        let peers = parse_live_peers(
            br#"{"sessions":[
                {"stale":false,"branch":"a","label":"peer-lane","session_id":"deadbeefcafe"},
                {"stale":false,"branch":"b","label":"","session_id":"deadbeefcafe"},
                {"stale":false,"branch":"c","session_id":"deadbeefcafe"},
                {"stale":true,"branch":"d","label":"stale-peer","session_id":"aaaa"}
            ]}"#,
        )
        .expect("sessions present");
        assert!(peers.known);
        assert_eq!(peers.get("a"), Some("peer-lane"));
        assert_eq!(peers.get("b"), Some("deadbeef"));
        assert_eq!(peers.get("c"), Some("deadbeef"));
        assert_eq!(peers.get("d"), None);
    }

    #[test]
    fn interpret_peers_all_unreadable_sources() {
        // Spawn failure (a real io::Error the caller could not spawn with).
        let spawn = interpret_peers(Err(io::Error::new(io::ErrorKind::NotFound, "no edda")));
        assert!(!spawn.known);
        // Non-zero exit.
        assert!(!classify_peers(false, b"{\"sessions\":[]}").known);
        // Unparseable JSON.
        assert!(!classify_peers(true, b"not json at all").known);
        // Missing `sessions`.
        assert!(!classify_peers(true, b"{\"other\":1}").known);
        // Valid empty set.
        let empty = classify_peers(true, b"{\"sessions\":[]}");
        assert!(empty.known);
        assert_eq!(empty.get("x"), None);
    }

    #[test]
    fn parse_live_peers_missing_sessions_is_none() {
        assert!(parse_live_peers(b"{\"sessions\":{}}").is_none());
        assert!(parse_live_peers(b"garbage").is_none());
        assert!(parse_live_peers(b"{\"sessions\":[]}").is_some());
    }

    #[test]
    fn pr_table_failure_is_err_with_first_stderr_line() {
        assert_eq!(
            pr_table_outcome(false, b"", b"boom\nsecond\n"),
            Err("boom".to_string())
        );
        assert_eq!(pr_table_outcome(true, b"", b""), Ok(Vec::new()));
    }

    #[test]
    fn remote_ref_read_failure_is_empty() {
        let failed: Vec<(String, String)> =
            interpret_remote_refs(Err(io::Error::new(io::ErrorKind::NotFound, "no git")));
        assert!(failed.is_empty());
        assert!(remote_table_outcome(false, b"abc\trefs/heads/x\n").is_empty());
        let present = remote_table_outcome(true, b"abc\trefs/heads/x\n");
        assert_eq!(present, vec![("x".to_string(), "abc".to_string())]);
    }

    #[test]
    fn classify_deletions_still_present_and_gone() {
        let list = vec![("a".to_string(), "#1".to_string(), "s1".to_string())];
        let (receipts, kept, failed) =
            classify_deletions(&list, ReRead::Ok(HashSet::new()), DeleteKind::Local);
        assert_eq!(
            receipts,
            vec!["reclaimed local branch\ta\tpr=#1\tsquash=s1"]
        );
        assert!(kept.is_empty());
        assert!(!failed);

        let mut present = HashSet::new();
        present.insert("a".to_string());
        let (receipts, kept, failed) =
            classify_deletions(&list, ReRead::Ok(present), DeleteKind::Remote);
        assert!(receipts.is_empty());
        assert_eq!(
            kept,
            vec!["KEPT remote branch\torigin/a\tstill present after delete"]
        );
        assert!(!failed);
    }

    #[test]
    fn classify_deletions_failed_reread_fabricates_nothing() {
        let list = vec![
            ("a".to_string(), "#1".to_string(), "s1".to_string()),
            ("b".to_string(), "#2".to_string(), "s2".to_string()),
        ];
        let (receipts, kept, failed) = classify_deletions(
            &list,
            ReRead::Failed("origin unreachable".to_string()),
            DeleteKind::Remote,
        );
        assert!(receipts.is_empty(), "a failed re-read must prove nothing");
        assert_eq!(kept.len(), 2);
        assert!(
            kept[0].starts_with("KEPT remote branch\torigin/a\tunverified — git ls-remote failed")
        );
        assert!(kept[0].ends_with(": origin unreachable"));
        assert!(failed);
    }

    #[test]
    fn parse_worktrees_reads_branch_locked_prunable_detached() {
        let raw = b"worktree /main\nHEAD aaa\nbranch refs/heads/main\n\n\
                    worktree /x\nHEAD bbb\nbranch refs/heads/x\nlocked\nprunable\n\n\
                    worktree /detached\nHEAD ccc\ndetached\n";
        let entries = parse_worktrees(raw);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "/main");
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(entries[1].locked);
        assert!(entries[1].prunable);
        assert_eq!(entries[2].branch, None);
        assert_eq!(entries[2].head, "ccc");
    }
}
