use super::*;
use std::sync::{Arc, Barrier, Mutex};

#[derive(Default)]
struct FakeLocal {
    current: Mutex<Option<StructuredReviewRequest>>,
    terminal_generation: Mutex<Option<u64>>,
}

impl LocalClaimAuthority for FakeLocal {
    fn claim(&self, request: &StructuredReviewRequest) -> Result<LocalClaimOutcome> {
        let mut current = self.current.lock().expect("local claim");
        match current.as_ref() {
            None => {
                *current = Some(request.clone());
                Ok(LocalClaimOutcome::Won { adopted: false })
            }
            Some(prior) if prior == request => Ok(LocalClaimOutcome::Won { adopted: true }),
            Some(prior)
                if *self.terminal_generation.lock().expect("terminal")
                    == Some(prior.generation)
                    && prior.generation.checked_add(1) == Some(request.generation) =>
            {
                *current = Some(request.clone());
                Ok(LocalClaimOutcome::Won { adopted: false })
            }
            Some(_) => Ok(LocalClaimOutcome::RefusedLive),
        }
    }

    fn verify(&self, request: &StructuredReviewRequest) -> Result<()> {
        anyhow::ensure!(request.seal == seal_for(request), "forged seal");
        Ok(())
    }
}

#[derive(Default)]
struct FakeRemote(Mutex<Option<StructuredReviewRequest>>);

struct MalformedRemote;

impl RemoteClaimEvidence for MalformedRemote {
    fn create_if_absent(&self, _request: &StructuredReviewRequest) -> Result<bool> {
        Ok(false)
    }

    fn read(&self, _request: &StructuredReviewRequest) -> Result<Option<StructuredReviewRequest>> {
        anyhow::bail!("malformed immutable tag payload")
    }
}

impl RemoteClaimEvidence for FakeRemote {
    fn create_if_absent(&self, request: &StructuredReviewRequest) -> Result<bool> {
        let mut current = self.0.lock().expect("remote claim");
        if current.is_some() {
            return Ok(false);
        }
        *current = Some(request.clone());
        Ok(true)
    }

    fn read(&self, _request: &StructuredReviewRequest) -> Result<Option<StructuredReviewRequest>> {
        Ok(self.0.lock().expect("remote claim").clone())
    }
}

fn request(action: char, generation: u64) -> StructuredReviewRequest {
    let mut request = StructuredReviewRequest {
        claim_version: edda_core::guided_execution::CONTROL_REVIEW_CLAIM_VERSION,
        repository: "owner/repo".into(),
        control_id: "control_reviewclaim".into(),
        action_id: format!("action_{}", action.to_string().repeat(64)),
        manifest_digest: action.to_string().repeat(64),
        charter_digest: "e".repeat(64),
        review_bundle_ref: format!("blob:sha256:{}", "e".repeat(64)),
        generation,
        state_version: generation,
        pr_number: 1141,
        base_sha: "a".repeat(40),
        head_sha: "b".repeat(40),
        claimant: "machine/controller".into(),
        claimant_session: "session-one".into(),
        controller_identity: "controller".into(),
        verifier_identity: "independent-verifier".into(),
        verifier_profile: "pi:provider/model".into(),
        frozen_surface_source: "manifest".into(),
        seal: String::new(),
    };
    request.seal = seal_for(&request);
    request
}

fn seal_for(request: &StructuredReviewRequest) -> String {
    let mut value = serde_json::to_value(request).expect("request");
    value.as_object_mut().expect("object").remove("seal");
    edda_core::hash::sha256_hex(
        &edda_core::canon::canonical_json_bytes(&value).expect("canonical request"),
    )
}

#[test]
fn simultaneous_live_generations_have_one_local_winner() {
    let local = Arc::new(FakeLocal::default());
    let barrier = Arc::new(Barrier::new(2));
    let threads = ['a', 'b'].map(|action| {
        let local = Arc::clone(&local);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            local.claim(&request(action, 1)).expect("claim")
        })
    });
    let results = threads
        .into_iter()
        .map(|thread| thread.join().expect("contender"))
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, LocalClaimOutcome::Won { .. }))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, LocalClaimOutcome::RefusedLive))
            .count(),
        1
    );
}

#[test]
fn same_head_retry_requires_terminal_adjudicated_generation() {
    let local = FakeLocal::default();
    assert_eq!(
        local.claim(&request('a', 1)).unwrap(),
        LocalClaimOutcome::Won { adopted: false }
    );
    assert_eq!(
        local.claim(&request('b', 2)).unwrap(),
        LocalClaimOutcome::RefusedLive
    );
    *local.terminal_generation.lock().unwrap() = Some(1);
    assert_eq!(
        local.claim(&request('b', 2)).unwrap(),
        LocalClaimOutcome::Won { adopted: false }
    );
    assert_eq!(
        local.claim(&request('c', 3)).unwrap(),
        LocalClaimOutcome::RefusedLive
    );
}

#[test]
fn forged_preclaimed_remote_ref_is_evidence_refusal_not_authority() {
    let local = FakeLocal::default();
    let mut forged = request('a', 1);
    forged.claimant = "attacker/controller".into();
    let remote = FakeRemote(Mutex::new(Some(forged)));
    assert!(matches!(
        claim_with_stores(&local, &remote, request('a', 1)).unwrap(),
        ReviewClaimOutcome::Refused { reason_code }
            if reason_code == "REVIEW_CLAIM_EVIDENCE_INVALID"
    ));
    assert_eq!(
        local.current.lock().unwrap().as_ref(),
        Some(&request('a', 1)),
        "forged GitHub evidence cannot replace the local live authority"
    );

    *local.terminal_generation.lock().unwrap() = Some(1);
    assert!(matches!(
        claim_with_stores(&local, &FakeRemote::default(), request('b', 2)).unwrap(),
        ReviewClaimOutcome::Won { adopted: false, .. }
    ));
}

#[test]
fn malformed_remote_payload_becomes_an_adjudicable_refusal() {
    let local = FakeLocal::default();
    assert!(matches!(
        claim_with_stores(&local, &MalformedRemote, request('a', 1)).unwrap(),
        ReviewClaimOutcome::Refused { reason_code }
            if reason_code == "REVIEW_CLAIM_EVIDENCE_INVALID"
    ));
}

#[test]
fn altered_signed_payload_is_rejected_on_readback() {
    let local = FakeLocal::default();
    let mut altered = request('a', 1);
    altered.charter_digest = "f".repeat(64);
    let remote = FakeRemote(Mutex::new(Some(altered)));
    assert!(matches!(
        claim_with_stores(&local, &remote, request('a', 1)).unwrap(),
        ReviewClaimOutcome::Refused { reason_code }
            if reason_code == "REVIEW_CLAIM_EVIDENCE_INVALID"
    ));
}
