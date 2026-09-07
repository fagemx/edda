//! Atomic admission for scheduled writers; the existing board remains the claim carrier.
use anyhow::{bail, Context, Result};
use edda_bridge_claude::peers;
use std::fs::OpenOptions;
use std::path::Path;

pub struct Claim {
    project: String,
    session: String,
    released: bool,
}

/// Does this board claim still stand against a new writer?
///
/// A session that heartbeats is judged by the one shared session criterion,
/// exactly as before.
///
/// A bare-CLI claim (`cli-*`) never heartbeats — its claimant is a one-shot
/// process — so that criterion can only ever call it dead. GH-705 answered
/// that by treating every such claim as live, fail-closed, so a one-shot
/// writer could not be stomped on mid-write. Unconditionally, though,
/// "fail-closed" reads as "never expires": GH-1018 found 100 claims left from
/// July and August still refusing September lanes, with no `unclaim` able to
/// clear them, which made the guard something lanes had to route around
/// rather than obey.
///
/// The claim's own timestamp is judgeable even when its session's heartbeat
/// is not, and `edda peers --json` already publishes exactly that verdict
/// (GH-569). Sharing that one rule bounds the bare-CLI case in time without
/// weakening it: a claim written moments ago still refuses a second writer.
fn claim_still_stands(project: &str, claim: &peers::ClaimEntry, now_epoch: u64) -> bool {
    if peers::liveness::classify_session_liveness(project, &claim.session_id).is_live() {
        return true;
    }
    crate::cmd_claim::is_bare_cli_session(&claim.session_id)
        && !peers::liveness::claim_is_stale_at(&claim.ts, now_epoch)
}

pub fn acquire(cwd: &Path, session: &str, paths: &[String]) -> Result<Option<Claim>> {
    if paths.is_empty() {
        return Ok(None);
    }
    let project = edda_store::project_id(cwd);
    edda_store::ensure_dirs(&project)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(edda_store::project_dir(&project).join("dispatch-admission.lock"))?;
    lock.lock().context("lock dispatch admission")?;
    let claims = crate::cmd_claim::read_active_claims(&project)?;
    let now_epoch = peers::liveness::now_epoch();
    let live: Vec<_> = claims
        .into_iter()
        .filter(|c| claim_still_stands(&project, c, now_epoch))
        .collect();
    if live.iter().any(|claim| claim.session_id == session) {
        bail!("session {session} already owns a live dispatch claim");
    }
    let refs: Vec<_> = paths.iter().map(String::as_str).collect();
    let report = crate::cmd_claim::check(&live, &refs).map_err(anyhow::Error::msg)?;
    if !report.conflicts.is_empty() {
        bail!(
            "owned paths conflict with live claim(s): {}",
            report
                .conflicts
                .iter()
                .map(|c| c.session_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let heartbeat = edda_conductor::runner::heartbeat::LaneHeartbeat {
        cwd: cwd.to_path_buf(),
        session_id: session.into(),
        plan: "dispatch".into(),
        phase: "dispatch".into(),
        attempt: 1,
    };
    heartbeat
        .try_write("starting")
        .context("register writer heartbeat")?;
    peers::write_claim_with_subject(&project, session, "dispatch", paths, None);
    let recorded = crate::cmd_claim::read_active_claims(&project)?;
    if !recorded
        .iter()
        .any(|c| c.session_id == session && c.paths == paths)
    {
        bail!("dispatch claim was not recorded; refusing to start an invisible writer");
    }
    Ok(Some(Claim {
        project,
        session: session.into(),
        released: false,
    }))
}

impl Claim {
    /// Release only the claim and heartbeat this guard admitted.  A successful
    /// agent turn is not committed to the caller until the board confirms the
    /// release; Drop remains only a fallback for unwind paths.
    pub fn release(mut self) -> Result<()> {
        self.release_inner()?;
        self.released = true;
        Ok(())
    }

    fn release_inner(&self) -> Result<()> {
        peers::write_unclaim(&self.project, &self.session);
        let claims = crate::cmd_claim::read_active_claims(&self.project)?;
        if claims.iter().any(|claim| claim.session_id == self.session) {
            bail!(
                "dispatch claim release was not confirmed for {}",
                self.session
            );
        }
        let heartbeat = edda_store::heartbeat_path(&self.project, &self.session);
        match std::fs::remove_file(&heartbeat) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error)
                .with_context(|| format!("remove dispatch heartbeat {}", heartbeat.display())),
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if !self.released {
            if let Err(error) = self.release_inner() {
                eprintln!("warning: dispatch claim fallback release failed: {error:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_writer_is_refused_and_drop_releases_the_claim() {
        let _store = crate::test_support::isolated_store();
        let cwd = tempfile::tempdir().expect("dispatch claim cwd");
        let paths = vec!["crates/edda-cli/src/cmd_dispatch.rs".to_owned()];

        let first = acquire(cwd.path(), "cli-first", &paths)
            .expect("first writer admission")
            .expect("first writer claim");
        let conflict = match acquire(cwd.path(), "cli-second", &paths) {
            Ok(_) => panic!("second writer must be refused"),
            Err(error) => error,
        };
        assert!(conflict.to_string().contains("cli-first"));

        drop(first);
        let second = acquire(cwd.path(), "cli-second", &paths)
            .expect("released writer admission")
            .expect("released writer claim");
        drop(second);

        let project = edda_store::project_id(cwd.path());
        assert!(
            crate::cmd_claim::read_active_claims(&project)
                .expect("read released board")
                .is_empty(),
            "the final claim must be released"
        );
    }

    #[test]
    fn same_session_disjoint_writer_is_refused_without_releasing_the_first() {
        let _store = crate::test_support::isolated_store();
        let cwd = tempfile::tempdir().expect("dispatch claim cwd");
        let first_paths = vec!["src/first.rs".to_owned()];
        let second_paths = vec!["src/second.rs".to_owned()];

        let first = acquire(cwd.path(), "cli-same", &first_paths)
            .expect("first writer admission")
            .expect("first writer claim");
        let error = match acquire(cwd.path(), "cli-same", &second_paths) {
            Ok(_) => panic!("same session must not replace its live claim"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("cli-same"), "{error:#}");

        let project = edda_store::project_id(cwd.path());
        assert_eq!(
            crate::cmd_claim::read_active_claims(&project).expect("read first claim")[0].paths,
            first_paths,
            "the refused writer must not replace the live claim"
        );
        first.release().expect("release first claim");
        acquire(cwd.path(), "cli-same", &second_paths)
            .expect("session is reusable after its release")
            .expect("second claim")
            .release()
            .expect("release second claim");
    }

    #[test]
    fn a_stale_bare_cli_claim_no_longer_refuses_a_new_writer() {
        let _store = crate::test_support::isolated_store();
        let cwd = tempfile::tempdir().expect("dispatch claim cwd");
        let project = edda_store::project_id(cwd.path());
        let paths = vec!["docs/reference/cli.md".to_owned()];
        // The shape GH-1018 found on a long-lived board: a bare-CLI claim
        // from three weeks ago, no heartbeat ever written for it, nothing
        // that expires it.
        crate::test_support::write_aged_claim(
            &project,
            "cli-gh466-round1-fixes",
            60 * 60 * 24 * 22,
            &paths,
        );

        let claim = acquire(cwd.path(), "lane-gh1018", &paths)
            .expect("a three-week-old claim must not refuse a new writer")
            .expect("writer claim");
        claim.release().expect("release the admitted claim");
    }

    #[test]
    fn a_fresh_bare_cli_claim_still_refuses_a_new_writer() {
        let _store = crate::test_support::isolated_store();
        let cwd = tempfile::tempdir().expect("dispatch claim cwd");
        let project = edda_store::project_id(cwd.path());
        let paths = vec!["docs/reference/cli.md".to_owned()];
        // GH-705's case, unweakened: a one-shot CLI writer that claimed
        // seconds ago has no heartbeat either, and must still be honoured.
        crate::test_support::write_aged_claim(&project, "cli-just-now", 5, &paths);

        let error = match acquire(cwd.path(), "lane-gh1018", &paths) {
            Ok(_) => panic!("a fresh bare-CLI claim must still refuse a second writer"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("cli-just-now"), "{error:#}");
    }

    #[test]
    fn confirmed_release_failure_is_returned_to_the_caller() {
        let _store = crate::test_support::isolated_store();
        let cwd = tempfile::tempdir().expect("dispatch claim cwd");
        let paths = vec!["src/owned.rs".to_owned()];
        let claim = acquire(cwd.path(), "cli-release", &paths)
            .expect("writer admission")
            .expect("writer claim");
        let project = edda_store::project_id(cwd.path());
        let board = edda_store::project_dir(&project)
            .join("state")
            .join("coordination.jsonl");
        std::fs::write(&board, "not valid json\n").expect("damage isolated board");

        let error = claim
            .release()
            .expect_err("release confirmation must fail closed");
        assert!(
            error.to_string().contains("coordination board"),
            "release error must be visible: {error:#}"
        );
    }
}
