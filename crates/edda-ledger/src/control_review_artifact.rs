use crate::{control::ControlEffectResultV1, Ledger};
use edda_core::guided_execution::{ControlActionKindV1, ControlReceiptV1, ControlStateV1};

const AUDIT_BINDING: &str = ":audit-sha256:";

pub(super) fn validate_digest(digest: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "control receipt ephemeral artifact digest is malformed"
    );
    Ok(())
}

fn bind_external_identity(
    external_identity: Option<String>,
    digest: &str,
) -> anyhow::Result<String> {
    validate_digest(digest)?;
    let identity = external_identity.unwrap_or_else(|| "local-review-artifact".into());
    anyhow::ensure!(
        !identity.contains(AUDIT_BINDING),
        "control result already has an audit artifact binding"
    );
    Ok(format!("{identity}{AUDIT_BINDING}{digest}"))
}

pub(super) fn digest_from_external_identity(identity: Option<&str>) -> Option<&str> {
    let digest = identity?.rsplit_once(AUDIT_BINDING)?.1;
    validate_digest(digest).ok().map(|()| digest)
}

pub(super) fn receipt_external_identity(
    result: &ControlEffectResultV1,
) -> anyhow::Result<Option<String>> {
    match &result.ephemeral_artifact_digest {
        Some(digest) => Ok(Some(bind_external_identity(
            result.external_identity.clone(),
            digest,
        )?)),
        None => Ok(result.external_identity.clone()),
    }
}

pub(super) fn effect_cleanup_digest(
    action: ControlActionKindV1,
    result: &ControlEffectResultV1,
) -> Option<String> {
    (action == ControlActionKindV1::RequestVerification
        || result.next_state == ControlStateV1::NeedsDecision)
        .then(|| result.ephemeral_artifact_digest.clone())
        .flatten()
}

pub(super) fn validate_external_identity(identity: Option<&str>) -> anyhow::Result<()> {
    if identity.is_some_and(|value| value.contains(AUDIT_BINDING)) {
        anyhow::ensure!(
            digest_from_external_identity(identity).is_some(),
            "control receipt audit artifact binding is malformed"
        );
    }
    Ok(())
}

pub(super) fn cleanup(ledger: &Ledger, digest: &str) -> anyhow::Result<()> {
    validate_digest(digest)?;
    let path = ledger
        .paths
        .edda_dir
        .join("control-local/review-bundles")
        .join(format!("{digest}.json"));
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn should_cleanup(receipt: &ControlReceiptV1) -> bool {
    digest_from_external_identity(receipt.external_identity.as_deref()).is_some()
        && (receipt.action_kind == ControlActionKindV1::RequestVerification
            || receipt.next_state == ControlStateV1::NeedsDecision)
}

pub(super) fn cleanup_recorded(ledger: &Ledger, control_id: &str) -> anyhow::Result<()> {
    let projected = crate::control_projection::project_control(ledger, control_id)?;
    for receipt in projected
        .receipts
        .values()
        .map(|(_, receipt)| receipt)
        .filter(|receipt| should_cleanup(receipt))
    {
        if let Some(digest) = digest_from_external_identity(receipt.external_identity.as_deref()) {
            cleanup(ledger, digest)?;
        }
    }
    Ok(())
}
