use clap::{Subcommand, ValueEnum};
use edda_core::guided_execution::{
    validate_control_raw, ControlAdjudicationInputV1, ControlCommandProfileV1,
    ControlCompileInputV1,
};
use edda_ledger::{ControlAuthorityProvision, ControlMergeCapabilityProvision, Ledger};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum AuthorityProfileArg {
    Strong,
    Flash,
}

impl From<AuthorityProfileArg> for ControlCommandProfileV1 {
    fn from(value: AuthorityProfileArg) -> Self {
        match value {
            AuthorityProfileArg::Strong => Self::Strong,
            AuthorityProfileArg::Flash => Self::Flash,
        }
    }
}

#[derive(Subcommand)]
pub enum ControlAuthorityCmd {
    /// Issue a local HMAC-sealed capability after actor/session/RBAC checks
    Issue {
        #[arg(long)]
        principal: String,
        #[arg(long)]
        session: String,
        #[arg(long, value_enum)]
        profile: AuthorityProfileArg,
        #[arg(long)]
        portable_repo_id: Option<String>,
        #[arg(long)]
        expires_at: String,
        /// Pre-provisioned local issuer secret; possession is required to mint authority
        #[arg(long)]
        issuer_token_file: PathBuf,
        /// Private destination for the one-time strong-controller bearer
        #[arg(long)]
        token_out: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ControlCmd {
    /// Provision local control authority; never portable or manifest-carried
    Authority {
        #[command(subcommand)]
        cmd: ControlAuthorityCmd,
    },
    /// Strong path: atomically accept brief inputs and one control manifest
    Compile {
        file: PathBuf,
        /// Session correlation; a matching string is not authority without the local seal
        #[arg(long)]
        session: Option<String>,
        /// Private file containing the issued strong-controller bearer
        #[arg(long)]
        authority_token_file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Bind a sealed host-local merge capability to an exact control subject
    AuthorizeMerge {
        control_id: String,
        #[arg(long)]
        source: String,
        #[arg(long)]
        principal: String,
        #[arg(long)]
        session: String,
        #[arg(long)]
        expires_at: String,
        #[arg(long)]
        issuer_token_file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Read-only complete control projection
    Status {
        control_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Read-only next action and state/target/expiry-bound opaque token
    Next {
        control_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Apply one token-bound local or bounded product control action
    Apply {
        control_id: String,
        #[arg(long)]
        token: String,
        #[arg(long)]
        json: bool,
    },
    /// Strong path: append a versioned authority-bound adjudication
    Adjudicate {
        control_id: String,
        #[arg(long)]
        state_version: u64,
        #[arg(long)]
        decision: PathBuf,
        #[arg(long)]
        session: Option<String>,
        /// Private file containing the issued strong-controller bearer
        #[arg(long)]
        authority_token_file: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

pub fn execute(cmd: ControlCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        ControlCmd::Authority { cmd } => authority(cmd, repo_root),
        ControlCmd::Compile {
            file,
            session,
            authority_token_file,
            json,
        } => {
            let bytes = read_bounded(&file, "control compile input")?;
            validate_control_raw(&bytes, "control compile input")?;
            let mut input: ControlCompileInputV1 = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid ControlCompileInputV1 schema"))?;
            bind_and_authenticate_control_repository(repo_root, &mut input)?;
            let session = resolve_session(repo_root, session.as_deref())?;
            let authority_token = read_secret(&authority_token_file, "control authority token")?;
            let result =
                Ledger::open(repo_root)?.compile_control(input, &session, &authority_token)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "COMPILED_LOCAL",
                        "control_id": result.manifest.control_id,
                        "manifest_event_id": result.manifest_event_id,
                        "manifest_digest": result.manifest_digest,
                        "briefs": result.briefs.iter().map(|brief| serde_json::json!({
                            "brief_id": brief.brief.brief_id,
                            "brief_event_id": brief.event_id,
                            "content_digest": brief.content_digest,
                        })).collect::<Vec<_>>(),
                    }))?
                );
            } else {
                println!("Compiled control {} locally.", result.manifest.control_id);
                println!("  manifest event: {}", result.manifest_event_id);
                println!("  manifest digest: {}", result.manifest_digest);
                println!("  accepted briefs: {}", result.briefs.len());
            }
            Ok(())
        }
        ControlCmd::AuthorizeMerge {
            control_id,
            source,
            principal,
            session,
            expires_at,
            issuer_token_file,
            json,
        } => {
            let issuer_token = read_secret(&issuer_token_file, "control issuer token")?;
            let capability = Ledger::open(repo_root)?.authorize_control_merge(
                &control_id,
                ControlMergeCapabilityProvision {
                    source: &source,
                    principal_id: &principal,
                    session_id: &session,
                    expires_at: &expires_at,
                    issuer_token: &issuer_token,
                },
            )?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "MERGE_AUTHORIZED_LOCAL",
                        "capability_id": capability.capability_id,
                        "control_id": capability.control_id,
                        "source": capability.source,
                        "principal_id": capability.principal_id,
                        "pr_number": capability.pr_number,
                        "expected_head_sha": capability.expected_head_sha,
                        "expected_base_sha": capability.expected_base_sha,
                        "manifest_digest": capability.manifest_digest,
                        "expires_at": capability.expires_at,
                    }))?
                );
            } else {
                println!(
                    "Authorized delegated merge for control {} and PR #{}.",
                    capability.control_id, capability.pr_number
                );
            }
            Ok(())
        }
        ControlCmd::Status { control_id, json } => {
            let status = Ledger::open_existing(repo_root)?.control_status(&control_id)?;
            print_value(&status, json)
        }
        ControlCmd::Next { control_id, json } => {
            let next = Ledger::open_existing(repo_root)?.control_next(&control_id)?;
            print_value(&next, json)
        }
        ControlCmd::Apply {
            control_id,
            token,
            json,
        } => {
            let ledger = Ledger::open(repo_root)?;
            let result = ledger.control_apply_with(&control_id, &token, |request| {
                crate::cmd_control_effects::apply(repo_root, &ledger, request)
            })?;
            print_value(&result, json)
        }
        ControlCmd::Adjudicate {
            control_id,
            state_version,
            decision,
            session,
            authority_token_file,
            json,
        } => {
            let bytes = read_bounded(&decision, "control adjudication input")?;
            validate_control_raw(&bytes, "control adjudication input")?;
            let input: ControlAdjudicationInputV1 = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid ControlAdjudicationInputV1 schema"))?;
            let session = resolve_session(repo_root, session.as_deref())?;
            let authority_token = read_secret(&authority_token_file, "control authority token")?;
            let status = Ledger::open(repo_root)?.adjudicate_control(
                &control_id,
                state_version,
                input,
                &session,
                &authority_token,
            )?;
            print_value(&status, json)
        }
    }
}

fn authority(cmd: ControlAuthorityCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        ControlAuthorityCmd::Issue {
            principal,
            session,
            profile,
            portable_repo_id,
            expires_at,
            issuer_token_file,
            token_out,
            json,
        } => {
            let issuer_token = read_secret(&issuer_token_file, "control issuer token")?;
            let authority = Ledger::open(repo_root)?.provision_control_authority(
                ControlAuthorityProvision {
                    principal_id: &principal,
                    session_id: &session,
                    command_profile: profile.into(),
                    portable_repo_id: portable_repo_id.as_deref(),
                    expires_at: &expires_at,
                    issuer_token: &issuer_token,
                    authority_token_out: &token_out,
                },
            )?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "AUTHORITY_ISSUED_LOCAL",
                        "capability_id": authority.capability_id,
                        "principal_id": authority.principal_id,
                        "session_id": authority.session_id,
                        "expires_at": authority.expires_at,
                    }))?
                );
            } else {
                println!(
                    "Issued local control authority {}.",
                    authority.capability_id
                );
                println!("  principal: {}", authority.principal_id);
                println!("  session: {}", authority.session_id);
                println!("  expires: {}", authority.expires_at);
            }
            Ok(())
        }
    }
}

fn resolve_session(repo_root: &Path, supplied: Option<&str>) -> anyhow::Result<String> {
    let project_id = edda_store::project_id(repo_root);
    Ok(crate::cmd_bridge::resolve_session_id(supplied, &project_id, "control")?.0)
}

fn bind_and_authenticate_control_repository(
    repo_root: &Path,
    input: &mut ControlCompileInputV1,
) -> anyhow::Result<()> {
    let requires_github = input.manifest.completion_condition
        != edda_core::guided_execution::ControlCompletionConditionV1::LocalPreparationOnly
        || input.manifest.tasks.iter().any(|task| !task.local_only)
        || input.manifest.merge_policy.required;
    if !requires_github {
        return Ok(());
    }
    let portable = input
        .manifest
        .basis
        .portable_repo_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("controlled GitHub workflow omits portable repository"))?;
    let repository =
        crate::cmd_review::github::GitHubRepository::for_control_compile(repo_root, portable)?;
    let canonical = repository.full_name();
    if let Some(declared) = input.manifest.basis.github_repository.as_deref() {
        anyhow::ensure!(
            declared == canonical,
            "control compile GitHub repository differs from the canonical remote"
        );
    }
    input.manifest.basis.github_repository = Some(canonical);
    let transport = crate::cmd_review::github::ControlledGitHubTransport::production()?;
    let login = crate::cmd_review::github::authenticated_login(&transport, repo_root, &repository)?;
    anyhow::ensure!(
        input.manifest.review_policy.verifier_identity != login,
        "controlled verifier login must differ from authenticated controller login"
    );
    Ok(())
}

fn read_bounded(path: &Path, label: &str) -> anyhow::Result<Vec<u8>> {
    let file =
        std::fs::File::open(path).map_err(|_| anyhow::anyhow!("{label} file is unavailable"))?;
    anyhow::ensure!(file.metadata()?.is_file(), "{label} path is not a file");
    let limit = edda_core::guided_execution::MAX_CONTROL_INPUT_BYTES;
    let mut bytes = Vec::with_capacity(limit.min(8 * 1024) + 1);
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= limit, "{label} exceeds its size bound");
    Ok(bytes)
}

pub(crate) fn read_secret(path: &Path, label: &str) -> anyhow::Result<String> {
    let bytes = edda_ledger::read_owner_only_file(path, label, 128)?;
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("{label} file is malformed"))?
        .trim();
    anyhow::ensure!(
        value.len() == 64
            && value
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()),
        "{label} file is malformed"
    );
    Ok(value.to_string())
}

fn print_value<T: serde::Serialize>(value: &T, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", serde_json::to_string(value)?);
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn issuer_and_bearer_file_reads_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("control-secret");
        std::fs::write(&path, "11".repeat(32)).unwrap();
        let error = read_secret(&path, "control authority token")
            .unwrap_err()
            .to_string();
        assert!(error.contains("CONTROL_UNAVAILABLE"), "{error}");
    }
}
