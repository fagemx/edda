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
//! - 2 — usage error (no query paths given)

use anyhow::Context;
use serde::Serialize;
use std::path::Path;

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
    let board = edda_bridge_claude::peers::compute_board_state(&project_id);
    let query_refs: Vec<&str> = query.iter().map(String::as_str).collect();
    let report = check(&board.claims, &query_refs);

    if json {
        let out = serde_json::to_string_pretty(&report).context("serialize claim check report")?;
        println!("{out}");
    } else if report.conflicts.is_empty() {
        if board.claims.is_empty() {
            println!("No active claims on the coordination board; surface is clear.");
        } else {
            println!(
                "No conflicts: {} active claim(s) checked against {} query path(s).",
                board.claims.len(),
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

/// Pure intersection of a query surface against claims (unit-testable core).
///
/// Claims whose recorded path list is empty cover nothing (a label-only
/// claim), so they never conflict. Claims are folded one-per-session by the
/// board, so each claim appears at most once in the report.
pub fn check(claims: &[edda_bridge_claude::peers::ClaimEntry], query: &[&str]) -> CheckReport {
    let mut conflicts = Vec::new();
    for c in claims {
        if c.paths.is_empty() {
            continue;
        }
        let mut intersections = Vec::new();
        for q in query {
            for claimed in &c.paths {
                if surfaces_intersect(q, claimed) {
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
    CheckReport { conflicts }
}

/// Whether a query token and a claim token can name the same file.
///
/// - literal vs literal: exact equality
/// - glob vs literal: globset match of the pattern against the literal
/// - glob vs glob: each pattern is matched against a concrete "witness"
///   derived from the other (each wildcard run becomes a one-character
///   filler). This catches broad-vs-narrow overlaps like `src/*` vs
///   `src/cmd_*.rs`; it can miss an overlap that requires a filler longer
///   than one character between two anchored fragments (`ab*` vs `*cd`).
///   For a scope-conflict check that errs on the side of "disjoint", which
///   the operator resolves by reading the named claims.
fn surfaces_intersect(a: &str, b: &str) -> bool {
    match (has_wildcard(a), has_wildcard(b)) {
        (false, false) => a == b,
        (true, false) => glob_matches(a, b),
        (false, true) => glob_matches(b, a),
        (true, true) => glob_matches(a, &witness(b)) || glob_matches(b, &witness(a)),
    }
}

fn has_wildcard(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[') || pattern.contains('{')
}

fn glob_matches(pattern: &str, candidate: &str) -> bool {
    match globset::Glob::new(pattern) {
        Ok(glob) => glob.compile_matcher().is_match(candidate),
        // An unparseable pattern degrades to a literal comparison rather than
        // silently matching everything or nothing.
        Err(_) => pattern == candidate,
    }
}

/// A concrete path shape derived from a glob: each wildcard run collapses to
/// one filler character, so `src/cmd_*.rs` becomes `src/cmd_z.rs`.
fn witness(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut in_wildcard = false;
    for ch in pattern.chars() {
        if ch == '*' {
            if !in_wildcard {
                out.push('z');
                in_wildcard = true;
            }
        } else {
            in_wildcard = false;
            out.push(ch);
        }
    }
    out
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

    #[test]
    fn exact_overlap_conflicts() {
        let claims = vec![claim(
            "peer-a",
            "s1",
            &["crates/edda-conductor/src/agent/codex_rpc.rs"],
        )];
        let report = check(&claims, &["crates/edda-conductor/src/agent/codex_rpc.rs"]);
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
        let report = check(&claims, &["crates/edda-cli/src/*"]);
        assert_eq!(labels(&report), vec!["peer-b"]);
        assert_eq!(exit_code_for(&report), 1);
    }

    #[test]
    fn glob_vs_literal_overlap_conflicts() {
        let claims = vec![claim("peer-c", "s3", &["crates/edda-cli/src/*"])];
        let report = check(&claims, &["crates/edda-cli/src/main.rs"]);
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
        let report = check(&claims, &["crates/edda-conductor/src/agent/codex_rpc.rs"]);
        assert!(report.conflicts.is_empty());
        assert_eq!(exit_code_for(&report), 0);
    }

    #[test]
    fn disjoint_globs_exit_zero() {
        let claims = vec![
            claim("plan-owner", "s6", &["crates/edda-conductor/src/plan/**"]),
            claim("cli-owner", "s7", &["crates/edda-cli/src/cmd_*.rs"]),
        ];
        let report = check(&claims, &["crates/edda-conductor/src/agent/*"]);
        assert!(report.conflicts.is_empty());
        assert_eq!(exit_code_for(&report), 0);
    }

    #[test]
    fn no_claims_is_disjoint() {
        let report = check(&[], &["crates/edda-cli/src/*"]);
        assert!(report.conflicts.is_empty());
        assert_eq!(exit_code_for(&report), 0);
    }

    #[test]
    fn claim_without_paths_covers_nothing() {
        let claims = vec![claim("label-only", "s8", &[])];
        let report = check(&claims, &["crates/edda-cli/src/*"]);
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
        );
        assert_eq!(labels(&report), vec!["dirty"]);
        assert_eq!(report.conflicts[0].intersections.len(), 1);
        assert_eq!(
            report.conflicts[0].intersections[0].query,
            "crates/edda-cli/src/main.rs"
        );
    }

    #[test]
    fn invalid_glob_degrades_to_literal() {
        // `[` without a closing bracket is unparseable for globset.
        assert!(surfaces_intersect("src/[oops", "src/[oops"));
        assert!(!surfaces_intersect("src/[oops", "src/fine.rs"));
    }

    #[test]
    fn json_report_serializes_conflict_list() {
        let claims = vec![claim("peer-j", "s12", &["crates/edda-cli/src/main.rs"])];
        let report = check(&claims, &["crates/edda-cli/src/main.rs"]);
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
