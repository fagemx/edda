//! GH-1015: fleet order — deterministic, health-gated ready queue.
//!
//! `edda fleet order` replaces the controller's hand-written ordering plan
//! with a ranker any session can re-run: the same input yields the same output
//! byte-for-byte, and every rank carries the component contributions that
//! produced it, so the operator's remaining move is a veto at the queue head
//! rather than a re-derivation of the whole plan.
//!
//! Three properties the shell layer could not hold (per `fleet.mechanism-layer`
//! and `fleet.health-ordering`):
//!
//! * **Health coupling** — when `edda fleet health` reports RED, mechanism-class
//!   weight is zeroed. The one exception is an issue whose body cites a red
//!   run/gate link; without a link it is not a pipeline blocker.
//! * **Per-file collision** — collisions are the pairwise intersection of
//!   `## Predicted surface` paths, never labels (#1005). #671 and #685 both
//!   name `crates/edda-cli/src/main.rs` and share no label at all.
//! * **Freshness judgment in the product** (#970) — the applicability re-check
//!   runs against the pinned tree and this binary's own verb table, so a stale
//!   `edda` on `PATH` can no longer produce a false FAIL, and a path or command
//!   the issue itself declares it will create WARNs instead of FAILing.
//!
//! Computation is pure and network-free: `gh`, `git`, and the ledger are read
//! once at collection time into [`OrderInput`], and [`compute_order`] is a
//! total function over it. Flash criteria that would need a subprocess (the
//! brief render, the #945 dry-run validator) are emitted as named pending
//! checks on the row instead of being shelled out mid-computation or silently
//! skipped.

mod cli;
mod render;

pub use cli::{run, OrderArgs};
pub use render::{render_markdown, render_text};

use crate::cmd_fleet::{classify_paths, surface_paths, Class};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Ledger key holding the flash lane's surface-file ceiling.
const FLASH_CAP_KEY: &str = "fleet.order.flash-max-surface-files";
/// Applied when [`FLASH_CAP_KEY`] resolves to nothing parseable.
const FLASH_CAP_DEFAULT: usize = 3;
/// `REVIEW.md` §6: marks a check with no mechanical decision step.
const JUDGMENT_MARKER: &str = "[判斷]";

/// Score weights. Named constants because every one of them is quoted back in
/// the rendered decomposition, and a reader has to be able to check the sum.
const W_PRODUCT: i64 = 40;
const W_OTHER: i64 = 10;
const W_MECHANISM: i64 = 0;
const W_READY: i64 = 25;
const W_P0: i64 = 30;
const W_P1: i64 = 15;
const W_P2: i64 = 5;
const W_RED_RUN_BLOCKER: i64 = 50;
const W_COLLISION_PEER: i64 = -10;
const W_STALE: i64 = -100;

/// Flash criteria that need a subprocess and are therefore reported, not run.
const PENDING_BRIEF_RENDER: &str = "brief-render";
const PENDING_DISPATCH_DRY_RUN: &str = "dispatch-dry-run";

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct GhLabel {
    pub name: String,
}

/// One open issue as `gh issue list --json number,title,body,labels` returns
/// it. Also the fixture shape accepted by `--issues`.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenIssue {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub labels: Vec<GhLabel>,
}

/// Everything [`compute_order`] is allowed to read. Collected once, up front.
pub struct OrderInput {
    pub issues: Vec<OpenIssue>,
    /// Every path tracked at the pinned tree. Freshness resolves against this,
    /// never against the filesystem or a binary on `PATH`.
    pub tree_paths: BTreeSet<String>,
    /// Top-level verbs of *this* binary — and therefore of the pinned tree.
    pub known_verbs: BTreeSet<String>,
    /// `GREEN`, `YELLOW`, or `RED`, from `edda fleet health`.
    pub health_status: String,
    pub flash_max_surface_files: usize,
    /// `ledger` or `default`.
    pub flash_cap_source: String,
    pub now: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Lane a row routes to. Flash is the cheap runtime; controller means the row
/// carries judgment the mechanism must not resolve on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lane {
    Flash,
    Strong,
    Controller,
}

impl Lane {
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Flash => "flash",
            Lane::Strong => "strong",
            Lane::Controller => "controller",
        }
    }
}

/// Dispatchability of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Dispatchable now.
    Ready,
    /// Blocked behind a higher-ranked row that declares the same path.
    Hold,
    /// Mechanism-class under a RED health freeze, with no red-run evidence.
    Frozen,
    /// The freshness re-check FAILed against the pinned tree.
    Stale,
    /// Already in flight (`fleet:claimed`).
    Claimed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ready => "ready",
            Status::Hold => "hold",
            Status::Frozen => "frozen",
            Status::Stale => "stale",
            Status::Claimed => "claimed",
        }
    }
}

/// Freshness verdict for one referenced path or command. There is no `Pass`:
/// a reference that resolves produces no finding at all, so an empty findings
/// list is the fresh case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Unresolvable, or resolvable only once the issue is delivered.
    Warn,
    /// The reference is wrong about today's tree; the issue needs re-reading.
    Fail,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Warn => "warn",
            Verdict::Fail => "fail",
        }
    }
}

/// One freshness observation. `kind` is `path`, `command`, or `donewhen`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FreshnessFinding {
    pub verdict: Verdict,
    pub kind: String,
    pub target: String,
    pub note: String,
}

/// Every contribution to a row's rank. The fields sum to `total`; the renderer
/// prints the non-zero ones so a reader can check the arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Score {
    /// Path class of the declared surface: product / other / mechanism.
    pub class: i64,
    /// `fleet:ready` promotion.
    pub readiness: i64,
    /// P0 / P1 / P2 label.
    pub priority: i64,
    /// RED-freeze exception: the body cites a red run or gate link.
    pub blocker: i64,
    /// One penalty per peer declaring a shared surface path.
    pub collision: i64,
    /// The freshness re-check FAILed.
    pub freshness: i64,
    /// Zeroing term applied to frozen mechanism rows under RED health.
    pub frozen: i64,
    pub total: i64,
}

/// One ranked issue.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub rank: usize,
    pub number: u64,
    pub title: String,
    /// `product`, `mechanism`, or `other`.
    pub class: String,
    pub surface: Vec<String>,
    pub lane: Lane,
    /// Why the lane came out the way it did.
    pub lane_reasons: Vec<String>,
    /// Flash criteria that need a subprocess; the dispatcher runs these.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pending_checks: Vec<String>,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub collides_with: Vec<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub freshness: Vec<FreshnessFinding>,
    pub score: Score,
}

/// The full ordered queue. Serialized verbatim under `--json`.
#[derive(Debug, Clone, Serialize)]
pub struct Queue {
    pub generated_at: String,
    pub health_status: String,
    pub mechanism_dispatch: String,
    pub flash_max_surface_files: usize,
    pub flash_cap_source: String,
    pub total_issues: usize,
    pub rows: Vec<Row>,
}

// ---------------------------------------------------------------------------
// Computation
// ---------------------------------------------------------------------------

/// Per-issue facts derived before ranking. One pass means the collision matrix
/// and the scores see exactly the same surfaces.
struct Draft {
    number: u64,
    title: String,
    surface: Vec<String>,
    class: Class,
    labels: BTreeSet<String>,
    freshness: Vec<FreshnessFinding>,
    cites_red_run: bool,
    lane: Lane,
    lane_reasons: Vec<String>,
    pending_checks: Vec<String>,
}

/// Rank and route every issue in `input`. Pure, total and deterministic: the
/// sort key is `(score descending, issue number ascending)`, so equal scores
/// always fall back to the older issue and no input permutation can reorder
/// the result.
pub fn compute_order(input: &OrderInput) -> Queue {
    let frozen_health = input.health_status.eq_ignore_ascii_case("RED");
    let drafts: Vec<Draft> = input
        .issues
        .iter()
        .map(|issue| draft(issue, input))
        .collect();
    let collisions = collision_matrix(&drafts);

    let mut rows: Vec<Row> = drafts
        .iter()
        .map(|draft| {
            let peers = collisions.get(&draft.number).cloned().unwrap_or_default();
            row(draft, peers, frozen_health)
        })
        .collect();

    rows.sort_by(|a, b| {
        b.score
            .total
            .cmp(&a.score.total)
            .then(a.number.cmp(&b.number))
    });
    serialize_collisions(&mut rows);

    Queue {
        generated_at: input.now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        health_status: input.health_status.to_ascii_uppercase(),
        mechanism_dispatch: if frozen_health { "freeze" } else { "open" }.to_string(),
        flash_max_surface_files: input.flash_max_surface_files,
        flash_cap_source: input.flash_cap_source.clone(),
        total_issues: input.issues.len(),
        rows,
    }
}

fn draft(issue: &OpenIssue, input: &OrderInput) -> Draft {
    let surface = surface_paths(&issue.body);
    let labels: BTreeSet<String> = issue
        .labels
        .iter()
        .map(|label| label.name.trim().to_ascii_lowercase())
        .collect();
    let (lane, lane_reasons, pending_checks) = route(
        &surface,
        &labels,
        &issue.body,
        input.flash_max_surface_files,
    );
    Draft {
        number: issue.number,
        title: issue.title.trim().to_string(),
        class: classify_paths(&surface),
        labels,
        freshness: evaluate_freshness(&issue.body, &input.tree_paths, &input.known_verbs),
        cites_red_run: cites_red_run(&issue.body),
        surface,
        lane,
        lane_reasons,
        pending_checks,
    }
}

fn row(draft: &Draft, peers: BTreeSet<u64>, frozen_health: bool) -> Row {
    let mechanism = draft.class == Class::Mechanism;
    let stale = draft.freshness.iter().any(|f| f.verdict == Verdict::Fail);
    let frozen = frozen_health && mechanism && !draft.cites_red_run;

    let class = match draft.class {
        Class::Product => W_PRODUCT,
        Class::Mechanism => W_MECHANISM,
        Class::Other => W_OTHER,
    };
    let readiness = if draft.labels.contains("fleet:ready") {
        W_READY
    } else {
        0
    };
    let priority = if draft.labels.contains("p0") {
        W_P0
    } else if draft.labels.contains("p1") {
        W_P1
    } else if draft.labels.contains("p2") {
        W_P2
    } else {
        0
    };
    let blocker = if frozen_health && mechanism && draft.cites_red_run {
        W_RED_RUN_BLOCKER
    } else {
        0
    };
    let collision = W_COLLISION_PEER * peers.len() as i64;
    let freshness = if stale { W_STALE } else { 0 };
    let subtotal = class + readiness + priority + blocker + collision + freshness;
    // "mechanism weight is 0 under RED" taken literally: the zeroing term is
    // recorded as its own component so the decomposition still sums to total.
    let frozen_term = if frozen { -subtotal } else { 0 };

    let status = if stale {
        Status::Stale
    } else if frozen {
        Status::Frozen
    } else if draft.labels.contains("fleet:claimed") {
        Status::Claimed
    } else {
        Status::Ready
    };

    Row {
        rank: 0,
        number: draft.number,
        title: draft.title.clone(),
        class: class_name(draft.class).to_string(),
        surface: draft.surface.clone(),
        lane: draft.lane,
        lane_reasons: draft.lane_reasons.clone(),
        pending_checks: draft.pending_checks.clone(),
        status,
        hold_reason: None,
        collides_with: peers.into_iter().collect(),
        freshness: draft.freshness.clone(),
        score: Score {
            class,
            readiness,
            priority,
            blocker,
            collision,
            freshness,
            frozen: frozen_term,
            total: subtotal + frozen_term,
        },
    }
}

/// Assign ranks, then serialize collisions: the first dispatchable row to
/// claim a path holds every later row declaring the same path. Walking the
/// already-sorted queue makes the hold a function of the ranking, not of input
/// order.
fn serialize_collisions(rows: &mut [Row]) {
    let mut claimed: BTreeMap<String, u64> = BTreeMap::new();
    for (index, row) in rows.iter_mut().enumerate() {
        row.rank = index + 1;
        // Only a dispatchable row claims a path: an in-flight, frozen or stale
        // row must not block the queue behind it.
        if row.status != Status::Ready {
            continue;
        }
        let held = row
            .surface
            .iter()
            .filter_map(|path| claimed.get(path).map(|owner| (path.clone(), *owner)))
            .min();
        match held {
            Some((path, owner)) => {
                row.status = Status::Hold;
                row.hold_reason = Some(format!("surface collision with #{owner} on {path}"));
            }
            None => {
                for path in &row.surface {
                    claimed.entry(path.clone()).or_insert(row.number);
                }
            }
        }
    }
}

fn class_name(class: Class) -> &'static str {
    match class {
        Class::Product => "product",
        Class::Mechanism => "mechanism",
        Class::Other => "other",
    }
}

/// Pairwise `## Predicted surface` intersection (#1005). Labels are never
/// consulted: #671 and #685 carried no label in common and both got built.
fn collision_matrix(drafts: &[Draft]) -> BTreeMap<u64, BTreeSet<u64>> {
    let sets: Vec<BTreeSet<&str>> = drafts
        .iter()
        .map(|d| d.surface.iter().map(String::as_str).collect())
        .collect();
    let mut matrix: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    for i in 0..drafts.len() {
        for j in (i + 1)..drafts.len() {
            if drafts[i].number == drafts[j].number || sets[i].is_disjoint(&sets[j]) {
                continue;
            }
            matrix
                .entry(drafts[i].number)
                .or_default()
                .insert(drafts[j].number);
            matrix
                .entry(drafts[j].number)
                .or_default()
                .insert(drafts[i].number);
        }
    }
    matrix
}

/// Lane routing. Every criterion here is mechanical; the two that would need a
/// subprocess are named as pending checks rather than run (see module docs).
fn route(
    surface: &[String],
    labels: &BTreeSet<String>,
    body: &str,
    flash_cap: usize,
) -> (Lane, Vec<String>, Vec<String>) {
    if labels.contains("governance") || labels.contains("fleet:goal") {
        return (
            Lane::Controller,
            vec!["governance label".into()],
            Vec::new(),
        );
    }
    if body.contains(JUDGMENT_MARKER) {
        return (
            Lane::Controller,
            vec![format!("judgment marker {JUDGMENT_MARKER} in body")],
            Vec::new(),
        );
    }
    if surface.is_empty() {
        return (Lane::Strong, vec!["no declared surface".into()], Vec::new());
    }
    if surface.len() > flash_cap {
        return (
            Lane::Strong,
            vec![format!(
                "surface {} files > flash cap {flash_cap}",
                surface.len()
            )],
            Vec::new(),
        );
    }
    if let Some(path) = surface.iter().find(|p| p.starts_with("scripts/")) {
        return (
            Lane::Strong,
            vec![format!("surface touches scripts/ ({path})")],
            Vec::new(),
        );
    }
    (
        Lane::Flash,
        vec![format!(
            "surface {} files <= flash cap {flash_cap}, no scripts/ path",
            surface.len()
        )],
        vec![
            PENDING_BRIEF_RENDER.to_string(),
            PENDING_DISPATCH_DRY_RUN.to_string(),
        ],
    )
}

/// The RED-freeze exception: the body cites a run or gate link. A mechanism
/// issue without one is not a pipeline blocker, whatever its prose claims.
pub fn cites_red_run(body: &str) -> bool {
    body.contains("/actions/runs/") || body.contains("/checks")
}

// ---------------------------------------------------------------------------
// Freshness (#970), evaluated against the pinned tree
// ---------------------------------------------------------------------------

/// Re-derive an issue's applicability against the pinned tree.
///
/// Only WARN and FAIL findings are emitted: an empty result means every
/// reference resolved. Four shapes the shell gate got wrong:
///
/// * a path the issue itself declares under `## Predicted surface` WARNs
///   instead of FAILing when it is referenced again elsewhere in the body;
/// * a `doneWhen` heading matches whatever its case;
/// * a command that is to be built (mentioned under `doneWhen` or
///   `## Predicted surface`) or is evidence-cited WARNs instead of FAILing;
/// * a reference to a genuinely deleted path still FAILs.
///
/// Commands resolve against `known_verbs` — this binary's own verb table, and
/// therefore the pinned tree's — so a stale `edda` on `PATH` cannot produce the
/// false FAIL that `edda fleet --help` produced on #1015 itself.
pub fn evaluate_freshness(
    body: &str,
    tree_paths: &BTreeSet<String>,
    known_verbs: &BTreeSet<String>,
) -> Vec<FreshnessFinding> {
    let declared: BTreeSet<String> = surface_paths(body).into_iter().collect();
    let mut findings = Vec::new();
    let mut section = String::new();
    let mut donewhen_seen = false;
    let mut donewhen_content = false;

    for line in body.lines() {
        if let Some(heading) = section_heading(line) {
            section = heading;
            donewhen_seen |= is_donewhen(&section);
            continue;
        }
        if is_donewhen(&section) && !line.trim().is_empty() {
            donewhen_content = true;
        }
        // Predicted-surface tokens are proposals for files that may not exist
        // yet, never references.
        if is_surface(&section) {
            continue;
        }
        let parts: Vec<&str> = line.split('`').collect();
        let mut index = 1;
        while index < parts.len() {
            if !negated(parts[index - 1]) {
                if let Some(finding) = judge_token(
                    parts[index].trim(),
                    line,
                    &section,
                    &declared,
                    tree_paths,
                    known_verbs,
                ) {
                    findings.push(finding);
                }
            }
            index += 2;
        }
    }

    if !donewhen_seen || !donewhen_content {
        findings.push(FreshnessFinding {
            verdict: Verdict::Fail,
            kind: "donewhen".to_string(),
            target: "## doneWhen".to_string(),
            note: "section missing or empty".to_string(),
        });
    }
    // Bodies cite the same file a dozen times; the reader needs the finding
    // once. First occurrence wins, so the order stays the body's.
    let mut seen = BTreeSet::new();
    findings.retain(|finding| seen.insert((finding.kind.clone(), finding.target.clone())));
    findings
}

/// `## Heading` (exactly two hashes), lowercased. `###` and deeper do not
/// change the section, and the case of the heading never matters.
fn section_heading(line: &str) -> Option<String> {
    let trimmed = line.trim_end();
    if trimmed.chars().take_while(|c| *c == '#').count() != 2 {
        return None;
    }
    let rest = trimmed.get(2..)?.strip_prefix(' ')?;
    Some(rest.trim().to_ascii_lowercase())
}

fn is_donewhen(section: &str) -> bool {
    section.starts_with("donewhen")
}

/// Any `… surface` heading. Collision scoring reads only the canonical
/// `## Predicted surface` (via [`surface_paths`], the one definition), but for
/// freshness every surface heading — `Suspected surface` in #761, say — is a
/// proposal about a file that may not exist yet.
fn is_surface(section: &str) -> bool {
    section.ends_with("surface")
}

/// Sections that describe the tree the issue will produce rather than the one
/// it observed. A wrong claim about today's tree is a stale premise; a claim
/// about tomorrow's cannot be. `改哪裡` ("where to change") is this corpus's
/// older spelling of `## Predicted surface` — #763 names the module it will
/// create there, and the shell gate FAILed it for not existing yet.
fn declares_future(section: &str) -> bool {
    is_donewhen(section) || is_surface(section) || section.starts_with("改哪裡")
}

/// A mention preceded by `no` / `not` / `without` is a statement of absence.
fn negated(prose: &str) -> bool {
    let last = prose
        .trim_end()
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(last.as_str(), "no" | "not" | "without")
}

/// Evidence that the mention was observed rather than proposed.
fn evidence_cited(line: &str) -> bool {
    line.contains("http://") || line.contains("https://") || line.contains("CI run")
}

fn judge_token(
    token: &str,
    line: &str,
    section: &str,
    declared: &BTreeSet<String>,
    tree_paths: &BTreeSet<String>,
    known_verbs: &BTreeSet<String>,
) -> Option<FreshnessFinding> {
    if token.is_empty() {
        return None;
    }
    if let Some(rest) = token.strip_prefix("edda ") {
        return judge_command(token, rest, line, section, known_verbs);
    }
    if token.contains(char::is_whitespace) {
        return None;
    }
    // Issues cite line ranges (`crates/edda-pack/src/lib.rs:288-296`); the
    // file is what the tree can answer for.
    let probe = strip_line_suffix(token);
    if probe.contains('/') {
        return judge_path(token, probe, section, declared, tree_paths);
    }
    if has_file_extension(probe) && !probe.contains(|c: char| !is_bare_name_char(c)) {
        // A bare filename cannot be probed directly: resolve it against the
        // tracked paths. More than one hit is an ambiguous reference worth
        // reporting; zero hits is almost always not a path at all — a ledger
        // key (`fleet.mechanism-layer`) or a field (`event_data.error`) — and
        // reporting those would bury the findings that mean something.
        let suffix = format!("/{probe}");
        let hits = tree_paths
            .iter()
            .filter(|p| p.as_str() == probe || p.ends_with(&suffix))
            .count();
        if hits > 1 {
            return Some(warn(
                "path",
                token,
                "bare name does not resolve to exactly one tracked file",
            ));
        }
    }
    None
}

/// `path.rs:12` and `path.rs:12-30` name a file; everything else is returned
/// unchanged.
fn strip_line_suffix(token: &str) -> &str {
    match token.rsplit_once(':') {
        Some((head, tail))
            if !head.is_empty()
                && !tail.is_empty()
                && tail
                    .split('-')
                    .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) =>
        {
            head
        }
        _ => token,
    }
}

fn judge_command(
    token: &str,
    rest: &str,
    line: &str,
    section: &str,
    known_verbs: &BTreeSet<String>,
) -> Option<FreshnessFinding> {
    let verb = rest.split_whitespace().next()?;
    // The same shape the shell gate greps for: anything else (`edda --version`,
    // an elided `edda d…`) is not a verb reference.
    if !verb.starts_with(|c: char| c.is_ascii_lowercase())
        || !verb
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    if known_verbs.contains(verb) {
        return None;
    }
    let to_be_built = declares_future(section) || evidence_cited(line);
    Some(FreshnessFinding {
        verdict: if to_be_built {
            Verdict::Warn
        } else {
            Verdict::Fail
        },
        kind: "command".to_string(),
        target: token.to_string(),
        note: if to_be_built {
            "declared by this issue or evidence-cited; absent from the pinned tree".to_string()
        } else {
            "verb absent from the pinned tree".to_string()
        },
    })
}

/// `token` is what the body said; `probe` is what the tree is asked about.
fn judge_path(
    token: &str,
    probe: &str,
    section: &str,
    declared: &BTreeSet<String>,
    tree_paths: &BTreeSet<String>,
) -> Option<FreshnessFinding> {
    // A slash token is only credible when its last segment names a file;
    // `if/else` and `fix/gh533-...` are not references at all.
    if !has_file_extension(probe.rsplit('/').next()?) {
        return None;
    }
    // Anything that is not a plain repo-relative path — an absolute path, a
    // flag, a traversal — is unparseable rather than missing.
    if probe.starts_with('-')
        || probe.starts_with('/')
        || probe
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        || probe.contains(|c: char| !is_path_char(c))
    {
        return Some(warn("path", token, "unparsed as a repository path"));
    }
    if tree_paths.contains(probe) {
        return None;
    }
    // Issues cite crate- and module-relative paths (`peers/heartbeat.rs`,
    // `edda-ledger/src/paths.rs`). Those are references, not deleted files, so
    // resolve them as a suffix of the tracked paths before FAILing.
    let suffix = format!("/{probe}");
    match tree_paths.iter().filter(|p| p.ends_with(&suffix)).count() {
        1 => return None,
        0 => {}
        _ => {
            return Some(warn(
                "path",
                token,
                "relative reference does not resolve to exactly one tracked file",
            ))
        }
    }
    if declared.contains(probe) || declares_future(section) {
        return Some(warn(
            "path",
            token,
            "declared by this issue; not created yet",
        ));
    }
    Some(FreshnessFinding {
        verdict: Verdict::Fail,
        kind: "path".to_string(),
        target: token.to_string(),
        note: "missing at the pinned tree".to_string(),
    })
}

/// A last path segment names a file when it ends in a plain alphanumeric
/// extension: `main.rs` and `ci.yml` do, `fleet.mechanism-layer` (a ledger
/// key) and `gh533-...` do not.
fn has_file_extension(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty() && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-')
}

fn is_bare_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn warn(kind: &str, target: &str, note: &str) -> FreshnessFinding {
    FreshnessFinding {
        verdict: Verdict::Warn,
        kind: kind.to_string(),
        target: target.to_string(),
        note: note.to_string(),
    }
}

#[cfg(test)]
mod tests;
