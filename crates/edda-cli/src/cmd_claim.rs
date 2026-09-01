//! `edda claim check` — surface-intersection query (GH-562).
//!
//! Answers "does this write surface conflict with any active claim?" against
//! the coordination board that `edda claim --paths` records today. Read-only:
//! it never writes a claim, heartbeat, or request.
//!
//! Exit codes are part of the contract:
//! - 0 — the query surface is disjoint from every active claim (or the board
//!   holds no claims)
//! - 1 — at least one active claim overlaps; the conflicting labels/sessions
//!   are named on stdout
//! - 2 — the query could not be answered soundly: usage error, the board
//!   cannot be read or parsed, or a glob pattern is malformed. Uncertainty
//!   must surface as an error here, never as a false "clear": this verb is
//!   meant to become the machine judgement GH-563's dispatch guard calls
//!   before letting two lanes write the same file.

use anyhow::Context;
use edda_bridge_claude::peers::ClaimEntry;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// One query-path/claim-path pair that overlaps.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PathIntersection {
    /// The path/glob passed to `claim check`.
    pub query: String,
    /// The claimed path/glob it intersects.
    pub claim_path: String,
}

/// A claim whose recorded surface intersects the query surface.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaimConflict {
    pub label: String,
    pub session_id: String,
    pub intersections: Vec<PathIntersection>,
}

/// Machine-readable result of a claim check (`--json`).
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct CheckReport {
    pub conflicts: Vec<ClaimConflict>,
}

/// `edda claim check <paths|globs>...` — read-only conflict query.
///
/// Prints human lines (or a JSON report with `--json`), then exits 1 when any
/// active claim overlaps the query surface. The exit happens via
/// `std::process::exit` because `main` maps `Err` to exit 1 as well, and the
/// two meanings (usage failure vs. surface conflict) must stay distinct.
pub fn claim_check(repo_root: &Path, query: &[String], json: bool) -> anyhow::Result<()> {
    if query.is_empty() {
        eprintln!("usage: edda claim check <path-or-glob>... [--json]");
        std::process::exit(2);
    }

    let project_id = edda_store::project_id(repo_root);
    let claims = match read_active_claims(&project_id) {
        Ok(claims) => claims,
        Err(err) => {
            eprintln!("error: {err:#}");
            eprintln!(
                "refusing to report the surface clear while the coordination board cannot be read"
            );
            std::process::exit(2);
        }
    };
    let query_refs: Vec<&str> = query.iter().map(String::as_str).collect();
    let report = match check(&claims, &query_refs) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(2);
        }
    };

    if json {
        let out = serde_json::to_string_pretty(&report).context("serialize claim check report")?;
        println!("{out}");
    } else if report.conflicts.is_empty() {
        if claims.is_empty() {
            println!("No active claims on the coordination board; surface is clear.");
        } else {
            println!(
                "No conflicts: {} active claim(s) checked against {} query path(s).",
                claims.len(),
                query.len()
            );
        }
    } else {
        for conflict in &report.conflicts {
            println!(
                "CONFLICT with claim \"{}\" (session {})",
                conflict.label, conflict.session_id
            );
            for pair in &conflict.intersections {
                println!("  query {}  <->  claim {}", pair.query, pair.claim_path);
            }
        }
        println!(
            "{} conflicting claim(s) across {} query path(s).",
            report.conflicts.len(),
            query.len()
        );
    }

    if exit_code_for(&report) != 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Exit code for a check result: 0 = disjoint, 1 = conflict.
fn exit_code_for(report: &CheckReport) -> i32 {
    if report.conflicts.is_empty() {
        0
    } else {
        1
    }
}

// ── Strict board read (Round-1 P1-2) ──

/// Path of the coordination board, mirroring
/// `edda_bridge_claude::peers::coordination_path` (which is crate-private and
/// therefore unreachable from here).
///
/// Unlike the peers helper this never migrates in place: `check` is a
/// read-only query, so a legacy `decisions.jsonl` is read as-is instead of
/// being renamed under the caller.
fn coordination_board_path(project_id: &str) -> PathBuf {
    let dir = edda_store::project_dir(project_id).join("state");
    let path = dir.join("coordination.jsonl");
    if !path.exists() && dir.join("decisions.jsonl").exists() {
        return dir.join("decisions.jsonl");
    }
    path
}

/// Strictly read the active claims off the coordination board.
///
/// This is the fail-closed twin of
/// `edda_bridge_claude::peers::compute_board_state`, which serves interactive
/// peers and maps every read or parse failure to an empty board. That bias is
/// right for humans rendering a status view and exactly wrong for the machine
/// judgement the dispatch guard will call: a permission error, a transient
/// read failure, or a damaged claim line must never read as "no claims".
/// Here a missing file legitimately means an empty board; every other I/O or
/// parse failure is an error the caller surfaces as exit 2.
fn read_active_claims(project_id: &str) -> anyhow::Result<Vec<ClaimEntry>> {
    let path = coordination_board_path(project_id);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        // A board that was never created is genuinely empty.
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => anyhow::bail!("cannot read coordination board {}: {e}", path.display()),
    };

    // Event types this query understands but does not fold (kept in sync with
    // `CoordEventType` in the peers module). Anything else is damaged data.
    const NON_CLAIM_EVENTS: &[&str] = &[
        "binding",
        "decision", // legacy alias of "binding"
        "request",
        "request_ack",
        "subagent_completed",
        "task_completed",
        "teammate_idle",
    ];

    let mut claims: HashMap<String, ClaimEntry> = HashMap::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if line.trim().is_empty() {
            continue;
        }
        let ctx = format!("coordination board {} line {line_no}", path.display());
        let event: serde_json::Value = serde_json::from_str(line).with_context(|| ctx.clone())?;
        let obj = event
            .as_object()
            .with_context(|| format!("{ctx}: event is not a JSON object"))?;
        let kind = obj
            .get("event_type")
            .and_then(|v| v.as_str())
            .with_context(|| format!("{ctx}: event has no event_type"))?;
        match kind {
            "claim" => {
                let session_id = str_field(obj, "session_id").with_context(|| ctx.clone())?;
                let ts = str_field(obj, "ts").with_context(|| ctx.clone())?;
                let payload = obj
                    .get("payload")
                    .and_then(|v| v.as_object())
                    .with_context(|| format!("{ctx}: claim event has no payload object"))?;
                let label = payload
                    .get("label")
                    .and_then(|v| v.as_str())
                    .with_context(|| format!("{ctx}: claim event has no string label"))?;
                let paths: Vec<String> = match payload.get("paths") {
                    Some(v) => v
                        .as_array()
                        .with_context(|| format!("{ctx}: claim paths is not an array"))?
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            v.as_str()
                                .map(str::to_string)
                                .with_context(|| format!("{ctx}: claim path {i} is not a string"))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    None => anyhow::bail!("{ctx}: claim event has no paths field"),
                };
                claims.insert(
                    session_id.to_string(),
                    ClaimEntry {
                        session_id: session_id.to_string(),
                        label: label.to_string(),
                        paths,
                        ts: ts.to_string(),
                    },
                );
            }
            "unclaim" => {
                let session_id = str_field(obj, "session_id").with_context(|| ctx.clone())?;
                claims.remove(session_id);
            }
            other if NON_CLAIM_EVENTS.contains(&other) => {}
            other => anyhow::bail!("{ctx}: unknown event_type {other:?}"),
        }
    }

    // Same fold order as `compute_board_state`: one claim per session, sorted
    // by label, so `check` never disagrees with what peers see.
    let mut sorted: Vec<_> = claims.into_values().collect();
    sorted.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(sorted)
}

fn str_field<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> anyhow::Result<&'a str> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .with_context(|| format!("event has no string {key} field"))
}

/// Pure intersection of a query surface against claims (unit-testable core).
///
/// Claims whose recorded path list is empty cover nothing (a label-only
/// claim), so they never conflict. Claims are folded one-per-session by the
/// board, so each claim appears at most once in the report.
///
/// Errors when a glob cannot be parsed soundly; the caller surfaces that as
/// exit 2 rather than reporting a false "clear".
pub fn check(
    claims: &[edda_bridge_claude::peers::ClaimEntry],
    query: &[&str],
) -> Result<CheckReport, String> {
    // Validate every glob up front, so a malformed pattern is an error even
    // when it would never be compared against another glob.
    for token in query.iter().map(|t| normalize_token(t)).chain(
        claims
            .iter()
            .flat_map(|c| c.paths.iter())
            .map(|t| normalize_token(t)),
    ) {
        if has_wildcard(&token) {
            globset::Glob::new(&token).map_err(|e| format!("invalid glob {token:?}: {e}"))?;
        }
    }

    let mut conflicts = Vec::new();
    for c in claims {
        if c.paths.is_empty() {
            continue;
        }
        let mut intersections = Vec::new();
        for q in query {
            for claimed in &c.paths {
                if surfaces_intersect(q, claimed)? {
                    intersections.push(PathIntersection {
                        query: q.to_string(),
                        claim_path: claimed.clone(),
                    });
                }
            }
        }
        if !intersections.is_empty() {
            conflicts.push(ClaimConflict {
                label: c.label.clone(),
                session_id: c.session_id.clone(),
                intersections,
            });
        }
    }
    Ok(CheckReport { conflicts })
}

/// Whether a query token and a claim token can name the same file.
///
/// The bias here is the opposite of a human-facing matcher: this judgement is
/// what the GH-563 dispatch guard will call before letting two lanes write,
/// so uncertainty must resolve toward "conflict", never toward "clear".
///
/// - literal vs literal: exact equality after normalization (separator, case
///   on Windows, and leading `./` — see [`normalize_token`])
/// - glob vs literal: exact globset match of the pattern against the literal
/// - glob vs glob: an NFA-product decision over the two glob languages (see
///   `globs_intersect`). Errs toward conflict whenever a glob cannot be
///   parsed soundly.
fn surfaces_intersect(a: &str, b: &str) -> Result<bool, String> {
    let a = normalize_token(a);
    let b = normalize_token(b);
    match (has_wildcard(&a), has_wildcard(&b)) {
        (false, false) => Ok(a == b),
        (true, false) => glob_matches(&a, &b),
        (false, true) => glob_matches(&b, &a),
        (true, true) => globs_intersect(&a, &b),
    }
}

/// Normalize a surface token before any comparison.
///
/// - A leading `./` is stripped on every platform: `./src/x.rs` and
///   `src/x.rs` name the same file.
/// - On Windows, `\\` is a path separator and file names compare
///   case-insensitively, so separators are folded to `/` and ASCII letters
///   are lowercased. Without this, two tokens naming the same Windows file
///   are declared disjoint.
fn normalize_token(token: &str) -> String {
    let mut s = token.to_string();
    if cfg!(windows) {
        s = s.replace('\\', "/");
        s = s
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c
                }
            })
            .collect();
    }
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    s
}

fn has_wildcard(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[') || pattern.contains('{')
}

/// Exact: does `pattern` match the literal path `candidate`?
fn glob_matches(pattern: &str, candidate: &str) -> Result<bool, String> {
    let glob = globset::Glob::new(pattern).map_err(|e| format!("invalid glob {pattern:?}: {e}"))?;
    Ok(glob.compile_matcher().is_match(candidate))
}

/// Sound over-approximation of glob-vs-glob intersection.
///
/// Both patterns are compiled to Thompson NFAs and the product is searched
/// for a string both languages accept; when one exists, the witness string is
/// returned so tests can confirm it against real globset. The NFA language is
/// a superset of what globset accepts (see the module-level glob semantics
/// notes), so a reported intersection may occasionally be broader than
/// globset itself — conservative — while an empty product is a true proof of
/// disjointness. A pattern that cannot even be parsed is an error, never a
/// silent "disjoint".
fn globs_intersect(a: &str, b: &str) -> Result<bool, String> {
    let n1 = build_nfa(a)?;
    let n2 = build_nfa(b)?;
    Ok(nfa_pair_intersects(&n1, &n2).0)
}

// ── Glob → NFA engine ──
//
// Semantics encoded here (probed against globset 0.4.18's default `Glob`,
// which is NOT separator-aware):
// - `*` and `?` match any character including `/`; `[...]` classes too.
// - a full-component `**` (i.e. preceded by start or `/` and followed by
//   `/` or end): `x/**/y` ≡ `x/([^/]*/)*y`, `**/y` ≡ `([^/]*/)*y`,
//   `x/**` ≡ `x/.*`, a bare `**` ≡ `.*`.
// - any other star run (`a**b`, `***`) collapses to `.*`.
// - `{a,b}` alternation may contain nested globs and `/`.
// Every rule below matches or over-approximates globset, so an empty product
// language is a sound proof of disjointness.

/// One character matched by an NFA transition.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CharClass {
    /// Exactly one character (may be `/`).
    Lit(char),
    /// Any single character, including `/`.
    Any,
    /// A `[...]` class over sorted, disjoint ranges.
    Set {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl CharClass {
    fn matches(&self, c: char) -> bool {
        match self {
            CharClass::Lit(l) => *l == c,
            CharClass::Any => true,
            CharClass::Set { negated, ranges } => {
                ranges.iter().any(|&(lo, hi)| c >= lo && c <= hi) != *negated
            }
        }
    }
}

struct Nfa {
    trans: Vec<Vec<(CharClass, usize)>>,
    eps: Vec<Vec<usize>>,
    start: usize,
    accept: usize,
}

#[derive(Default)]
struct NfaBuilder {
    trans: Vec<Vec<(CharClass, usize)>>,
    eps: Vec<Vec<usize>>,
}

impl NfaBuilder {
    fn state(&mut self) -> usize {
        self.trans.push(Vec::new());
        self.eps.push(Vec::new());
        self.trans.len() - 1
    }

    fn eps_edge(&mut self, from: usize, to: usize) {
        self.eps[from].push(to);
    }

    fn char_edge(&mut self, from: usize, class: CharClass, to: usize) {
        self.trans[from].push((class, to));
    }
}

/// A Thompson fragment: `start` has no incoming edges beyond what earlier
/// compositions added, `end` has no outgoing edges until composed further.
#[derive(Clone, Copy)]
struct Frag {
    start: usize,
    end: usize,
}

struct Cursor {
    chars: Vec<char>,
    pos: usize,
}

impl Cursor {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }
}

fn build_nfa(pattern: &str) -> Result<Nfa, String> {
    let mut b = NfaBuilder::default();
    let mut cur = Cursor {
        chars: pattern.chars().collect(),
        pos: 0,
    };
    let frag = parse_alternates(&mut cur, &mut b, 0)?;
    if cur.pos != cur.chars.len() {
        return Err(format!(
            "unexpected {:?} at position {}",
            cur.peek(),
            cur.pos
        ));
    }
    Ok(Nfa {
        trans: b.trans,
        eps: b.eps,
        start: frag.start,
        accept: frag.end,
    })
}

/// Parse `seq (',' seq)*` — a brace-alternation body at `depth > 0`, or the
/// whole pattern at depth 0. At depth 0 both `,` and `}` are plain literals.
fn parse_alternates(cur: &mut Cursor, b: &mut NfaBuilder, depth: usize) -> Result<Frag, String> {
    let mut frags = vec![parse_sequence(cur, b, depth)?];
    while depth > 0 && cur.peek() == Some(',') {
        cur.pos += 1;
        frags.push(parse_sequence(cur, b, depth)?);
    }
    if frags.len() == 1 {
        return Ok(frags.remove(0));
    }
    let s = b.state();
    let e = b.state();
    for f in frags {
        b.eps_edge(s, f.start);
        b.eps_edge(f.end, e);
    }
    Ok(Frag { start: s, end: e })
}

fn parse_sequence(cur: &mut Cursor, b: &mut NfaBuilder, depth: usize) -> Result<Frag, String> {
    let mut acc: Option<Frag> = None;
    // True at sequence start or right after a `/` literal — the boundary a
    // full-component `**` needs on its left.
    let mut prev_boundary = true;
    while let Some(c) = cur.peek() {
        match c {
            ',' | '}' if depth > 0 => break,
            '{' => {
                cur.pos += 1;
                let inner = parse_alternates(cur, b, depth + 1)?;
                if cur.peek() != Some('}') {
                    return Err("unterminated {...} alternation".to_string());
                }
                cur.pos += 1;
                acc = Some(concat(b, acc, inner));
                prev_boundary = false;
            }
            '[' => {
                let class = parse_class(cur)?;
                let frag = single_edge(b, class);
                acc = Some(concat(b, acc, frag));
                prev_boundary = false;
            }
            '\\' => {
                cur.pos += 1;
                let esc = cur
                    .next()
                    .ok_or_else(|| "trailing backslash escape".to_string())?;
                let frag = single_edge(b, CharClass::Lit(esc));
                acc = Some(concat(b, acc, frag));
                prev_boundary = esc == '/';
            }
            '/' => {
                cur.pos += 1;
                let frag = single_edge(b, CharClass::Lit('/'));
                acc = Some(concat(b, acc, frag));
                prev_boundary = true;
            }
            '?' => {
                cur.pos += 1;
                let frag = single_edge(b, CharClass::Any);
                acc = Some(concat(b, acc, frag));
                prev_boundary = false;
            }
            '*' => {
                let run_start = cur.pos;
                while cur.peek() == Some('*') {
                    cur.pos += 1;
                }
                let run_len = cur.pos - run_start;
                let full_component =
                    run_len == 2 && prev_boundary && matches!(cur.peek(), None | Some('/'));
                if full_component && cur.peek() == Some('/') {
                    // `x/**/y` ≡ `x/([^/]*/)*y`: the component loop already
                    // ends with its separator, so consume the literal `/`.
                    cur.pos += 1;
                    let frag = starstar_components(b);
                    acc = Some(concat(b, acc, frag));
                    prev_boundary = true;
                } else {
                    // A trailing full-component `**` is `.*` after the `/`
                    // already emitted; any other star run (`a**b`, `***`,
                    // `x**/y`) collapses to `.*` — a strict superset of every
                    // globset reading of the run.
                    let frag = star_any(b);
                    acc = Some(concat(b, acc, frag));
                    prev_boundary = false;
                }
            }
            c => {
                cur.pos += 1;
                let frag = single_edge(b, CharClass::Lit(c));
                acc = Some(concat(b, acc, frag));
                prev_boundary = false;
            }
        }
    }
    Ok(acc.unwrap_or_else(|| eps_fragment(b)))
}

fn concat(b: &mut NfaBuilder, acc: Option<Frag>, f: Frag) -> Frag {
    match acc {
        None => f,
        Some(a) => {
            b.eps_edge(a.end, f.start);
            Frag {
                start: a.start,
                end: f.end,
            }
        }
    }
}

fn single_edge(b: &mut NfaBuilder, class: CharClass) -> Frag {
    let s = b.state();
    let e = b.state();
    b.char_edge(s, class, e);
    Frag { start: s, end: e }
}

fn eps_fragment(b: &mut NfaBuilder) -> Frag {
    let s = b.state();
    let e = b.state();
    b.eps_edge(s, e);
    Frag { start: s, end: e }
}

/// `.*` — zero or more of any character, `/` included.
fn star_any(b: &mut NfaBuilder) -> Frag {
    let s = b.state();
    let a = b.state();
    let e = b.state();
    b.eps_edge(s, e);
    b.eps_edge(s, a);
    b.char_edge(a, CharClass::Any, a);
    b.eps_edge(a, e);
    Frag { start: s, end: e }
}

/// Zero or more whole path components: `([^/]*/)*` — the language of a
/// full-component `**` that is followed by another component or ends a
/// non-empty prefix. Matches `a/b` and `a/x/y/b` for `a/**/b`, and `b` for
/// `**/b` (empty components are allowed, as in globset: `a//b`). Components
/// themselves cannot contain `/`, exactly like globset.
fn starstar_components(b: &mut NfaBuilder) -> Frag {
    let s = b.state();
    let a = b.state();
    let e = b.state();
    let any_component_char = CharClass::Set {
        negated: true,
        ranges: vec![('/', '/')],
    };
    b.eps_edge(s, e); // zero components
    b.eps_edge(s, a); // enter one component
    b.char_edge(a, any_component_char, a); // component characters
    b.char_edge(a, CharClass::Lit('/'), s); // separator, then repeat or stop
    Frag { start: s, end: e }
}

fn parse_class(cur: &mut Cursor) -> Result<CharClass, String> {
    cur.pos += 1; // consume '['
    let negated = matches!(cur.peek(), Some('!') | Some('^'));
    if negated {
        cur.pos += 1;
    }
    let mut ranges: Vec<(char, char)> = Vec::new();
    let mut first = true;
    loop {
        let c = cur
            .next()
            .ok_or_else(|| "unterminated [...] class".to_string())?;
        if c == ']' && !first {
            break;
        }
        first = false;
        let lo = if c == '\\' {
            cur.next()
                .ok_or_else(|| "unterminated [...] class".to_string())?
        } else {
            c
        };
        // `x-y` is a range unless the `-` is the final character before `]`.
        if cur.peek() == Some('-') && matches!(cur.peek_at(1), Some(n) if n != ']') {
            cur.pos += 1;
            let hi = cur
                .next()
                .ok_or_else(|| "unterminated [...] class".to_string())?;
            let hi = if hi == '\\' {
                cur.next()
                    .ok_or_else(|| "unterminated [...] class".to_string())?
            } else {
                hi
            };
            if hi < lo {
                return Err(format!("invalid character range {lo}-{hi}"));
            }
            ranges.push((lo, hi));
        } else {
            ranges.push((lo, lo));
        }
    }
    ranges.sort();
    let mut merged: Vec<(char, char)> = Vec::with_capacity(ranges.len());
    for (lo, hi) in ranges {
        match merged.last_mut() {
            Some(last) if lo as u32 <= last.1 as u32 + 1 => {
                if hi > last.1 {
                    last.1 = hi;
                }
            }
            _ => merged.push((lo, hi)),
        }
    }
    Ok(CharClass::Set {
        negated,
        ranges: merged,
    })
}

// ── NFA product emptiness ──

/// Decide whether the two glob languages share a string, and return that
/// string as a witness when they do (used to test the engine against
/// globset itself).
///
/// Classic product construction: the search state is the set of PAIRS of NFA
/// states reachable in lockstep, so a reported witness really drives both
/// automata to acceptance simultaneously.
fn nfa_pair_intersects(n1: &Nfa, n2: &Nfa) -> (bool, Option<String>) {
    type StatePair = (usize, usize);
    type PairSet = BTreeSet<StatePair>;

    fn eps_close_pairs(n1: &Nfa, n2: &Nfa, seed: PairSet) -> PairSet {
        let mut set = BTreeSet::new();
        let mut queue: VecDeque<StatePair> = seed.into_iter().collect();
        while let Some((x, y)) = queue.pop_front() {
            if set.insert((x, y)) {
                for &x2 in &n1.eps[x] {
                    queue.push_back((x2, y));
                }
                for &y2 in &n2.eps[y] {
                    queue.push_back((x, y2));
                }
            }
        }
        set
    }

    let start = eps_close_pairs(n1, n2, BTreeSet::from([(n1.start, n2.start)]));
    if start.contains(&(n1.accept, n2.accept)) {
        return (true, Some(String::new()));
    }

    let mut visited = std::collections::HashSet::new();
    visited.insert(start.clone());
    let mut parent: HashMap<PairSet, (PairSet, char)> = HashMap::new();
    let mut queue: VecDeque<PairSet> = VecDeque::new();
    queue.push_back(start);

    while let Some(set) = queue.pop_front() {
        // Group successor pairs by the shared character consumed.
        let mut nexts: HashMap<char, PairSet> = HashMap::new();
        for &(x, y) in set.iter() {
            for (cls1, t1) in &n1.trans[x] {
                for (cls2, t2) in &n2.trans[y] {
                    if let Some(ch) = common_char(cls1, cls2) {
                        nexts.entry(ch).or_default().insert((*t1, *t2));
                    }
                }
            }
        }
        for (ch, raw) in nexts {
            let next = eps_close_pairs(n1, n2, raw);
            if visited.insert(next.clone()) {
                parent.insert(next.clone(), (set.clone(), ch));
                if next.contains(&(n1.accept, n2.accept)) {
                    // Reconstruct the witness path back to the start pair.
                    let mut path = String::new();
                    let mut cur = &next;
                    while let Some((prev, ch)) = parent.get(cur) {
                        path.insert(0, *ch);
                        cur = prev;
                    }
                    return (true, Some(path));
                }
                queue.push_back(next);
            }
        }
    }
    (false, None)
}

/// A character matched by both classes, if one exists.
fn common_char(a: &CharClass, b: &CharClass) -> Option<char> {
    match (a, b) {
        (CharClass::Lit(x), CharClass::Lit(y)) => (x == y).then_some(*x),
        // A literal shares a character with the other class only if that
        // class matches the literal's character.
        (CharClass::Lit(c), other) => other.matches(*c).then_some(*c),
        (other, CharClass::Lit(c)) => other.matches(*c).then_some(*c),
        (CharClass::Any, CharClass::Any) => Some('z'),
        (CharClass::Any, CharClass::Set { negated, ranges })
        | (CharClass::Set { negated, ranges }, CharClass::Any) => set_first_match(*negated, ranges),
        (
            CharClass::Set {
                negated: n1,
                ranges: r1,
            },
            CharClass::Set {
                negated: n2,
                ranges: r2,
            },
        ) => set_pair_first_common(*n1, r1, *n2, r2),
    }
}

/// First character (by code point) matched by a class-set, if any valid
/// character belongs to it.
fn set_first_match(negated: bool, ranges: &[(char, char)]) -> Option<char> {
    if !negated {
        return ranges.first().map(|&(lo, _)| lo);
    }
    // Negated: first code point outside every range (surrogates skipped).
    let mut next = 0u32;
    for &(lo, hi) in ranges {
        if next < lo as u32 {
            return first_valid_char(next, lo as u32 - 1);
        }
        next = next.max(hi as u32 + 1);
    }
    first_valid_char(next, u32::MAX)
}

/// First common character of two class-sets. `a`/`b` may each be negated.
fn set_pair_first_common(
    a_neg: bool,
    a: &[(char, char)],
    b_neg: bool,
    b: &[(char, char)],
) -> Option<char> {
    match (a_neg, b_neg) {
        (false, false) => first_range_overlap(a, b),
        (false, true) => first_in_ranges_minus(a, b),
        (true, false) => first_in_ranges_minus(b, a),
        (true, true) => {
            // Both negated: any character outside a ∪ b.
            let mut union: Vec<(u32, u32)> = a
                .iter()
                .chain(b)
                .map(|&(lo, hi)| (lo as u32, hi as u32))
                .collect();
            union.sort();
            let mut next = 0u32;
            for &(lo, hi) in &union {
                if next < lo {
                    return first_valid_char(next, lo - 1);
                }
                next = next.max(hi.saturating_add(1));
            }
            first_valid_char(next, u32::MAX)
        }
    }
}

/// First code point covered by both sorted disjoint range lists.
fn first_range_overlap(a: &[(char, char)], b: &[(char, char)]) -> Option<char> {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if lo <= hi {
            return Some(lo);
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    None
}

/// First character in `a` that is not covered by `b`.
fn first_in_ranges_minus(a: &[(char, char)], b: &[(char, char)]) -> Option<char> {
    for &(alo, ahi) in a {
        let mut cur = alo as u32;
        let ahi = ahi as u32;
        for &(blo, bhi) in b {
            let (blo, bhi) = (blo as u32, bhi as u32);
            if bhi < cur || blo > ahi {
                continue;
            }
            if blo > cur {
                if let Some(c) = first_valid_char(cur, blo - 1) {
                    return Some(c);
                }
            }
            cur = cur.max(bhi + 1);
            if cur > ahi {
                break;
            }
        }
        if cur <= ahi {
            if let Some(c) = first_valid_char(cur, ahi) {
                return Some(c);
            }
        }
    }
    None
}

/// First valid (non-surrogate) character in an inclusive code-point range.
fn first_valid_char(from: u32, to: u32) -> Option<char> {
    (from..=to).find_map(char::from_u32)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn claim(label: &str, session: &str, paths: &[&str]) -> edda_bridge_claude::peers::ClaimEntry {
        edda_bridge_claude::peers::ClaimEntry {
            session_id: session.to_string(),
            label: label.to_string(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            ts: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn labels(report: &CheckReport) -> Vec<&str> {
        report.conflicts.iter().map(|c| c.label.as_str()).collect()
    }

    /// An NFA accepting exactly `s` (literal characters only), so engine
    /// acceptance of a probe string can be tested without parsing it as a
    /// glob.
    fn literal_nfa(s: &str) -> Nfa {
        let mut b = NfaBuilder::default();
        let mut acc: Option<Frag> = None;
        for c in s.chars() {
            let frag = single_edge(&mut b, CharClass::Lit(c));
            acc = Some(concat(&mut b, acc, frag));
        }
        let frag = acc.unwrap_or_else(|| eps_fragment(&mut b));
        Nfa {
            trans: b.trans,
            eps: b.eps,
            start: frag.start,
            accept: frag.end,
        }
    }

    #[test]
    fn exact_overlap_conflicts() {
        let claims = vec![claim(
            "peer-a",
            "s1",
            &["crates/edda-conductor/src/agent/codex_rpc.rs"],
        )];
        let report =
            check(&claims, &["crates/edda-conductor/src/agent/codex_rpc.rs"]).expect("globs parse");
        assert_eq!(labels(&report), vec!["peer-a"]);
        assert_eq!(
            report.conflicts[0].intersections,
            vec![PathIntersection {
                query: "crates/edda-conductor/src/agent/codex_rpc.rs".into(),
                claim_path: "crates/edda-conductor/src/agent/codex_rpc.rs".into(),
            }]
        );
        assert_eq!(exit_code_for(&report), 1);
    }

    #[test]
    fn glob_vs_glob_overlap_conflicts() {
        let claims = vec![claim("peer-b", "s2", &["crates/edda-cli/src/cmd_*.rs"])];
        let report = check(&claims, &["crates/edda-cli/src/*"]).expect("globs parse");
        assert_eq!(labels(&report), vec!["peer-b"]);
        assert_eq!(exit_code_for(&report), 1);
    }

    #[test]
    fn glob_vs_literal_overlap_conflicts() {
        let claims = vec![claim("peer-c", "s3", &["crates/edda-cli/src/*"])];
        let report = check(&claims, &["crates/edda-cli/src/main.rs"]).expect("globs parse");
        assert_eq!(labels(&report), vec!["peer-c"]);
        assert_eq!(exit_code_for(&report), 1);
    }

    #[test]
    fn disjoint_surfaces_exit_zero() {
        let claims = vec![
            claim(
                "gh561",
                "s4",
                &["crates/edda-conductor/src/runner/sequential.rs"],
            ),
            claim("cli", "s5", &["crates/edda-cli/src/cmd_bridge.rs"]),
        ];
        let report =
            check(&claims, &["crates/edda-conductor/src/agent/codex_rpc.rs"]).expect("globs parse");
        assert!(report.conflicts.is_empty());
        assert_eq!(exit_code_for(&report), 0);
    }

    #[test]
    fn disjoint_globs_exit_zero() {
        let claims = vec![
            claim("plan-owner", "s6", &["crates/edda-conductor/src/plan/**"]),
            claim("cli-owner", "s7", &["crates/edda-cli/src/cmd_*.rs"]),
        ];
        let report = check(&claims, &["crates/edda-conductor/src/agent/*"]).expect("globs parse");
        assert!(report.conflicts.is_empty());
        assert_eq!(exit_code_for(&report), 0);
    }

    #[test]
    fn no_claims_is_disjoint() {
        let report = check(&[], &["crates/edda-cli/src/*"]).expect("globs parse");
        assert!(report.conflicts.is_empty());
        assert_eq!(exit_code_for(&report), 0);
    }

    #[test]
    fn claim_without_paths_covers_nothing() {
        let claims = vec![claim("label-only", "s8", &[])];
        let report = check(&claims, &["crates/edda-cli/src/*"]).expect("globs parse");
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn one_conflicting_claim_named_among_many() {
        let claims = vec![
            claim("clean-1", "s9", &["docs/*"]),
            claim("dirty", "s10", &["crates/edda-cli/src/main.rs"]),
            claim("clean-2", "s11", &["scripts/**"]),
        ];
        let report = check(
            &claims,
            &[
                "crates/edda-cli/src/cmd_claim.rs",
                "crates/edda-cli/src/main.rs",
            ],
        )
        .expect("globs parse");
        assert_eq!(labels(&report), vec!["dirty"]);
        assert_eq!(report.conflicts[0].intersections.len(), 1);
        assert_eq!(
            report.conflicts[0].intersections[0].query,
            "crates/edda-cli/src/main.rs"
        );
    }

    // ── Round-1 P1-1 regressions: real intersections must never be cleared ──

    #[test]
    fn review_pair_double_star_prefix() {
        // Both match crates/edda-cli/src/cmd_claim.rs.
        assert!(
            surfaces_intersect("crates/**/cmd_*", "crates/edda-cli/src/*claim.rs")
                .expect("glob pair decidable")
        );
    }

    #[test]
    fn review_pair_both_sided_wildcards() {
        // Both match crates/edda-cli/src/cmd_claim.rs.
        assert!(surfaces_intersect(
            "crates/edda-*/src/cmd_claim.rs",
            "crates/*cli/src/cmd_claim.rs"
        )
        .expect("glob pair decidable"));
    }

    #[test]
    fn review_pair_char_class_and_question() {
        // Both match crates/edda-cli/src/cmd_ask.rs.
        assert!(
            surfaces_intersect("crates/edda-cli/src/cmd_[ab]*", "*cmd_a?*")
                .expect("glob pair decidable")
        );
    }

    #[test]
    fn review_pair_brace_expansion() {
        // Both match crates/edda-cli/src/main.rs.
        assert!(surfaces_intersect(
            "crates/{edda-cli,docs}/src/main.rs",
            "crates/edda-cli/src/*.rs"
        )
        .expect("glob pair decidable"));
    }

    #[test]
    fn review_pair_filler_longer_than_one_char() {
        // `abcd` is matched by both; the old one-character witness missed it.
        assert!(surfaces_intersect("ab*", "*cd").expect("glob pair decidable"));
    }

    #[test]
    fn review_pair_suffix_only_globs() {
        // Both match cmd_claim.rs.
        assert!(surfaces_intersect("*claim.rs", "*cmd_claim.rs").expect("glob pair decidable"));
    }

    // ── Round-2 P1: NTFS Unicode case and glob class escapes ──

    #[cfg(windows)]
    #[test]
    fn ntfs_unicode_case_literal_pair_is_refused_not_clear() {
        // On this NTFS volume `Ä.rs` and `ä.rs` resolve to the same file
        // (Test-Path control: true), but normalize_token folds ASCII case
        // only. The pre-fix engine reported the pair disjoint and exit 0.
        // Rust cannot read the system upcase table that decides NTFS name
        // equality, so the only sound answer is refusal.
        let err = surfaces_intersect("src/Ä.rs", "src/ä.rs")
            .expect_err("Unicode case-variant literals must be refused, not cleared");
        assert!(err.contains("cannot decide"), "got: {err}");
        assert!(err.contains("src/Ä.rs") && err.contains("src/ä.rs"), "got: {err}");
    }

    #[cfg(windows)]
    #[test]
    fn ntfs_unicode_case_glob_vs_literal_is_refused_not_clear() {
        // A claim `src/Ä*` can reach the same NTFS file the query names as
        // `src/ä.rs` through a case-variant spelling. The pre-fix engine
        // (globset is case-sensitive) reported clear.
        let err = surfaces_intersect("src/Ä*", "src/ä.rs")
            .expect_err("Unicode case-variant glob vs literal must be refused, not cleared");
        assert!(err.contains("cannot decide"), "got: {err}");
    }

    #[cfg(windows)]
    #[test]
    fn ntfs_unicode_case_glob_vs_glob_is_refused_not_clear() {
        let err = surfaces_intersect("src/Ä*", "src/ä*")
            .expect_err("Unicode case-variant globs must be refused, not cleared");
        assert!(err.contains("cannot decide"), "got: {err}");
    }

    #[test]
    fn class_escape_member_matches_globset_exactly() {
        // globset 0.4.18's parse_class has no backslash handling at all: a
        // `\` inside `[...]` is an ordinary class member. `[a\-c]` is
        // therefore member `a`, member `\`, then range `\`..`c` — it matches
        // `a`, `b`, `c` but neither `-` nor `\` (probed against real
        // globset). The pre-fix engine escaped the next character instead,
        // reduced the class to {a, -, c}, and declared the pair disjoint.
        let accepts = |pat: &str, s: &str| {
            let n = build_nfa(pat).expect("parses");
            nfa_pair_intersects(&n, &literal_nfa(s)).0
        };
        assert!(accepts(r"[a\-c]", "b"));
        assert!(accepts(r"[a\-c]", "a"));
        assert!(accepts(r"[a\-c]", "c"));
        assert!(!accepts(r"[a\-c]", "-"));
        // `[\]]` is class {\} followed by a literal `]`: it matches only the
        // two-character string `\]`, never a bare `]`.
        assert!(accepts(r"[\]]", "\\]"));
        assert!(!accepts(r"[\]]", "]"));
        // `[\a]` is class {\, a}.
        assert!(accepts(r"[\a]", "a"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_class_escape_pair_intersects_not_clear() {
        // The round-2 reviewer pair, end to end through normalization: real
        // globset matches `b` with both patterns, so the pair must conflict,
        // never clear. (This Unix-only test could not be watched red on the
        // Windows lane; `class_escape_member_matches_globset_exactly` above
        // pins the same engine path and was watched red.)
        assert!(surfaces_intersect("[a\\-c]", "[b]").expect("decidable"));
    }

    #[cfg(windows)]
    #[test]
    fn e2e_unicode_case_pair_is_error_not_clear() {
        // Claim `src/Ä.rs`, query `src/ä.rs`: the pre-fix engine returned
        // exit 0 with {"conflicts":[]} although both spellings resolve to
        // the same NTFS file. The check must refuse (exit 2) instead.
        let repo = tempfile::tempdir().expect("repo tempdir");
        std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
        let project_id = edda_store::project_id(repo.path());
        let store = tempfile::tempdir().expect("store tempdir");
        write_board(
            store.path(),
            &project_id,
            &[coord_event("sess-9", "peer-c", &["src/Ä.rs"])],
        );
        let bin = edda_bin();
        assert!(bin.exists(), "edda binary not found at {}", bin.display());
        let out = std::process::Command::new(&bin)
            .args(["claim", "check", "src/ä.rs", "--json"])
            .current_dir(repo.path())
            .env("EDDA_STORE_ROOT", store.path())
            .output()
            .expect("spawn edda");
        let code = out.status.code().expect("exit code");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(code, 2, "stdout={stdout:?} stderr={stderr:?}");
        assert!(stderr.contains("cannot decide"), "stderr={stderr:?}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_is_normalized() {
        // SRC/*.RS and src/main.rs name the same file on Windows.
        assert!(surfaces_intersect("SRC/*.RS", "src/main.rs").expect("glob pair decidable"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_backslash_separator_is_normalized() {
        assert!(surfaces_intersect("src\\*.rs", "src/*.rs").expect("glob pair decidable"));
        assert!(surfaces_intersect("src\\main.rs", "src/main.rs").expect("glob pair decidable"));
    }

    #[test]
    fn leading_dot_slash_is_normalized() {
        assert!(surfaces_intersect("./src/main.rs", "src/main.rs").expect("glob pair decidable"));
        assert!(surfaces_intersect("./src/*.rs", "src/*.rs").expect("glob pair decidable"));
    }

    #[test]
    fn distinct_directories_stay_disjoint() {
        assert!(
            !surfaces_intersect("crates/edda-cli/src/*", "crates/edda-conductor/src/*")
                .expect("glob pair decidable")
        );
    }

    #[test]
    fn different_extensions_stay_disjoint() {
        assert!(!surfaces_intersect("src/*.rs", "docs/*.md").expect("glob pair decidable"));
    }

    #[test]
    fn double_star_confined_to_prefix_stays_disjoint() {
        assert!(!surfaces_intersect("crates/**", "docs/x.md").expect("glob pair decidable"));
    }

    #[test]
    fn double_star_still_needs_final_component() {
        assert!(!surfaces_intersect("a/**/b", "a/x").expect("glob pair decidable"));
    }

    // ── End-to-end: the board must not fail open (Round-1 P1-2) and the
    //    non-`check` parser must be unchanged (Round-1 P1-3). ──

    /// Spawn the binary and return (exit code, stdout, stderr).
    fn run_edda(args: &[&str], repo: &Path, store: &Path) -> (i32, String, String) {
        let out = std::process::Command::new(edda_bin())
            .args(args)
            .current_dir(repo)
            .env("EDDA_STORE_ROOT", store)
            .output()
            .expect("spawn edda");
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn board_file(store: &Path, project_id: &str) -> PathBuf {
        store
            .join("projects")
            .join(project_id)
            .join("state")
            .join("coordination.jsonl")
    }

    #[test]
    fn e2e_unreadable_board_is_error_not_clear() {
        let repo = tempfile::tempdir().expect("repo tempdir");
        std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
        let project_id = edda_store::project_id(repo.path());
        let store = tempfile::tempdir().expect("store tempdir");
        // A directory at the board path makes every read fail.
        std::fs::create_dir_all(board_file(store.path(), &project_id)).expect("block board");
        let (code, stdout, stderr) = run_edda(
            &["claim", "check", "src/main.rs", "--json"],
            repo.path(),
            store.path(),
        );
        assert_eq!(
            code, 2,
            "unreadable board must exit 2, got stdout={stdout:?} stderr={stderr:?}"
        );
    }

    #[test]
    fn e2e_malformed_board_line_is_error_not_clear() {
        let repo = tempfile::tempdir().expect("repo tempdir");
        std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
        let project_id = edda_store::project_id(repo.path());
        let store = tempfile::tempdir().expect("store tempdir");
        write_board(
            store.path(),
            &project_id,
            &["{".to_string(), coord_event("s1", "peer-a", &["src/*"])],
        );
        let (code, stdout, stderr) = run_edda(
            &["claim", "check", "src/main.rs", "--json"],
            repo.path(),
            store.path(),
        );
        assert_eq!(
            code, 2,
            "malformed board line must exit 2, got stdout={stdout:?} stderr={stderr:?}"
        );
    }

    #[test]
    fn e2e_missing_board_is_clear() {
        // A missing board file legitimately means an empty board: exit 0.
        let repo = tempfile::tempdir().expect("repo tempdir");
        std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
        let store = tempfile::tempdir().expect("store tempdir");
        let (code, stdout, stderr) = run_edda(
            &["claim", "check", "src/main.rs", "--json"],
            repo.path(),
            store.path(),
        );
        assert_eq!(
            code, 0,
            "missing board must stay clear, got stdout={stdout:?} stderr={stderr:?}"
        );
    }

    #[test]
    fn e2e_non_check_label_rejects_trailing_positional() {
        // Pre-GH-562 this was a clap usage error (exit 2); the shortcut must
        // not silently record a pathless claim from a typo.
        let repo = tempfile::tempdir().expect("repo tempdir");
        std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
        let project_id = edda_store::project_id(repo.path());
        let store = tempfile::tempdir().expect("store tempdir");
        let (code, stdout, stderr) = run_edda(
            &["claim", "auth", "extra", "--session", "probe"],
            repo.path(),
            store.path(),
        );
        assert_eq!(
            code, 2,
            "expected usage error, got stdout={stdout:?} stderr={stderr:?}"
        );
        assert!(
            !board_file(store.path(), &project_id).exists(),
            "a pathless claim must not be recorded"
        );
    }

    #[test]
    fn e2e_non_check_label_rejects_json_flag() {
        let repo = tempfile::tempdir().expect("repo tempdir");
        std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
        let store = tempfile::tempdir().expect("store tempdir");
        let (code, stdout, stderr) =
            run_edda(&["claim", "auth", "--json"], repo.path(), store.path());
        assert_eq!(
            code, 2,
            "--json is check-only, got stdout={stdout:?} stderr={stderr:?}"
        );
    }

    #[test]
    fn e2e_non_check_claim_still_records_paths() {
        // The plain claim path must keep working byte-identically.
        let repo = tempfile::tempdir().expect("repo tempdir");
        std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
        let project_id = edda_store::project_id(repo.path());
        let store = tempfile::tempdir().expect("store tempdir");
        let (code, stdout, stderr) = run_edda(
            &[
                "claim",
                "auth",
                "--paths",
                "src/auth/*",
                "--session",
                "probe",
            ],
            repo.path(),
            store.path(),
        );
        assert_eq!(
            code, 0,
            "plain claim must still work, got stdout={stdout:?} stderr={stderr:?}"
        );
        let board =
            std::fs::read_to_string(board_file(store.path(), &project_id)).expect("board file");
        assert!(
            board.contains("\"paths\":[\"src/auth/*\"]"),
            "claim event must carry the paths, got: {board}"
        );
    }
    #[test]
    fn invalid_glob_is_an_error_not_a_clear() {
        // `[` without a closing bracket is unparseable for globset. Deciding
        // "disjoint" on it would fail open, so it must be an error that the
        // CLI surfaces as exit 2.
        assert!(surfaces_intersect("src/[oops", "src/[oops").is_err());
        assert!(surfaces_intersect("src/[oops", "src/fine.rs").is_err());
        let claims = vec![claim("g", "s", &["src/[oops"])];
        assert!(check(&claims, &["src/fine.rs"]).is_err());
    }

    #[test]
    fn engine_witnesses_are_confirmed_by_globset() {
        // Property: whenever the engine reports an intersection it must also
        // produce a string that real globset matches against BOTH patterns.
        // This pins the NFA semantics to globset 0.4.18's (empty brace
        // branches are the one deliberate over-approximation and are
        // therefore not in the corpus).
        let corpus = [
            "crates/**/cmd_*",
            "crates/edda-cli/src/*claim.rs",
            "crates/edda-*/src/cmd_claim.rs",
            "crates/*cli/src/cmd_claim.rs",
            "crates/edda-cli/src/cmd_[ab]*",
            "*cmd_a?*",
            "crates/{edda-cli,docs}/src/main.rs",
            "crates/edda-cli/src/*.rs",
            "ab*",
            "*cd",
            "*claim.rs",
            "*cmd_claim.rs",
            "src/*",
            "src/*.rs",
            "docs/*.md",
            "crates/**",
            "docs/x.md",
            "a/**/b",
            "a/x",
            "a/**",
            "**/b",
            "**",
            "*",
            "?",
            "a?c",
            "[!a]",
            "[a-c]x",
            "[]]",
            "[-a]",
            "[a-]",
            "[^a]",
            "b",
            "c",
            "a,b",
            "x{y/z,w}",
            "a**b",
            "***",
            "src/main.rs",
            "src/*.md",
        ];
        for a in corpus {
            for b in corpus {
                let na = build_nfa(&normalize_token(a)).expect("parses");
                let nb = build_nfa(&normalize_token(b)).expect("parses");
                let (hit, witness) = nfa_pair_intersects(&na, &nb);
                if hit {
                    let w = witness.expect("hit must carry a witness");
                    let ma = globset::Glob::new(&normalize_token(a))
                        .expect("valid")
                        .compile_matcher();
                    let mb = globset::Glob::new(&normalize_token(b))
                        .expect("valid")
                        .compile_matcher();
                    assert!(
                        ma.is_match(&w) && mb.is_match(&w),
                        "engine witness {w:?} for {a:?} vs {b:?} is not confirmed by globset"
                    );
                } else {
                    assert!(witness.is_none());
                }
            }
        }
    }

    #[test]
    fn json_report_serializes_conflict_list() {
        let claims = vec![claim("peer-j", "s12", &["crates/edda-cli/src/main.rs"])];
        let report = check(&claims, &["crates/edda-cli/src/main.rs"]).expect("globs parse");
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["conflicts"][0]["label"], "peer-j");
        assert_eq!(json["conflicts"][0]["session_id"], "s12");
        assert_eq!(
            json["conflicts"][0]["intersections"][0]["claim_path"],
            "crates/edda-cli/src/main.rs"
        );
    }

    #[test]
    fn exit_codes_follow_report() {
        assert_eq!(exit_code_for(&CheckReport::default()), 0);
        let conflict = CheckReport {
            conflicts: vec![ClaimConflict {
                label: "x".into(),
                session_id: "y".into(),
                intersections: vec![],
            }],
        };
        assert_eq!(exit_code_for(&conflict), 1);
    }

    /// End-to-end exit-code contract: spawn the real binary against a
    /// temporary coordination board. `cargo test` places the package bin
    /// next to the deps directory that holds this test binary.
    fn edda_bin() -> std::path::PathBuf {
        let exe = std::env::current_exe().expect("current_exe");
        // current_exe = target/debug/deps/<test>-<hash>.exe
        let dir = exe
            .parent()
            .and_then(|d| d.parent())
            .expect("deps/.. = target/debug")
            .to_path_buf();
        dir.join(format!("edda{}", std::env::consts::EXE_SUFFIX))
    }

    fn write_board(store_root: &Path, project_id: &str, lines: &[String]) {
        let dir = store_root.join("projects").join(project_id).join("state");
        std::fs::create_dir_all(&dir).expect("state dir");
        std::fs::write(dir.join("coordination.jsonl"), lines.join("\n") + "\n")
            .expect("coordination.jsonl");
    }

    fn coord_event(session: &str, label: &str, paths: &[&str]) -> String {
        serde_json::json!({
            "ts": "2026-01-01T00:00:00Z",
            "session_id": session,
            "event_type": "claim",
            "payload": { "label": label, "paths": paths }
        })
        .to_string()
    }

    #[test]
    fn e2e_exit_codes_conflict_and_disjoint() {
        let bin = edda_bin();
        if !bin.exists() {
            panic!("edda binary not found at {}", bin.display());
        }
        let repo = tempfile::tempdir().expect("repo tempdir");
        std::fs::create_dir_all(repo.path().join(".git")).expect("fake .git");
        let project_id = edda_store::project_id(repo.path());

        // Conflict case: an active claim overlaps the query surface.
        let store = tempfile::tempdir().expect("store tempdir");
        write_board(
            store.path(),
            &project_id,
            &[coord_event("sess-1", "peer-a", &["crates/edda-cli/src/*"])],
        );
        let out = std::process::Command::new(&bin)
            .args(["claim", "check", "crates/edda-cli/src/main.rs"])
            .current_dir(repo.path())
            .env("EDDA_STORE_ROOT", store.path())
            .output()
            .expect("spawn edda");
        assert_eq!(
            out.status.code(),
            Some(1),
            "stdout: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("peer-a"),
            "conflict must name the claim label"
        );
        assert!(stdout.contains("sess-1"), "conflict must name the session");

        // Disjoint case: active claim exists but covers nothing overlapping.
        let store = tempfile::tempdir().expect("store tempdir");
        write_board(
            store.path(),
            &project_id,
            &[coord_event(
                "sess-2",
                "peer-b",
                &["crates/edda-conductor/src/plan/**"],
            )],
        );
        let out = std::process::Command::new(&bin)
            .args(["claim", "check", "crates/edda-cli/src/main.rs"])
            .current_dir(repo.path())
            .env("EDDA_STORE_ROOT", store.path())
            .output()
            .expect("spawn edda");
        assert_eq!(
            out.status.code(),
            Some(0),
            "stdout: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );

        // JSON case: machine-readable conflict list.
        let out = std::process::Command::new(&bin)
            .args(["claim", "check", "crates/edda-cli/src/main.rs", "--json"])
            .current_dir(repo.path())
            .env("EDDA_STORE_ROOT", store.path())
            .output()
            .expect("spawn edda");
        assert_eq!(out.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&out.stdout);
        let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON report");
        assert!(parsed["conflicts"]
            .as_array()
            .expect("conflicts array")
            .is_empty());
    }
}
