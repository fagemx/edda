use crate::control_authority::{seal_local, verify_local};
use crate::control_projection::{action_id, project_control, ProjectedControl};
use crate::{Ledger, WorkspaceLock};
use edda_core::guided_execution::{
    validate_control_review_claim, ControlActionKindV1, ControlReviewClaimV1, ControlStateV1,
    ControlTargetV1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlReviewClaimOutcomeV1 {
    Won { adopted: bool },
    RefusedLive,
}

fn claim_without_seal(claim: &ControlReviewClaimV1) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(claim)?;
    value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("review claim is not an object"))?
        .remove("seal");
    Ok(value)
}

fn expected_base(
    projected: &ProjectedControl,
    pr_number: u64,
    head_sha: &str,
) -> anyhow::Result<String> {
    let mut candidates = projected
        .receipts
        .values()
        .filter_map(|(_, receipt)| {
            let ControlTargetV1::Delivery {
                pr_number: Some(number),
                result_head_sha,
                observed_base_tip_sha: Some(base),
                ..
            } = &receipt.target
            else {
                return None;
            };
            (receipt.action_kind == ControlActionKindV1::BindDelivery
                && receipt.next_state != ControlStateV1::NeedsDecision
                && *number == pr_number
                && result_head_sha == head_sha)
                .then(|| base.clone())
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    anyhow::ensure!(
        candidates.len() <= 1,
        "review claim has multiple receipt-bound PR bases"
    );
    if let Some(receipt_base) = candidates.pop() {
        anyhow::ensure!(
            projected
                .record
                .manifest
                .merge_policy
                .expected_base_sha
                .as_deref()
                .is_none_or(|expected| expected == receipt_base),
            "review claim receipt base differs from manifest expectation"
        );
        return Ok(receipt_base);
    }
    projected
        .record
        .manifest
        .merge_policy
        .expected_base_sha
        .clone()
        .ok_or_else(|| anyhow::anyhow!("review claim has no receipt- or manifest-bound PR base"))
}

impl Ledger {
    pub fn control_review_expected_base(
        &self,
        control_id: &str,
        pr_number: u64,
        head_sha: &str,
    ) -> anyhow::Result<String> {
        let projected = project_control(self, control_id)?;
        anyhow::ensure!(
            projected.authority_valid,
            "CONTROL_UNAVAILABLE: invalid control authority"
        );
        expected_base(&projected, pr_number, head_sha)
    }

    /// Seal one canonical review claim with the active host-local S6 authority.
    /// Every policy, identity, generation, action, and PR binding is checked
    /// before the root key is used; key bytes never leave the ledger crate.
    pub fn seal_control_review_claim(
        &self,
        mut claim: ControlReviewClaimV1,
    ) -> anyhow::Result<ControlReviewClaimV1> {
        validate_control_review_claim(&claim)?;
        anyhow::ensure!(claim.seal.is_empty(), "review claim is already sealed");
        let projected = project_control(self, &claim.control_id)?;
        anyhow::ensure!(
            projected.authority_valid,
            "CONTROL_UNAVAILABLE: invalid control authority"
        );
        let manifest = &projected.record.manifest;
        let expected_base = expected_base(&projected, claim.pr_number, &claim.head_sha)?;
        let target = ControlTargetV1::PullRequest {
            number: claim.pr_number,
            head_sha: claim.head_sha.clone(),
        };
        anyhow::ensure!(
            claim.manifest_digest == projected.record.manifest_digest
                && manifest.basis.github_repository.as_deref() == Some(claim.repository.as_str())
                && claim.generation == manifest.manifest_version
                && claim.state_version == projected.status.state_version
                && claim.action_id
                    == action_id(&projected, ControlActionKindV1::ClaimVerification, &target)?
                && claim.claimant == manifest.admission_policy.claim_identity
                && claim.claimant_session == projected.record.authority.session_id
                && claim.verifier_identity == manifest.review_policy.verifier_identity
                && claim.verifier_profile == manifest.review_policy.verifier_profile
                && claim.frozen_surface_source == manifest.review_policy.frozen_surface_source
                && manifest
                    .merge_policy
                    .pr_number
                    .is_none_or(|number| number == claim.pr_number)
                && manifest
                    .merge_policy
                    .expected_head_sha
                    .as_deref()
                    .is_none_or(|head| head == claim.head_sha)
                && expected_base == claim.base_sha,
            "review claim does not match the sealed current control generation"
        );
        claim.seal = seal_local(self, &claim_without_seal(&claim)?)?;
        Ok(claim)
    }

    /// Verify cryptographic integrity only. Lifecycle checks remain separate
    /// so an old claim can be evaluated for adjudicated supersession.
    pub fn verify_control_review_claim(&self, claim: &ControlReviewClaimV1) -> anyhow::Result<()> {
        validate_control_review_claim(claim)?;
        anyhow::ensure!(!claim.seal.is_empty(), "review claim seal is absent");
        verify_local(self, &claim_without_seal(claim)?, &claim.seal)
    }

    pub fn control_review_claim_is_current(
        &self,
        claim: &ControlReviewClaimV1,
    ) -> anyhow::Result<()> {
        self.verify_control_review_claim(claim)?;
        let projected = project_control(self, &claim.control_id)?;
        anyhow::ensure!(
            projected.authority_valid
                && projected.record.manifest_digest == claim.manifest_digest
                && projected.record.manifest.manifest_version == claim.generation
                && projected.record.manifest.review_policy.verifier_identity
                    == claim.verifier_identity,
            "review claim is not from the current authorized control generation"
        );
        let target = ControlTargetV1::PullRequest {
            number: claim.pr_number,
            head_sha: claim.head_sha.clone(),
        };
        anyhow::ensure!(
            projected.intents.values().any(|(_, intent)| {
                intent.action_id == claim.action_id
                    && intent.observed_state_version == claim.state_version
                    && intent.action_kind == ControlActionKindV1::ClaimVerification
                    && intent.target == target
            }),
            "review claim has no matching durable control intent"
        );
        Ok(())
    }

    pub fn control_review_claim_for_subject(
        &self,
        repository: &str,
        pr_number: u64,
        head_sha: &str,
    ) -> anyhow::Result<Option<ControlReviewClaimV1>> {
        let repo_digest = edda_core::hash::sha256_hex(repository.as_bytes());
        let path = self
            .paths
            .edda_dir
            .join("control-local/review-claims")
            .join(repo_digest)
            .join(format!("pr-{pr_number}-{head_sha}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let claim: ControlReviewClaimV1 = serde_json::from_slice(&std::fs::read(path)?)
            .map_err(|_| anyhow::anyhow!("local review claim projection is malformed"))?;
        self.verify_control_review_claim(&claim)?;
        Ok(Some(claim))
    }

    pub fn claim_control_review(
        &self,
        request: &ControlReviewClaimV1,
    ) -> anyhow::Result<ControlReviewClaimOutcomeV1> {
        let _lock = WorkspaceLock::acquire(&self.paths)?;
        let repo_digest = edda_core::hash::sha256_hex(request.repository.as_bytes());
        let path = self
            .paths
            .edda_dir
            .join("control-local/review-claims")
            .join(repo_digest)
            .join(format!(
                "pr-{}-{}.json",
                request.pr_number, request.head_sha
            ));
        if path.exists() {
            let prior: ControlReviewClaimV1 = serde_json::from_slice(&std::fs::read(&path)?)
                .map_err(|_| anyhow::anyhow!("local review claim projection is malformed"))?;
            self.verify_control_review_claim(&prior)?;
            if prior == *request {
                self.control_review_claim_is_current(request)?;
                return Ok(ControlReviewClaimOutcomeV1::Won { adopted: true });
            }
            let prior_artifact = self
                .paths
                .edda_dir
                .join("control-local/review-bundles")
                .join(format!("{}.json", prior.charter_digest));
            let mut repaired = prior.clone();
            repaired.charter_digest = request.charter_digest.clone();
            repaired.review_bundle_ref = request.review_bundle_ref.clone();
            repaired.seal = request.seal.clone();
            let repairable_incomplete = !prior_artifact.exists()
                && repaired == *request
                && self.control_review_claim_is_current(request).is_ok();
            if !repairable_incomplete
                && !self.control_review_claim_is_supersedable(&prior, request)?
            {
                return Ok(ControlReviewClaimOutcomeV1::RefusedLive);
            }
        } else {
            self.control_review_claim_is_current(request)?;
        }
        edda_store::write_atomic(&path, &serde_json::to_vec_pretty(request)?)?;
        let readback: ControlReviewClaimV1 = serde_json::from_slice(&std::fs::read(&path)?)?;
        anyhow::ensure!(readback == *request, "local review claim readback changed");
        Ok(ControlReviewClaimOutcomeV1::Won { adopted: false })
    }

    /// A same-subject claim is replaceable only after a sealed adjudication of
    /// a terminal claim failure or a claimed review's inconclusive receipt.
    pub fn control_review_claim_is_supersedable(
        &self,
        prior: &ControlReviewClaimV1,
        next: &ControlReviewClaimV1,
    ) -> anyhow::Result<bool> {
        self.verify_control_review_claim(prior)?;
        self.control_review_claim_is_current(next)?;
        if prior.repository != next.repository
            || prior.pr_number != next.pr_number
            || prior.head_sha != next.head_sha
        {
            return Ok(false);
        }
        if prior.control_id != next.control_id || prior.base_sha != next.base_sha {
            let prior_projected = project_control(self, &prior.control_id)?;
            let prior_target = ControlTargetV1::PullRequest {
                number: prior.pr_number,
                head_sha: prior.head_sha.clone(),
            };
            return Ok(prior_projected.authority_valid
                && prior_projected.status.state == ControlStateV1::NeedsDecision
                && prior_projected.receipts.values().any(|(_, receipt)| {
                    receipt.target == prior_target
                        && receipt.next_state == ControlStateV1::NeedsDecision
                        && matches!(
                            receipt.action_kind,
                            ControlActionKindV1::ClaimVerification
                                | ControlActionKindV1::RequestVerification
                        )
                }));
        }
        if prior.generation.checked_add(1) != Some(next.generation) {
            return Ok(false);
        }
        let projected = project_control(self, &next.control_id)?;
        let Some(adjudication) = projected.record.adjudication.as_ref() else {
            return Ok(false);
        };
        if adjudication.prior_manifest_digest != prior.manifest_digest
            || adjudication.prior_state_version >= next.state_version
        {
            return Ok(false);
        }
        let prior_target = ControlTargetV1::PullRequest {
            number: prior.pr_number,
            head_sha: prior.head_sha.clone(),
        };
        let prior_claim_receipt = projected
            .receipts
            .values()
            .find(|(_, receipt)| {
                receipt.action_id == prior.action_id
                    && receipt.action_kind == ControlActionKindV1::ClaimVerification
                    && receipt.target == prior_target
                    && receipt.observed_state_version == prior.state_version
            })
            .map(|(_, receipt)| receipt);
        let claim_stage_terminal = prior_claim_receipt
            .is_some_and(|receipt| receipt.next_state == ControlStateV1::NeedsDecision);
        let claim_recorded = prior_claim_receipt
            .is_some_and(|receipt| receipt.next_state != ControlStateV1::NeedsDecision);
        let review_inconclusive = projected.receipts.values().any(|(_, receipt)| {
            receipt.action_kind == ControlActionKindV1::RequestVerification
                && receipt.target == prior_target
                && receipt.observed_state_version > prior.state_version
                && receipt.next_state == ControlStateV1::NeedsDecision
        });
        Ok(claim_stage_terminal || (claim_recorded && review_inconclusive))
    }
}
