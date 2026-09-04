//! Atomic admission for scheduled writers; the existing board remains the claim carrier.
use anyhow::{bail, Context, Result};
use edda_bridge_claude::peers;
use std::fs::OpenOptions;
use std::path::Path;

pub struct Claim {
    project: String,
    session: String,
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
    }))
}

impl Drop for Claim {
    fn drop(&mut self) {
        peers::write_unclaim(&self.project, &self.session);
        match crate::cmd_claim::read_active_claims(&self.project) {
            Ok(claims) if !claims.iter().any(|c| c.session_id == self.session) => {}
            result => eprintln!("warning: dispatch claim release was not confirmed: {result:?}"),
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
}
