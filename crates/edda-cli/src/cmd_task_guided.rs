use edda_core::guided_execution::{
    render_execution_brief, validate_execution_brief_raw, ExecutionBriefInputV1,
    MAX_EXECUTION_BRIEF_INPUT_BYTES,
};
use edda_ledger::{AcceptedExecutionBriefV1, Ledger};
use std::io::Read;
use std::path::Path;

pub(super) fn prepare(
    repo_root: &Path,
    file: &Path,
    asserted_author: Option<&str>,
    session: Option<&str>,
    authority_token_file: &Path,
    json: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        asserted_author.is_none(),
        "--author is not authority; use the locally sealed control capability"
    );
    let input_file = std::fs::File::open(file)
        .map_err(|_| anyhow::anyhow!("execution brief input file is unavailable"))?;
    anyhow::ensure!(
        input_file.metadata()?.is_file(),
        "execution brief input path is not a file"
    );
    let mut bytes = Vec::with_capacity(MAX_EXECUTION_BRIEF_INPUT_BYTES.min(8 * 1024) + 1);
    input_file
        .take(MAX_EXECUTION_BRIEF_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_EXECUTION_BRIEF_INPUT_BYTES,
        "execution brief input exceeds its size bound"
    );
    validate_execution_brief_raw(&bytes)?;
    let input: ExecutionBriefInputV1 = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid ExecutionBriefInputV1 schema"))?;
    let project_id = edda_store::project_id(repo_root);
    let session = crate::cmd_bridge::resolve_session_id(session, &project_id, "task-prepare")?.0;
    let authority_token =
        crate::cmd_control::read_secret(authority_token_file, "control authority token")?;
    let accepted =
        Ledger::open(repo_root)?.prepare_execution_brief(input, &session, &authority_token)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "PREPARED_LOCAL",
                "brief_id": accepted.brief.brief_id,
                "brief_event_id": accepted.event_id,
                "content_digest": accepted.content_digest,
                "runtime_profile": accepted.brief.runtime_profile,
                "task_ref": accepted.brief.task_ref,
                "authority": {
                    "principal_id": accepted.authority.principal_id,
                    "session_id": accepted.authority.session_id,
                    "manifest_event_id": accepted.authority.authority_event_id,
                }
            }))?
        );
    } else {
        println!(
            "Prepared immutable execution brief {}.",
            accepted.brief.brief_id
        );
        println!("  event: {}", accepted.event_id);
        println!("  digest: {}", accepted.content_digest);
    }
    Ok(())
}

pub(super) fn show(
    repo_root: &Path,
    event_id: &str,
    digest: &str,
    json: bool,
) -> anyhow::Result<()> {
    let accepted = load(repo_root, event_id, digest)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&accepted.brief)?);
    } else {
        println!("[locally accepted execution brief; immutable event]");
        println!("{}", serde_json::to_string_pretty(&accepted.brief)?);
    }
    Ok(())
}

pub(super) fn render(
    repo_root: &Path,
    event_id: &str,
    digest: &str,
    json: bool,
) -> anyhow::Result<()> {
    let accepted = load(repo_root, event_id, digest)?;
    let rendered = render_execution_brief(&accepted.brief)?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "brief_event_id": accepted.event_id,
                "content_digest": accepted.content_digest,
                "rendered": rendered,
            })
        );
    } else {
        print!("{rendered}");
    }
    Ok(())
}

pub(super) fn load(
    repo_root: &Path,
    event_id: &str,
    digest: &str,
) -> anyhow::Result<AcceptedExecutionBriefV1> {
    Ledger::open_existing(repo_root)?.load_execution_brief(event_id, digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_prepare_fails_closed_without_local_control_authority() {
        let tmp = tempfile::tempdir().unwrap();
        Ledger::ensure_initialized(tmp.path()).unwrap();
        let path = tmp.path().join("flash.json");
        std::fs::write(
            &path,
            r#"{"runtime_profile":"flash","procedure":{"kind":"controller_authored","authored_by":"arbitrary"}}"#,
        )
        .unwrap();
        let before = Ledger::open_existing(tmp.path())
            .unwrap()
            .count_events()
            .unwrap();
        let error = prepare(
            tmp.path(),
            &path,
            None,
            Some("arbitrary"),
            &tmp.path().join("missing-token"),
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("invalid ExecutionBriefInputV1 schema"));
        let after = Ledger::open_existing(tmp.path())
            .unwrap()
            .count_events()
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn author_assertion_is_never_treated_as_authority() {
        let tmp = tempfile::tempdir().unwrap();
        Ledger::ensure_initialized(tmp.path()).unwrap();
        let path = tmp.path().join("brief.json");
        std::fs::write(&path, "{}").unwrap();
        let error = prepare(
            tmp.path(),
            &path,
            Some("controller"),
            None,
            &tmp.path().join("missing-token"),
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("--author is not authority"));
    }
}
