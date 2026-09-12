//! Local control authority and cryptographic sealing.
//!
//! Threat model: issue text, manifests, events, environment values, and
//! portable/imported bytes are untrusted. Authority requires a registered
//! actor, a live registered session at issuance/use, an explicit RBAC grant,
//! and possession of this local HMAC-sealed capability's private bearer. The
//! root key, capability record, and bearer are outside ledger events and
//! continuity bundles. On Unix, every secret is owned by the effective user
//! and denies group/world access before any bytes are read or written. A
//! malicious process already running as that same owner remains outside S6a's
//! boundary; group/world-readable files do not. Windows authority provisioning
//! and use fail closed until a reviewed safe owner-only storage abstraction is
//! available. Missing, malformed, or insecure local material always fails
//! closed.

use crate::{Ledger, WorkspaceLock};
use edda_core::guided_execution::{
    ControlAdjudicationRecordV1, ControlAuthorityProofV1, ControlCommandProfileV1,
};
use edda_core::policy::{evaluate_authz, load_actors_from_dir, load_policy_from_dir, AuthzRequest};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const AUTHORITY_RECORD_VERSION: u8 = 1;
const MERGE_CAPABILITY_VERSION: u8 = 1;
const KEY_BYTES: usize = 32;
const SESSION_FRESH_SECONDS: i64 = 300;
pub(crate) const ACTION_COMPILE: &str = "control_compile";
pub(crate) const ACTION_ADJUDICATE: &str = "control_adjudicate";
pub(crate) const ACTION_AUTHORIZE_MERGE: &str = "control_authorize_merge";
pub(crate) const ACTION_REVIEW_MERGE: &str = "review_merge";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LocalControlCapabilityV1 {
    version: u8,
    capability_id: String,
    principal_id: String,
    session_id: String,
    command_profile: ControlCommandProfileV1,
    local_project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    portable_repo_id: Option<String>,
    permitted_actions: Vec<String>,
    issued_at: String,
    expires_at: String,
    nonce: String,
    bearer_digest: String,
    seal: String,
}

#[derive(Debug, Clone, Serialize)]
struct AuthorityProofMessage<'a> {
    capability_id: &'a str,
    principal_id: &'a str,
    session_id: &'a str,
    command_profile: ControlCommandProfileV1,
    local_project_id: &'a str,
    portable_repo_id: &'a Option<String>,
    github_repository: &'a Option<String>,
    permitted_action: &'a str,
    expires_at: &'a str,
    manifest_event_id: &'a str,
    manifest_digest: &'a str,
    manifest_version: u64,
    adjudication: Option<&'a ControlAdjudicationRecordV1>,
}

#[derive(Debug, Clone, Copy)]
pub struct ControlAuthorityProvision<'a> {
    pub principal_id: &'a str,
    pub session_id: &'a str,
    pub command_profile: ControlCommandProfileV1,
    pub portable_repo_id: Option<&'a str>,
    pub expires_at: &'a str,
    pub issuer_token: &'a str,
    pub authority_token_out: &'a Path,
}

#[derive(Debug, Clone)]
pub struct ProvisionedControlAuthorityV1 {
    pub capability_id: String,
    pub principal_id: String,
    pub session_id: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ControlMergeCapabilityProvision<'a> {
    pub source: &'a str,
    pub principal_id: &'a str,
    pub session_id: &'a str,
    pub expires_at: &'a str,
    pub issuer_token: &'a str,
}

/// Verified host-local authority passed to the sole review merge
/// implementation. This value is never serialized into a ledger event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedControlMergeCapabilityV1 {
    pub capability_id: String,
    pub source: String,
    pub principal_id: String,
    pub control_id: String,
    pub portable_repo_id: Option<String>,
    pub pr_number: u64,
    pub expected_head_sha: String,
    pub expected_base_sha: String,
    pub manifest_digest: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalMergeCapabilityV1 {
    version: u8,
    capability_id: String,
    source: String,
    principal_id: String,
    session_id: String,
    local_project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    portable_repo_id: Option<String>,
    control_id: String,
    pr_number: u64,
    expected_head_sha: String,
    expected_base_sha: String,
    permitted_action: String,
    expires_at: String,
    manifest_digest: String,
    seal: String,
}

impl Ledger {
    /// Provision the one local strong-controller capability used by S6a.
    ///
    /// Issuance is a same-user administrative operation. It does not accept an
    /// authority boolean: the pre-provisioned issuer secret, actor/session
    /// registration, and explicit RBAC grants are checked before a private
    /// CSPRNG bearer and sealed capability record are written.
    pub fn provision_control_authority(
        &self,
        request: ControlAuthorityProvision<'_>,
    ) -> anyhow::Result<ProvisionedControlAuthorityV1> {
        let ControlAuthorityProvision {
            principal_id,
            session_id,
            command_profile,
            portable_repo_id,
            expires_at,
            issuer_token,
            authority_token_out,
        } = request;
        anyhow::ensure!(
            command_profile == ControlCommandProfileV1::Strong,
            "Flash command profile cannot receive compile/adjudicate authority"
        );
        validate_identity(principal_id, "principal_id")?;
        validate_identity(session_id, "session_id")?;
        if let Some(repo) = portable_repo_id {
            edda_core::continuity::validate_portable_repo_id(repo)?;
        }
        ensure_registered_live_session(&self.paths.root, principal_id, session_id)?;
        for action in [ACTION_COMPILE, ACTION_ADJUDICATE] {
            require_explicit_rbac(&self.paths.edda_dir, principal_id, action)?;
        }
        let expiry = parse_future_expiry(expires_at)?;
        anyhow::ensure!(
            expiry <= OffsetDateTime::now_utc() + time::Duration::days(7),
            "control authority expiry exceeds the seven-day local bound"
        );

        let _workspace_lock = WorkspaceLock::acquire(&self.paths)?;
        let _authority_lock = edda_store::lock_file(&authority_lock_path(&self.paths.edda_dir))?;
        let key = load_root_key(&self.paths.edda_dir)?;
        let presented = decode_secret(issuer_token, "control issuer token")?;
        anyhow::ensure!(
            constant_time_eq(&key, &presented),
            "CONTROL_UNAVAILABLE: control issuer token is invalid"
        );
        let mut random = [0_u8; KEY_BYTES];
        rand::rngs::OsRng.fill_bytes(&mut random);
        let capability_id = format!("cap_{}", hex::encode(random));
        rand::rngs::OsRng.fill_bytes(&mut random);
        let authority_token = hex::encode(random);
        rand::rngs::OsRng.fill_bytes(&mut random);
        let nonce = hex::encode(random);
        let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let mut capability = LocalControlCapabilityV1 {
            version: AUTHORITY_RECORD_VERSION,
            capability_id: capability_id.clone(),
            principal_id: principal_id.to_string(),
            session_id: session_id.to_string(),
            command_profile,
            local_project_id: edda_store::project_id(&self.paths.root),
            portable_repo_id: portable_repo_id.map(str::to_string),
            permitted_actions: vec![ACTION_COMPILE.into(), ACTION_ADJUDICATE.into()],
            issued_at: now,
            expires_at: expires_at.to_string(),
            nonce,
            bearer_digest: edda_core::hash::sha256_hex(authority_token.as_bytes()),
            seal: String::new(),
        };
        capability.seal = seal_serialized(&key, &capability_without_seal(&capability)?)?;
        write_private_new(authority_token_out, authority_token.as_bytes())?;
        if let Err(error) = write_private_atomic(
            &authority_record_path(&self.paths.edda_dir),
            &serde_json::to_vec_pretty(&capability)?,
        ) {
            let _ = std::fs::remove_file(authority_token_out);
            return Err(error);
        }
        Ok(ProvisionedControlAuthorityV1 {
            capability_id,
            principal_id: principal_id.to_string(),
            session_id: session_id.to_string(),
            expires_at: expires_at.to_string(),
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn provision_local_merge_capability(
    ledger: &Ledger,
    control_id: &str,
    manifest_digest: &str,
    portable_repo_id: Option<&str>,
    pr_number: u64,
    expected_head_sha: &str,
    expected_base_sha: &str,
    request: ControlMergeCapabilityProvision<'_>,
) -> anyhow::Result<VerifiedControlMergeCapabilityV1> {
    anyhow::ensure!(
        request.source == "operator_grant",
        "CONTROL_UNAVAILABLE: repository standing-rule authority has no product-readable digest"
    );
    validate_identity(request.principal_id, "principal_id")?;
    validate_identity(request.session_id, "session_id")?;
    ensure_registered_live_session(&ledger.paths.root, request.principal_id, request.session_id)?;
    require_explicit_rbac(
        &ledger.paths.edda_dir,
        request.principal_id,
        ACTION_AUTHORIZE_MERGE,
    )?;
    let expiry = parse_future_expiry(request.expires_at)?;
    anyhow::ensure!(
        expiry <= OffsetDateTime::now_utc() + time::Duration::hours(24),
        "merge capability expiry exceeds the one-day local bound"
    );
    let key = load_root_key(&ledger.paths.edda_dir)?;
    let presented = decode_secret(request.issuer_token, "control issuer token")?;
    anyhow::ensure!(
        constant_time_eq(&key, &presented),
        "CONTROL_UNAVAILABLE: control issuer token is invalid"
    );
    let _workspace_lock = WorkspaceLock::acquire(&ledger.paths)?;
    let _authority_lock = edda_store::lock_file(&authority_lock_path(&ledger.paths.edda_dir))?;
    let path = merge_capability_path(&ledger.paths.edda_dir, control_id);
    let replace_existing = if path.exists() {
        let existing = load_merge_capability(ledger, control_id)?;
        let exact = existing.source == request.source
            && existing.principal_id == request.principal_id
            && existing.session_id == request.session_id
            && existing.local_project_id == edda_store::project_id(&ledger.paths.root)
            && existing.portable_repo_id.as_deref() == portable_repo_id
            && existing.pr_number == pr_number
            && existing.expected_head_sha == expected_head_sha
            && existing.expected_base_sha == expected_base_sha
            && existing.manifest_digest == manifest_digest
            && existing.expires_at == request.expires_at;
        if exact {
            return verified_merge_capability(existing);
        }
        true
    } else {
        false
    };
    let identity = format!(
        "{control_id}:{manifest_digest}:{pr_number}:{expected_head_sha}:{expected_base_sha}:{}:{}",
        request.source, request.principal_id
    );
    let capability_id = format!(
        "mergecap_{}",
        edda_core::hash::sha256_hex(identity.as_bytes())
    );
    let mut capability = LocalMergeCapabilityV1 {
        version: MERGE_CAPABILITY_VERSION,
        capability_id,
        source: request.source.to_string(),
        principal_id: request.principal_id.to_string(),
        session_id: request.session_id.to_string(),
        local_project_id: edda_store::project_id(&ledger.paths.root),
        portable_repo_id: portable_repo_id.map(str::to_string),
        control_id: control_id.to_string(),
        pr_number,
        expected_head_sha: expected_head_sha.to_string(),
        expected_base_sha: expected_base_sha.to_string(),
        permitted_action: ACTION_REVIEW_MERGE.into(),
        expires_at: request.expires_at.to_string(),
        manifest_digest: manifest_digest.to_string(),
        seal: String::new(),
    };
    capability.seal = seal_serialized(&key, &merge_capability_without_seal(&capability)?)?;
    let bytes = serde_json::to_vec_pretty(&capability)?;
    if replace_existing {
        write_private_atomic(&path, &bytes)?;
    } else {
        write_private_new(&path, &bytes)?;
    }
    verified_merge_capability(capability)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn verify_local_merge_capability(
    ledger: &Ledger,
    control_id: &str,
    manifest_digest: &str,
    portable_repo_id: Option<&str>,
    pr_number: u64,
    expected_head_sha: &str,
    expected_base_sha: &str,
) -> anyhow::Result<VerifiedControlMergeCapabilityV1> {
    let capability = load_merge_capability(ledger, control_id)?;
    parse_future_expiry(&capability.expires_at)?;
    anyhow::ensure!(
        capability.local_project_id == edda_store::project_id(&ledger.paths.root)
            && capability.portable_repo_id.as_deref() == portable_repo_id
            && capability.control_id == control_id
            && capability.pr_number == pr_number
            && capability.expected_head_sha == expected_head_sha
            && capability.expected_base_sha == expected_base_sha
            && capability.manifest_digest == manifest_digest
            && capability.permitted_action == ACTION_REVIEW_MERGE,
        "local merge capability binding does not match this control action"
    );
    ensure_registered_live_session(
        &ledger.paths.root,
        &capability.principal_id,
        &capability.session_id,
    )?;
    require_explicit_rbac(
        &ledger.paths.edda_dir,
        &capability.principal_id,
        ACTION_AUTHORIZE_MERGE,
    )?;
    verified_merge_capability(capability)
}

fn load_merge_capability(
    ledger: &Ledger,
    control_id: &str,
) -> anyhow::Result<LocalMergeCapabilityV1> {
    let bytes = read_owner_only_file(
        &merge_capability_path(&ledger.paths.edda_dir, control_id),
        "local merge capability",
        16 * 1024,
    )?;
    let capability: LocalMergeCapabilityV1 = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("local merge capability is malformed"))?;
    anyhow::ensure!(
        capability.version == MERGE_CAPABILITY_VERSION,
        "unsupported local merge capability version"
    );
    anyhow::ensure!(
        capability.source == "operator_grant",
        "local merge capability has an unsupported authority source"
    );
    verify_local(
        ledger,
        &merge_capability_without_seal(&capability)?,
        &capability.seal,
    )?;
    Ok(capability)
}

fn merge_capability_without_seal(
    capability: &LocalMergeCapabilityV1,
) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(capability)?;
    value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("merge capability record is not an object"))?
        .remove("seal");
    Ok(value)
}

fn verified_merge_capability(
    capability: LocalMergeCapabilityV1,
) -> anyhow::Result<VerifiedControlMergeCapabilityV1> {
    Ok(VerifiedControlMergeCapabilityV1 {
        capability_id: capability.capability_id,
        source: capability.source,
        principal_id: capability.principal_id,
        control_id: capability.control_id,
        portable_repo_id: capability.portable_repo_id,
        pr_number: capability.pr_number,
        expected_head_sha: capability.expected_head_sha,
        expected_base_sha: capability.expected_base_sha,
        manifest_digest: capability.manifest_digest,
        expires_at: capability.expires_at,
    })
}

fn merge_capability_path(edda_dir: &Path, control_id: &str) -> PathBuf {
    edda_dir
        .join("control-local")
        .join(format!("merge-{control_id}.json"))
}

pub(crate) fn authorize_command(
    ledger: &Ledger,
    caller_session: &str,
    action: &str,
    portable_repo_id: Option<&str>,
    github_repository: Option<&str>,
    authority_token: &str,
) -> anyhow::Result<ControlAuthorityProofV1> {
    let (key, capability) = load_verified_capability(ledger)?;
    parse_future_expiry(&capability.expires_at)?;
    let bearer = decode_secret(authority_token, "control authority token")?;
    anyhow::ensure!(
        constant_time_eq(
            edda_core::hash::sha256_hex(hex::encode(bearer).as_bytes()).as_bytes(),
            capability.bearer_digest.as_bytes(),
        ),
        "current caller does not possess the local control capability"
    );
    anyhow::ensure!(
        capability.command_profile == ControlCommandProfileV1::Strong,
        "Flash command profile cannot compile or adjudicate control policy"
    );
    // EDDA_SESSION_ID/CLI text is only correlation. The sealed capability is
    // the authority; a matching string without it cannot pass this function.
    anyhow::ensure!(
        caller_session == capability.session_id,
        "current session is not bound to the local control capability"
    );
    ensure_registered_live_session(&ledger.paths.root, &capability.principal_id, caller_session)?;
    require_explicit_rbac(&ledger.paths.edda_dir, &capability.principal_id, action)?;
    anyhow::ensure!(
        capability
            .permitted_actions
            .iter()
            .any(|allowed| allowed == action),
        "local control capability does not permit this action"
    );
    anyhow::ensure!(
        capability.portable_repo_id.as_deref() == portable_repo_id,
        "control authority repository binding does not match the manifest"
    );
    let _ = key;
    Ok(ControlAuthorityProofV1 {
        capability_id: capability.capability_id,
        principal_id: capability.principal_id,
        session_id: capability.session_id,
        command_profile: capability.command_profile,
        local_project_id: capability.local_project_id,
        portable_repo_id: capability.portable_repo_id,
        github_repository: github_repository.map(str::to_string),
        permitted_action: action.to_string(),
        expires_at: capability.expires_at,
        seal: String::new(),
    })
}

pub(crate) fn seal_manifest_proof(
    ledger: &Ledger,
    mut proof: ControlAuthorityProofV1,
    manifest_event_id: &str,
    manifest_digest: &str,
    manifest_version: u64,
    adjudication: Option<&ControlAdjudicationRecordV1>,
) -> anyhow::Result<ControlAuthorityProofV1> {
    let key = load_root_key(&ledger.paths.edda_dir)?;
    proof.seal = seal_serialized(
        &key,
        &AuthorityProofMessage {
            capability_id: &proof.capability_id,
            principal_id: &proof.principal_id,
            session_id: &proof.session_id,
            command_profile: proof.command_profile,
            local_project_id: &proof.local_project_id,
            portable_repo_id: &proof.portable_repo_id,
            github_repository: &proof.github_repository,
            permitted_action: &proof.permitted_action,
            expires_at: &proof.expires_at,
            manifest_event_id,
            manifest_digest,
            manifest_version,
            adjudication,
        },
    )?;
    Ok(proof)
}

pub(crate) fn verify_manifest_proof(
    ledger: &Ledger,
    proof: &ControlAuthorityProofV1,
    manifest_event_id: &str,
    manifest_digest: &str,
    manifest_version: u64,
    adjudication: Option<&ControlAdjudicationRecordV1>,
) -> anyhow::Result<()> {
    let key = load_root_key(&ledger.paths.edda_dir)?;
    anyhow::ensure!(
        proof.local_project_id == edda_store::project_id(&ledger.paths.root),
        "control authority belongs to a foreign local project"
    );
    let expected = seal_serialized(
        &key,
        &AuthorityProofMessage {
            capability_id: &proof.capability_id,
            principal_id: &proof.principal_id,
            session_id: &proof.session_id,
            command_profile: proof.command_profile,
            local_project_id: &proof.local_project_id,
            portable_repo_id: &proof.portable_repo_id,
            github_repository: &proof.github_repository,
            permitted_action: &proof.permitted_action,
            expires_at: &proof.expires_at,
            manifest_event_id,
            manifest_digest,
            manifest_version,
            adjudication,
        },
    )?;
    anyhow::ensure!(
        constant_time_eq(expected.as_bytes(), proof.seal.as_bytes()),
        "control manifest authority seal is invalid"
    );
    Ok(())
}

pub(crate) fn authority_is_active(
    ledger: &Ledger,
    proof: &ControlAuthorityProofV1,
) -> anyhow::Result<()> {
    parse_future_expiry(&proof.expires_at)?;
    let (_, capability) = load_verified_capability(ledger)?;
    anyhow::ensure!(
        proof.capability_id == capability.capability_id
            && proof.principal_id == capability.principal_id
            && proof.session_id == capability.session_id
            && proof.command_profile == capability.command_profile
            && proof.local_project_id == capability.local_project_id
            && proof.portable_repo_id == capability.portable_repo_id
            && proof.expires_at == capability.expires_at
            && capability
                .permitted_actions
                .iter()
                .any(|allowed| allowed == &proof.permitted_action),
        "control manifest authority binding does not match the active local capability"
    );
    require_explicit_rbac(
        &ledger.paths.edda_dir,
        &proof.principal_id,
        &proof.permitted_action,
    )
}

pub(crate) fn seal_local<T: Serialize>(ledger: &Ledger, value: &T) -> anyhow::Result<String> {
    seal_serialized(&load_root_key(&ledger.paths.edda_dir)?, value)
}

pub(crate) fn verify_local<T: Serialize>(
    ledger: &Ledger,
    value: &T,
    seal: &str,
) -> anyhow::Result<()> {
    let expected = seal_local(ledger, value)?;
    anyhow::ensure!(
        constant_time_eq(expected.as_bytes(), seal.as_bytes()),
        "local control record seal is invalid"
    );
    Ok(())
}

fn load_verified_capability(
    ledger: &Ledger,
) -> anyhow::Result<(Vec<u8>, LocalControlCapabilityV1)> {
    let key = load_root_key(&ledger.paths.edda_dir)?;
    let path = authority_record_path(&ledger.paths.edda_dir);
    let bytes = std::fs::read(&path)
        .map_err(|_| anyhow::anyhow!("CONTROL_UNAVAILABLE: local control authority is absent"))?;
    anyhow::ensure!(
        bytes.len() <= 16 * 1024,
        "local control authority record exceeds its bound"
    );
    let capability: LocalControlCapabilityV1 = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("local control authority record is malformed"))?;
    anyhow::ensure!(
        capability.version == AUTHORITY_RECORD_VERSION,
        "unsupported local control authority version"
    );
    let expected = seal_serialized(&key, &capability_without_seal(&capability)?)?;
    anyhow::ensure!(
        constant_time_eq(expected.as_bytes(), capability.seal.as_bytes()),
        "local control authority record seal is invalid"
    );
    anyhow::ensure!(
        capability.local_project_id == edda_store::project_id(&ledger.paths.root),
        "local control authority belongs to a foreign project"
    );
    Ok((key, capability))
}

fn capability_without_seal(
    capability: &LocalControlCapabilityV1,
) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(capability)?;
    value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("authority record is not an object"))?
        .remove("seal");
    Ok(value)
}

fn require_explicit_rbac(edda_dir: &Path, principal: &str, action: &str) -> anyhow::Result<()> {
    let actors = load_actors_from_dir(edda_dir)?;
    anyhow::ensure!(
        actors.actors.contains_key(principal),
        "control principal is not a registered actor"
    );
    let policy = load_policy_from_dir(edda_dir)?;
    anyhow::ensure!(
        policy.permissions.is_some(),
        "control authority requires an explicit RBAC permissions policy"
    );
    let result = evaluate_authz(
        &AuthzRequest {
            actor: principal.to_string(),
            action: action.to_string(),
            resource: None,
        },
        &policy,
        &actors,
    );
    anyhow::ensure!(
        result.allowed && result.matched_grant.is_some(),
        "RBAC denied explicit {action} authority for the registered principal"
    );
    Ok(())
}

fn ensure_registered_live_session(
    repo_root: &Path,
    principal_id: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let heartbeat = edda_store::read_heartbeat(&project_id, session_id)
        .ok_or_else(|| anyhow::anyhow!("control session is not registered"))?;
    anyhow::ensure!(
        heartbeat.session_id == session_id && heartbeat.label == principal_id,
        "registered control session identity mismatch"
    );
    let last = OffsetDateTime::parse(&heartbeat.last_heartbeat, &Rfc3339)
        .map_err(|_| anyhow::anyhow!("registered control session has an invalid heartbeat"))?;
    let age = (OffsetDateTime::now_utc() - last).whole_seconds();
    anyhow::ensure!(
        (-30..=SESSION_FRESH_SECONDS).contains(&age),
        "registered control session is stale"
    );
    Ok(())
}

fn parse_future_expiry(value: &str) -> anyhow::Result<OffsetDateTime> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| anyhow::anyhow!("control authority expiry must be RFC3339"))?;
    anyhow::ensure!(
        parsed > OffsetDateTime::now_utc(),
        "control authority is expired"
    );
    Ok(parsed)
}

fn authority_dir(edda_dir: &Path) -> PathBuf {
    edda_dir.join("control-local")
}

fn root_key_path(edda_dir: &Path) -> PathBuf {
    authority_dir(edda_dir).join("root.key")
}

fn authority_record_path(edda_dir: &Path) -> PathBuf {
    authority_dir(edda_dir).join("authority.json")
}

fn authority_lock_path(edda_dir: &Path) -> PathBuf {
    authority_dir(edda_dir).join("authority.lock")
}

/// Read owner-only local control material without exposing bytes before the
/// platform permission boundary is verified.
pub fn read_owner_only_file(path: &Path, label: &str, max_bytes: usize) -> anyhow::Result<Vec<u8>> {
    read_owner_only_file_platform(path, label, max_bytes)
}

#[cfg(unix)]
fn read_owner_only_file_platform(
    path: &Path,
    label: &str,
    max_bytes: usize,
) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::File::open(path)
        .map_err(|_| anyhow::anyhow!("CONTROL_UNAVAILABLE: {label} file is unavailable"))?;
    verify_owner_only_metadata(&file.metadata()?, label)?;
    let mut bytes = Vec::with_capacity(max_bytes.min(8 * 1024) + 1);
    file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= max_bytes, "{label} file exceeds its bound");
    Ok(bytes)
}

#[cfg(unix)]
fn verify_owner_only_metadata(metadata: &std::fs::Metadata, label: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    anyhow::ensure!(
        metadata.is_file(),
        "CONTROL_UNAVAILABLE: {label} path is not a file"
    );
    anyhow::ensure!(
        metadata.uid() == rustix::process::geteuid().as_raw(),
        "CONTROL_UNAVAILABLE: {label} file is not owned by the effective user"
    );
    anyhow::ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "CONTROL_UNAVAILABLE: {label} file permissions are not owner-only"
    );
    Ok(())
}

#[cfg(windows)]
fn read_owner_only_file_platform(
    _path: &Path,
    _label: &str,
    _max_bytes: usize,
) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!(
        "CONTROL_UNAVAILABLE: owner-only control authority storage is unsupported on Windows"
    )
}

#[cfg(not(any(unix, windows)))]
fn read_owner_only_file_platform(
    _path: &Path,
    _label: &str,
    _max_bytes: usize,
) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("CONTROL_UNAVAILABLE: owner-only control authority storage is unsupported")
}

fn load_root_key(edda_dir: &Path) -> anyhow::Result<Vec<u8>> {
    let bytes = read_owner_only_file(&root_key_path(edda_dir), "local control root key", 128)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("local control root key is malformed"))?;
    decode_secret(text, "local control root key")
}

fn decode_secret(value: &str, label: &str) -> anyhow::Result<Vec<u8>> {
    let key = hex::decode(value.trim()).map_err(|_| anyhow::anyhow!("{label} is malformed"))?;
    anyhow::ensure!(key.len() == KEY_BYTES, "{label} has the wrong length");
    Ok(key)
}

fn write_private_new(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let file = options.open(path).map_err(|error| {
            anyhow::anyhow!(
                "refusing to create private authority token output {}: {error}",
                path.display()
            )
        })?;
        return write_secret_and_cleanup(file, path, bytes);
    }
    #[cfg(windows)]
    {
        write_private_new_windows(path, bytes)
    }
    #[cfg(not(any(unix, windows)))]
    anyhow::bail!("private control material is unsupported on this platform")
}

#[cfg(unix)]
fn write_secret_and_cleanup(
    mut file: std::fs::File,
    path: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    #[cfg(unix)]
    verify_owner_only_metadata(&file.metadata()?, "control authority token output")?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

#[cfg(windows)]
fn write_private_new_windows(_path: &Path, _bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::bail!(
        "CONTROL_UNAVAILABLE: owner-only control authority storage is unsupported on Windows"
    )
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    edda_store::write_atomic(path, bytes)?;
    harden_private_permissions(path)
}

#[cfg(unix)]
fn harden_private_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(windows)]
fn harden_private_permissions(_path: &Path) -> anyhow::Result<()> {
    anyhow::bail!(
        "CONTROL_UNAVAILABLE: owner-only control authority storage is unsupported on Windows"
    )
}

#[cfg(not(any(unix, windows)))]
fn harden_private_permissions(_path: &Path) -> anyhow::Result<()> {
    anyhow::bail!("private control material is unsupported on this platform")
}

fn validate_identity(value: &str, field: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 160
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
        "{field} has an invalid shape"
    );
    Ok(())
}

fn seal_serialized<T: Serialize>(key: &[u8], value: &T) -> anyhow::Result<String> {
    let bytes = edda_core::canon::canonical_json_bytes(&serde_json::to_value(value)?)?;
    Ok(hex::encode(hmac_sha256(key, &bytes)))
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut normalized = [0_u8; BLOCK];
    if key.len() > BLOCK {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK];
    let mut outer_pad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_hash);
    outer.finalize().into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_token_creation_never_clobbers_an_existing_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bearer");
        write_private_new(&path, b"first").unwrap();
        let error = write_private_new(&path, b"second").unwrap_err().to_string();
        assert!(error.contains("refusing to create"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
    }

    #[cfg(unix)]
    #[test]
    fn forged_local_merge_capability_fails_its_product_owned_seal() {
        let root = tempfile::tempdir().unwrap();
        Ledger::ensure_initialized(root.path()).unwrap();
        let ledger = Ledger::open(root.path()).unwrap();
        let key = vec![0x42; KEY_BYTES];
        write_private_new(
            &root_key_path(&ledger.paths.edda_dir),
            hex::encode(&key).as_bytes(),
        )
        .unwrap();
        let mut capability = LocalMergeCapabilityV1 {
            version: MERGE_CAPABILITY_VERSION,
            capability_id: format!("mergecap_{}", "1".repeat(64)),
            source: "operator_grant".into(),
            principal_id: "planner".into(),
            session_id: "session-one".into(),
            local_project_id: edda_store::project_id(root.path()),
            portable_repo_id: Some(format!("repo_{}", "2".repeat(64))),
            control_id: "control_mergeone".into(),
            pr_number: 1141,
            expected_head_sha: "a".repeat(40),
            expected_base_sha: "b".repeat(40),
            permitted_action: ACTION_REVIEW_MERGE.into(),
            expires_at: "2999-01-01T00:00:00Z".into(),
            manifest_digest: "c".repeat(64),
            seal: String::new(),
        };
        capability.seal =
            seal_serialized(&key, &merge_capability_without_seal(&capability).unwrap()).unwrap();
        let path = merge_capability_path(&ledger.paths.edda_dir, &capability.control_id);
        write_private_new(&path, &serde_json::to_vec(&capability).unwrap()).unwrap();
        assert_eq!(
            load_merge_capability(&ledger, &capability.control_id)
                .unwrap()
                .pr_number,
            1141
        );

        let mut standing_rule = capability.clone();
        standing_rule.source = "repository_standing_rule".into();
        standing_rule.seal = seal_serialized(
            &key,
            &merge_capability_without_seal(&standing_rule).unwrap(),
        )
        .unwrap();
        std::fs::write(&path, serde_json::to_vec(&standing_rule).unwrap()).unwrap();
        let error = load_merge_capability(&ledger, &standing_rule.control_id)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unsupported authority source"), "{error}");

        capability.pr_number = 1142;
        std::fs::write(&path, serde_json::to_vec(&capability).unwrap()).unwrap();
        let error = load_merge_capability(&ledger, &capability.control_id)
            .unwrap_err()
            .to_string();
        assert!(error.contains("seal"), "{error}");
    }

    #[cfg(windows)]
    #[test]
    fn private_authority_storage_fails_closed_on_windows() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bearer");
        let error = write_private_new(&path, b"secret").unwrap_err().to_string();
        assert!(error.contains("CONTROL_UNAVAILABLE"), "{error}");
        assert!(!path.exists());

        std::fs::write(&path, b"secret").unwrap();
        let error = read_owner_only_file(&path, "control authority token", 128)
            .unwrap_err()
            .to_string();
        assert!(error.contains("CONTROL_UNAVAILABLE"), "{error}");
    }

    #[test]
    fn hmac_matches_rfc_4231_vector() {
        let key = [0x0b_u8; 20];
        assert_eq!(
            hex::encode(hmac_sha256(&key, b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
}
