use std::path::Path;

use super::request::{has_live_sessions, resolve_session_id};

/// The lines `claim` prints about what a new claim did to the session's old one.
///
/// Split out so the strings — and, more importantly, the *absence* of a
/// `released:` line — can be asserted. Reporting a release that did not happen
/// is the same false success this disclosure exists to remove.
pub(super) fn claim_disclosure(
    previous: Option<&edda_bridge_claude::peers::ClaimEntry>,
    label: &str,
    paths: &[String],
) -> Vec<String> {
    let Some(previous) = previous else {
        return vec![format!("Claimed scope: {label}")];
    };

    // Only paths the new claim no longer covers were actually let go. Naming
    // `previous.paths` wholesale said "released" about paths this very command
    // had just re-claimed — and an idempotent re-claim hits exactly that, since
    // tier 4 mints a deterministic `cli-<label>` and board claims never expire,
    // so a bare-shell restart re-runs the same command against its own
    // surviving claim.
    let released: Vec<&str> = previous
        .paths
        .iter()
        .filter(|p| !paths.contains(p))
        .map(String::as_str)
        .collect();
    let gained = paths.iter().any(|p| !previous.paths.contains(p));

    let mut lines = Vec::new();
    if previous.label != label {
        lines.push(format!(
            "Claimed scope: {label} (replaces this session's earlier claim on {})",
            previous.label
        ));
    } else if released.is_empty() && !gained {
        lines.push(format!("Re-claimed scope: {label} (unchanged)"));
    } else if released.is_empty() {
        lines.push(format!("Re-claimed scope: {label} (paths added)"));
    } else {
        lines.push(format!(
            "Re-claimed scope: {label} (previous paths replaced)"
        ));
    }
    if !released.is_empty() {
        lines.push(format!("  released: {}", released.join(", ")));
    }
    lines
}

/// `edda bridge claude claim <label>` — claim a coordination scope
///
/// The board folds claims into one per session, so a second claim replaces the
/// first rather than adding to it. That is the right shape — it is how a
/// session narrows or moves its scope, and how a restart re-claims
/// idempotently — but it used to happen in silence, so a worker could believe
/// it held two scopes while peers saw one (GH-488). The replacement is now
/// named.
pub fn claim(
    repo_root: &Path,
    label: &str,
    paths: &[String],
    subject: Option<&str>,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, label)?;

    let replaced = edda_bridge_claude::peers::compute_board_state(&project_id)
        .claims
        .into_iter()
        .find(|c| c.session_id == session_id);

    edda_bridge_claude::peers::write_claim_with_subject(
        &project_id,
        &session_id,
        label,
        paths,
        subject,
    );

    // GH-705: the board entry is the claim's entire machine visibility —
    // `claim check` counts a claim it can read, never one it cannot. The
    // append is best-effort by shape (it serves fire-and-forget hook paths
    // too), so verify the fold actually sees the new claim before printing
    // success: a lost write must not look like a claimed scope, or the
    // surface reads clear while the occupation is real. This deliberately
    // does NOT write a heartbeat for the minted session: the claimant is a
    // one-shot process, so a heartbeat it writes once ages with nothing to
    // refresh it and proves nothing after stale_secs — and its residue made
    // every subsequent bare claim hit resolve_session_id's live-session
    // ambiguity refusal. The machine gate instead counts a `cli-*` claim
    // for as long as it stands on the board (cmd_claim: unjudgeable,
    // fail-closed), which does not decay with time.
    //
    // Distinguish the new entry from any replaced entry: the entry must exist,
    // match the just-written label, paths, and subject, and its timestamp must
    // advance past the replaced entry (so a lost write on re-claiming,
    // narrowing, or identical re-claim does not falsely pass by re-reading the
    // prior state).
    let current = edda_bridge_claude::peers::compute_board_state(&project_id)
        .claims
        .into_iter()
        .find(|c| c.session_id == session_id);
    let recorded = current.as_ref().is_some_and(|c| {
        c.label == label
            && c.paths == paths
            && c.subject.as_deref() == subject
            && replaced.as_ref().map(|r| &r.ts) != Some(&c.ts)
    });
    if !recorded {
        anyhow::bail!(
            "claim for session {session_id} was not recorded on the coordination board; \
             refusing to report a claim the machine gate cannot see"
        );
    }

    for line in claim_disclosure(replaced.as_ref(), label, paths) {
        println!("{line}");
    }
    if let Some(sub) = subject {
        println!("  subject: {sub}");
    }
    if !paths.is_empty() {
        println!("  paths: {}", paths.join(", "));
    }
    println!("  session: {session_id}");
    Ok(())
}

/// `edda unclaim [--session <id>]` — release a coordination scope.
///
/// Unlike `claim`, this verb never mints a session id. `claim` may invent
/// `cli-<label>` because it is creating the claim; `unclaim` has to name one
/// that already exists, so it resolves against the board and refuses rather
/// than reporting success for a session that holds nothing (GH-486).
///
/// It also never guesses from the board. A caller with no session identity
/// cannot know that a claim is its own, and releasing someone else's would
/// drop the off-limits protection their live session depends on.
///
/// The automatic session-end path does not come through here — bridges call
/// `peers::write_unclaim` directly — so refusing costs a hooked session
/// nothing. A CI teardown that runs the verb unconditionally passes
/// `--if-claimed` and gets exit 0 when there is nothing left to release.
pub fn unclaim(
    repo_root: &Path,
    cli_session: Option<&str>,
    if_claimed: bool,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let board = edda_bridge_claude::peers::compute_board_state(&project_id);
    let session_id = match resolve_unclaim_target(cli_session, &project_id, &board.claims) {
        Ok(sid) => sid,
        // Teardown runs unconditionally and must not fail a job for the normal
        // case of having nothing left to release (GH-488). It reports what it
        // actually found: saying "nothing to unclaim" over a populated board
        // would be false, and the point of this verb is to stop reporting
        // releases that did not happen.
        Err(e) if if_claimed => {
            println!("Released nothing: {e}");
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    let held: Vec<&edda_bridge_claude::peers::ClaimEntry> = board
        .claims
        .iter()
        .filter(|c| c.session_id == session_id)
        .collect();
    if held.is_empty() {
        if if_claimed {
            println!("Nothing to unclaim for session {session_id}");
            return Ok(());
        }
        anyhow::bail!(
            "session {session_id} holds no claim; nothing was released.\n\
             Pass --session with one of the ids below.\n{}",
            describe_claims(&board.claims)
        );
    }

    edda_bridge_claude::peers::write_unclaim(&project_id, &session_id);
    let labels: Vec<&str> = held.iter().map(|c| c.label.as_str()).collect();
    println!(
        "Unclaimed scope for session: {session_id} ({})",
        labels.join(", ")
    );
    Ok(())
}

/// Name the session whose claim `unclaim` should release.
///
/// Explicit `--session` wins, then process-carried `EDDA_SESSION_ID`.
/// Heartbeats are evidence that identity is ambiguous, never evidence that a
/// session belongs to this process. Refuse and show the board instead, so the
/// id for `--session` is in the error.
fn resolve_unclaim_target(
    cli_session: Option<&str>,
    project_id: &str,
    claims: &[edda_bridge_claude::peers::ClaimEntry],
) -> anyhow::Result<String> {
    if let Some(sid) = cli_session.filter(|s| !s.is_empty()) {
        return Ok(sid.to_string());
    }
    if let Ok(sid) = std::env::var("EDDA_SESSION_ID") {
        if !sid.is_empty() {
            return Ok(sid);
        }
    }
    if has_live_sessions(project_id) {
        anyhow::bail!(
            "cannot prove which live session belongs to this process, so --session is required.\n{}",
            describe_claims(claims)
        );
    }

    // No identity of our own, so there is nothing to infer from. Do NOT fall
    // back to "the sole claim on the board": a caller without a session cannot
    // know that claim is theirs, and releasing it drops the off-limits
    // protection its real owner is relying on.
    if claims.is_empty() {
        anyhow::bail!("no claims on the board; nothing to unclaim");
    }
    anyhow::bail!(
        "cannot tell which claim is yours, so --session is required.\n{}",
        describe_claims(claims)
    );
}

/// Render the board's claims for an error message, so the reader can copy the
/// session id straight into `--session` instead of hunting for it.
fn describe_claims(claims: &[edda_bridge_claude::peers::ClaimEntry]) -> String {
    if claims.is_empty() {
        return "The board holds no claims.".to_string();
    }
    let mut out = String::from("Claims on the board:");
    for c in claims {
        out.push_str(&format!("\n  {} — {}", c.session_id, c.label));
        if !c.paths.is_empty() {
            out.push_str(&format!(" ({})", c.paths.join(", ")));
        }
    }
    out
}
