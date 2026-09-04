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
    let live: Vec<_> = claims
        .into_iter()
        .filter(|c| {
            // Same-session concurrent dispatch is a second writer too; do not ignore it.
            c.session_id.starts_with("cli-")
                || peers::liveness::classify_session_liveness(&project, &c.session_id).is_live()
        })
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
