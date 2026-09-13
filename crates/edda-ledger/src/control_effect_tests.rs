use super::*;
use crate::ControlReviewClaimOutcomeV1;
use crate::{ControlEffectRequestV1, ControlEffectResultV1};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The stable repository key every controlled effect test binds to.
fn bound_portable() -> String {
    format!("repo_{}", "d".repeat(64))
}

/// Bind the canonical GitHub repository and stable repository key the
/// production compiler requires for every non-local completion condition.
fn compile_input(control_id: &str) -> ControlCompileInputV1 {
    let mut input = super::compile_input(control_id);
    input.manifest.basis.portable_repo_id = Some(bound_portable());
    input.manifest.basis.github_repository = Some("owner/repo".into());
    input
}

// Real-OS-process contention harness.
//
// The production WorkspaceLock is a non-blocking admission gate, so two
// contemporaneous contenders cannot both hold it inside one process. These
// properties are cross-process, so the parent re-execs the test binary: each
// child receives its inputs through the environment and takes the contender
// branch at the top of the test.

const ROLE_ENV: &str = "EDDA_CONTROL_TEST_ROLE";
const ROOT_ENV: &str = "EDDA_CONTROL_TEST_ROOT";
const CONTROL_ID_ENV: &str = "EDDA_CONTROL_TEST_CONTROL_ID";
const TOKEN_ENV: &str = "EDDA_CONTROL_TEST_TOKEN";
const CLAIM_ENV: &str = "EDDA_CONTROL_TEST_CLAIM";
const MARKER_ENV: &str = "EDDA_CONTROL_TEST_MARKER";
const OUT_ENV: &str = "EDDA_CONTROL_TEST_OUT";
const GATE_ENV: &str = "EDDA_CONTROL_TEST_GATE";

/// True only inside a spawned contender; the parent never sets this in its own
/// environment.
fn contender_role() -> Option<String> {
    std::env::var(ROLE_ENV).ok()
}

fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("contender environment {key} is missing"))
}

/// A spawned contender reaped on every path, including a panic or early
/// return between spawn and wait.
struct Contender(Child);

impl Drop for Contender {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_contender(test_name: &str, env: &[(&str, String)], gate: &Path) -> Contender {
    let mut command = Command::new(std::env::current_exe().expect("current test executable"));
    command
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(ROLE_ENV, "contender")
        .env(GATE_ENV, gate)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    for (key, value) in env {
        command.env(*key, value);
    }
    Contender(command.spawn().expect("spawn contender process"))
}

/// Hold every contender until the parent has spawned all of them, so they
/// contend on the production lock instead of running one after another.
fn await_start_gate(gate: &Path) {
    let ready = PathBuf::from(require_env(OUT_ENV)).with_extension("ready");
    std::fs::write(ready, b"ready").expect("publish contender readiness");
    let start = Instant::now();
    while !gate.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "start gate at {} never opened",
            gate.display()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn release_contenders(gate: &Path, outputs: [&Path; 2]) {
    let start = Instant::now();
    while !outputs
        .iter()
        .all(|path| path.with_extension("ready").exists())
    {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "contenders did not become ready"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    std::fs::write(gate, b"go").expect("release both ready contenders");
}

/// Bound the parent wait and reap a hung contender so a failure cannot hang the
/// suite.
fn wait_bounded(child: &mut Contender, timeout: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        match child.0.try_wait().expect("poll contender") {
            Some(status) => return Some(status),
            None if start.elapsed() > timeout => {
                let _ = child.0.kill();
                let _ = child.0.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn read_outcome(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("missing contender outcome {}: {error}", path.display()))
        .trim()
        .to_string()
}

fn write_outcome(path: &Path, value: &str) {
    std::fs::write(path, value).expect("write contender outcome");
}

/// One O_APPEND line per executed adapter, visible across processes.
fn append_marker(path: &Path, value: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open effect marker");
    writeln!(file, "{value}").expect("append effect marker");
    file.sync_all().expect("sync effect marker");
}

/// Retry only the non-blocking workspace admission. A real caller can be
/// refused while the winner commits the durable intent; the per-action effect
/// lock is still what serializes the adapter, and this never bypasses it.
fn retry_workspace_lock<T>(mut attempt: impl FnMut() -> anyhow::Result<T>) -> anyhow::Result<T> {
    let start = Instant::now();
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error)
                if error.to_string().contains("workspace is locked")
                    && start.elapsed() < Duration::from_secs(30) =>
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error),
        }
    }
}

fn concurrent_presenter_contender() {
    let root = PathBuf::from(require_env(ROOT_ENV));
    let control_id = require_env(CONTROL_ID_ENV);
    let token = require_env(TOKEN_ENV);
    let marker = PathBuf::from(require_env(MARKER_ENV));
    let out = PathBuf::from(require_env(OUT_ENV));
    let gate = PathBuf::from(require_env(GATE_ENV));
    let ledger = Ledger::open(&root).expect("contender ledger");
    await_start_gate(&gate);
    let attempt = retry_workspace_lock(|| {
        ledger.control_apply_with(&control_id, &token, |request| {
            append_marker(&marker, &format!("effect:{}", std::process::id()));
            std::thread::sleep(Duration::from_millis(150));
            ControlEffectResultV1::applied(request.intent.action_kind, "single_external_effect")
        })
    });
    match attempt {
        Ok(applied) => write_outcome(&out, &format!("applied={}", applied.applied)),
        Err(error) => write_outcome(&out, &format!("error={error:#}")),
    }
}

fn review_claim_contender() {
    let root = PathBuf::from(require_env(ROOT_ENV));
    let claim_path = PathBuf::from(require_env(CLAIM_ENV));
    let out = PathBuf::from(require_env(OUT_ENV));
    let gate = PathBuf::from(require_env(GATE_ENV));
    let claim: ControlReviewClaimV1 =
        serde_json::from_slice(&std::fs::read(&claim_path).expect("read contender claim"))
            .expect("parse contender claim");
    let ledger = Ledger::open(&root).expect("contender ledger");
    await_start_gate(&gate);
    match retry_workspace_lock(|| ledger.claim_control_review(&claim)) {
        Ok(ControlReviewClaimOutcomeV1::Won { .. }) => write_outcome(&out, "won"),
        Ok(ControlReviewClaimOutcomeV1::RefusedLive) => write_outcome(&out, "refused_live"),
        Err(error) => write_outcome(&out, &format!("error={error:#}")),
    }
}

#[test]
fn product_effect_is_intent_first_and_recovery_adopts_the_same_action() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision_for(Some(&bound_portable()));
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_effectorder");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let claim = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(claim.action_kind, ControlActionKindV1::ClaimVerification);
    ledger
        .control_apply_with("control_effectorder", &claim.token.unwrap(), |request| {
            crate::ControlEffectResultV1::applied(
                request.intent.action_kind,
                "review_claim_recorded",
            )
        })
        .unwrap();
    let next = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(next.action_kind, ControlActionKindV1::RequestVerification);
    let token = next.token.unwrap();

    let error = ledger
        .control_apply_with("control_effectorder", &token, |request| {
            assert!(!request.recovering);
            assert_eq!(
                ledger
                    .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(
                ledger
                    .iter_events_by_type(CONTROL_RECEIPT_EVENT_TYPE)
                    .unwrap()
                    .len(),
                1
            );
            anyhow::bail!("simulated crash boundary")
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("simulated crash boundary"), "{error}");
    assert!(ledger
        .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
        .unwrap()
        .iter()
        .all(|event| !serde_json::to_string(event).unwrap().contains(&token)));
    assert_eq!(
        ledger
            .control_status("control_effectorder")
            .unwrap()
            .state_version,
        2
    );
    let before_recovery_next = ledger.count_events().unwrap();
    let recovered = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(ledger.count_events().unwrap(), before_recovery_next);
    assert_eq!(recovered.token.as_deref(), Some(token.as_str()));
    assert_eq!(recovered.reason_code, "PENDING_INTENT_RECOVERY_READY");

    let pending = ledger
        .control_apply_with(
            "control_effectorder",
            recovered.token.as_deref().unwrap(),
            |request| {
                assert!(request.recovering);
                Ok(crate::ControlEffectResultV1::durable_pending(
                    "verifier_running",
                    "dispatch-review-action".into(),
                    "dispatch_manifest:dispatch-review-action",
                ))
            },
        )
        .unwrap();
    assert!(!pending.applied);
    assert_eq!(pending.receipt_event_id, None);
    let recovered_after_pending = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(
        recovered_after_pending.token.as_deref(),
        Some(token.as_str())
    );

    let applied = ledger
        .control_apply_with(
            "control_effectorder",
            recovered_after_pending.token.as_deref().unwrap(),
            |request| {
                assert!(request.recovering);
                let mut result = crate::ControlEffectResultV1::applied(
                    request.intent.action_kind,
                    "structured_request_adopted",
                )?;
                result.dispatch_handle = Some("dispatch-review-action".into());
                Ok(result)
            },
        )
        .unwrap();
    assert!(applied.applied);
    assert_eq!(applied.state, ControlStateV1::Verifying);
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_RECEIPT_EVENT_TYPE)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        ledger
            .control_receipts("control_effectorder")
            .unwrap()
            .last()
            .unwrap()
            .dispatch_handle
            .as_deref(),
        Some("dispatch-review-action")
    );
    let waiting = ledger.control_next("control_effectorder").unwrap();
    assert_eq!(waiting.action_kind, ControlActionKindV1::WaitForWorkers);
    assert_eq!(
        waiting.availability,
        ControlAvailabilityV1::ControlUnavailable
    );
    assert!(waiting.token.is_none());
}

#[test]
fn concurrent_presenters_execute_one_product_effect_for_the_durable_intent() {
    if contender_role().is_some() {
        return concurrent_presenter_contender();
    }
    let fixture = Fixture::new();
    let authority_token = fixture.provision_for(Some(&bound_portable()));
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_oneeffect");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let token = ledger
        .control_next("control_oneeffect")
        .unwrap()
        .token
        .unwrap();
    let root = fixture.root.path().to_path_buf();
    let marker = root.join("contender-effect-marker");
    let gate = root.join("contender-start-gate");
    let out_one = root.join("contender-out-one");
    let out_two = root.join("contender-out-two");
    let mut one = spawn_contender(
        "control_tests::effect_tests::concurrent_presenters_execute_one_product_effect_for_the_durable_intent",
        &[
            (ROOT_ENV, root.display().to_string()),
            (CONTROL_ID_ENV, "control_oneeffect".to_string()),
            (TOKEN_ENV, token.clone()),
            (MARKER_ENV, marker.display().to_string()),
            (OUT_ENV, out_one.display().to_string()),
        ],
        &gate,
    );
    let mut two = spawn_contender(
        "control_tests::effect_tests::concurrent_presenters_execute_one_product_effect_for_the_durable_intent",
        &[
            (ROOT_ENV, root.display().to_string()),
            (CONTROL_ID_ENV, "control_oneeffect".to_string()),
            (TOKEN_ENV, token.clone()),
            (MARKER_ENV, marker.display().to_string()),
            (OUT_ENV, out_two.display().to_string()),
        ],
        &gate,
    );
    release_contenders(&gate, [&out_one, &out_two]);
    let status_one =
        wait_bounded(&mut one, Duration::from_secs(60)).expect("first contender finished");
    let status_two =
        wait_bounded(&mut two, Duration::from_secs(60)).expect("second contender finished");
    assert!(
        status_one.success(),
        "first contender failed: {status_one:?}"
    );
    assert!(
        status_two.success(),
        "second contender failed: {status_two:?}"
    );

    let outcomes = [read_outcome(&out_one), read_outcome(&out_two)];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| outcome.as_str() == "applied=true")
            .count(),
        1,
        "exactly one real presenter applies the product effect: {outcomes:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| outcome.as_str() == "applied=false")
            .count(),
        1,
        "the losing real presenter must adopt the recorded receipt: {outcomes:?}"
    );
    assert!(
        !outcomes.iter().any(|outcome| outcome.starts_with("error=")),
        "no contender may be refused: {outcomes:?}"
    );
    let marker_lines = std::fs::read_to_string(&marker).unwrap().lines().count();
    assert_eq!(
        marker_lines, 1,
        "exactly one cross-process effect: {outcomes:?}"
    );
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_RECEIPT_EVENT_TYPE)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn delegated_merge_is_unavailable_before_an_effect_intent_without_provider_base_cas() {
    let fixture = Fixture::new();
    let portable_repo_id = format!("repo_{}", "d".repeat(64));
    let authority_token = fixture.provision_for(Some(&portable_repo_id));
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_nobasecas");
    input.manifest.completion_condition = ControlCompletionConditionV1::DelegatedMergeSucceeded;
    input.manifest.merge_policy.required = true;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    input.manifest.basis.portable_repo_id = Some(portable_repo_id);
    ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    for expected in [
        ControlActionKindV1::ClaimVerification,
        ControlActionKindV1::RequestVerification,
    ] {
        let next = ledger.control_next("control_nobasecas").unwrap();
        assert_eq!(next.action_kind, expected);
        ledger
            .control_apply_with(
                "control_nobasecas",
                next.token.as_deref().unwrap(),
                |request| ControlEffectResultV1::applied(request.intent.action_kind, "fixture"),
            )
            .unwrap();
    }
    let before = ledger
        .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
        .unwrap()
        .len();
    let merge = ledger.control_next("control_nobasecas").unwrap();
    assert_eq!(merge.action_kind, ControlActionKindV1::MergeDelegated);
    assert_eq!(
        merge.availability,
        ControlAvailabilityV1::ControlUnavailable
    );
    assert_eq!(
        merge.reason_code,
        "DELEGATED_MERGE_ATOMIC_BASE_PRECONDITION_UNAVAILABLE"
    );
    assert!(merge.token.is_none());
    assert_eq!(
        ledger
            .iter_events_by_type(CONTROL_INTENT_EVENT_TYPE)
            .unwrap()
            .len(),
        before
    );
}

fn review_claim_for(request: &ControlEffectRequestV1) -> ControlReviewClaimV1 {
    let ControlTargetV1::PullRequest { number, head_sha } = &request.intent.target else {
        panic!("review claim fixture target")
    };
    ControlReviewClaimV1 {
        claim_version: CONTROL_REVIEW_CLAIM_VERSION,
        repository: "owner/repo".into(),
        control_id: request.manifest.control_id.clone(),
        action_id: request.intent.action_id.clone(),
        manifest_digest: request.manifest_digest.clone(),
        charter_digest: "d".repeat(64),
        review_bundle_ref: format!("blob:sha256:{}", "e".repeat(64)),
        generation: request.manifest.manifest_version,
        state_version: request.intent.observed_state_version,
        pr_number: *number,
        base_sha: request
            .manifest
            .merge_policy
            .expected_base_sha
            .clone()
            .expect("base"),
        head_sha: head_sha.clone(),
        claimant: request.manifest.admission_policy.claim_identity.clone(),
        claimant_session: request.authority.session_id.clone(),
        controller_identity: "controller".into(),
        verifier_identity: request.manifest.review_policy.verifier_identity.clone(),
        verifier_profile: request.manifest.review_policy.verifier_profile.clone(),
        frozen_surface_source: request.manifest.review_policy.frozen_surface_source.clone(),
        seal: String::new(),
    }
}

#[test]
fn production_workspace_lock_selects_one_live_signed_claim() {
    if contender_role().is_some() {
        return review_claim_contender();
    }
    let fixture = Fixture::new();
    let authority_token = fixture.provision_for(Some(&bound_portable()));
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_reviewclaimrace");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let first = ledger.control_next("control_reviewclaimrace").unwrap();
    let root = fixture.root.path().to_path_buf();
    ledger
        .control_apply_with(
            "control_reviewclaimrace",
            first.token.as_deref().unwrap(),
            |request| {
                let one = ledger.seal_control_review_claim(review_claim_for(request))?;
                let mut other = review_claim_for(request);
                other.charter_digest = "f".repeat(64);
                other.review_bundle_ref = format!("blob:sha256:{}", "f".repeat(64));
                let two = ledger.seal_control_review_claim(other)?;
                // A claim without its audit artifact is intentionally repairable.
                // Model two complete live claims, not that recovery state. This
                // ledger API checks artifact presence; bundle validation has its
                // own CLI tests and is not replaced by this fixture marker.
                let artifacts = root.join(".edda/control-local/review-bundles");
                std::fs::create_dir_all(&artifacts)?;
                for claim in [&one, &two] {
                    std::fs::write(
                        artifacts.join(format!("{}.json", claim.charter_digest)),
                        b"published audit artifact fixture",
                    )?;
                }
                let gate = root.join("claim-start-gate");
                let one_path = root.join("claim-one.json");
                let two_path = root.join("claim-two.json");
                let out_one = root.join("claim-out-one");
                let out_two = root.join("claim-out-two");
                std::fs::write(&one_path, serde_json::to_vec(&one)?)?;
                std::fs::write(&two_path, serde_json::to_vec(&two)?)?;
                let mut child_one = spawn_contender(
                    "control_tests::effect_tests::production_workspace_lock_selects_one_live_signed_claim",
                    &[
                        (ROOT_ENV, root.display().to_string()),
                        (CLAIM_ENV, one_path.display().to_string()),
                        (OUT_ENV, out_one.display().to_string()),
                    ],
                    &gate,
                );
                let mut child_two = spawn_contender(
                    "control_tests::effect_tests::production_workspace_lock_selects_one_live_signed_claim",
                    &[
                        (ROOT_ENV, root.display().to_string()),
                        (CLAIM_ENV, two_path.display().to_string()),
                        (OUT_ENV, out_two.display().to_string()),
                    ],
                    &gate,
                );
                release_contenders(&gate, [&out_one, &out_two]);
                let status_one = wait_bounded(&mut child_one, Duration::from_secs(60))
                    .expect("first contender finished");
                let status_two = wait_bounded(&mut child_two, Duration::from_secs(60))
                    .expect("second contender finished");
                anyhow::ensure!(status_one.success(), "first contender failed: {status_one:?}");
                anyhow::ensure!(status_two.success(), "second contender failed: {status_two:?}");
                let outcomes = [read_outcome(&out_one), read_outcome(&out_two)];
                anyhow::ensure!(
                    outcomes
                        .iter()
                        .filter(|outcome| outcome.as_str() == "won")
                        .count()
                        == 1,
                    "exactly one live signed claim must win: {outcomes:?}"
                );
                anyhow::ensure!(
                    outcomes
                        .iter()
                        .filter(|outcome| outcome.as_str() == "refused_live")
                        .count()
                        == 1,
                    "exactly one contender must be refused live: {outcomes:?}"
                );
                let persisted = ledger
                    .control_review_claim_for_subject(&one.repository, one.pr_number, &one.head_sha)?
                    .expect("winning signed claim is persisted");
                let winner = if outcomes[0] == "won" { &one } else { &two };
                anyhow::ensure!(
                    &persisted == winner,
                    "persisted subject must retain the winning signed claim"
                );
                ControlEffectResultV1::applied(request.intent.action_kind, "claim_recorded")
            },
        )
        .unwrap();
}

#[test]
fn signed_claim_payload_and_same_head_retry_require_inconclusive_adjudication() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision_for(Some(&bound_portable()));
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_reviewgeneration");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    let compiled = ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();

    let first = ledger.control_next("control_reviewgeneration").unwrap();
    let mut prior = None;
    ledger
        .control_apply_with(
            "control_reviewgeneration",
            first.token.as_deref().unwrap(),
            |request| {
                let signed = ledger.seal_control_review_claim(review_claim_for(request))?;
                ledger.verify_control_review_claim(&signed)?;
                let mut altered = signed.clone();
                altered.charter_digest = "f".repeat(64);
                assert!(ledger.verify_control_review_claim(&altered).is_err());
                prior = Some(signed);
                ControlEffectResultV1::applied(request.intent.action_kind, "claim_recorded")
            },
        )
        .unwrap();
    let prior = prior.unwrap();
    let review = ledger.control_next("control_reviewgeneration").unwrap();
    ledger
        .control_apply_with(
            "control_reviewgeneration",
            review.token.as_deref().unwrap(),
            |_| Ok(ControlEffectResultV1::needs_decision("REVIEW_INCONCLUSIVE")),
        )
        .unwrap();
    let status = ledger.control_status("control_reviewgeneration").unwrap();
    ledger
        .adjudicate_control(
            "control_reviewgeneration",
            status.state_version,
            ControlAdjudicationInputV1 {
                adjudication_version: 1,
                reason_code: "RETRY_REVIEW".into(),
                evidence: vec!["structured review was inconclusive".into()],
                expected_manifest_digest: compiled.manifest_digest,
                clear_return_for_decision: true,
            },
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let retry = ledger.control_next("control_reviewgeneration").unwrap();
    assert_eq!(retry.action_kind, ControlActionKindV1::ClaimVerification);
    ledger
        .control_apply_with(
            "control_reviewgeneration",
            retry.token.as_deref().unwrap(),
            |request| {
                let next = ledger.seal_control_review_claim(review_claim_for(request))?;
                assert!(ledger.control_review_claim_is_supersedable(&prior, &next)?);
                ControlEffectResultV1::applied(request.intent.action_kind, "retry_claim_recorded")
            },
        )
        .unwrap();
}

#[test]
fn adjudication_can_supersede_a_signed_claim_after_claim_evidence_failure() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision_for(Some(&bound_portable()));
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_reviewclaimretry");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    let compiled = ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();

    let first = ledger.control_next("control_reviewclaimretry").unwrap();
    let artifact_digest = "d".repeat(64);
    let artifact = fixture
        .root
        .path()
        .join(".edda/control-local/review-bundles")
        .join(format!("{artifact_digest}.json"));
    std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    let mut prior = None;
    ledger
        .control_apply_with(
            "control_reviewclaimretry",
            first.token.as_deref().unwrap(),
            |request| {
                let claim = ledger.seal_control_review_claim(review_claim_for(request))?;
                assert_eq!(
                    ledger.claim_control_review(&claim)?,
                    ControlReviewClaimOutcomeV1::Won { adopted: false }
                );
                prior = Some(claim);
                std::fs::write(&artifact, "ephemeral audit").unwrap();
                let mut result =
                    ControlEffectResultV1::needs_decision("REVIEW_CLAIM_EVIDENCE_INVALID");
                result.ephemeral_artifact_digest = Some(artifact_digest.clone());
                Ok(result)
            },
        )
        .unwrap();
    assert!(!artifact.exists(), "durable receipt triggers cleanup");
    std::fs::write(&artifact, "crash residue").unwrap();
    let status = ledger.control_status("control_reviewclaimretry").unwrap();
    assert!(artifact.exists(), "read-only status never performs cleanup");
    ledger
        .control_apply_with(
            "control_reviewclaimretry",
            first.token.as_deref().unwrap(),
            |_| panic!("stale token cannot execute its adapter"),
        )
        .unwrap();
    assert!(!artifact.exists(), "mutating recovery retries cleanup");
    ledger
        .adjudicate_control(
            "control_reviewclaimretry",
            status.state_version,
            ControlAdjudicationInputV1 {
                adjudication_version: 1,
                reason_code: "RETRY_CLAIM_EVIDENCE".into(),
                evidence: vec!["immutable remote evidence was unavailable".into()],
                expected_manifest_digest: compiled.manifest_digest,
                clear_return_for_decision: true,
            },
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let retry = ledger.control_next("control_reviewclaimretry").unwrap();
    assert_eq!(retry.action_kind, ControlActionKindV1::ClaimVerification);
    ledger
        .control_apply_with(
            "control_reviewclaimretry",
            retry.token.as_deref().unwrap(),
            |request| {
                let next = ledger.seal_control_review_claim(review_claim_for(request))?;
                assert!(
                    ledger.control_review_claim_is_supersedable(prior.as_ref().unwrap(), &next,)?
                );
                assert_eq!(
                    ledger.claim_control_review(&next)?,
                    ControlReviewClaimOutcomeV1::Won { adopted: false }
                );
                ControlEffectResultV1::applied(request.intent.action_kind, "retry_claim_recorded")
            },
        )
        .unwrap();
}

#[test]
fn needs_decision_receipt_never_satisfies_a_product_prerequisite_after_adjudication() {
    let fixture = Fixture::new();
    let authority_token = fixture.provision_for(Some(&bound_portable()));
    let ledger = Ledger::open(fixture.root.path()).unwrap();
    let mut input = compile_input("control_failedclaim");
    input.manifest.completion_condition = ControlCompletionConditionV1::VerificationSucceeded;
    input.manifest.merge_policy.pr_number = Some(1141);
    input.manifest.merge_policy.expected_head_sha = Some("b".repeat(40));
    input.manifest.merge_policy.expected_base_sha = Some("c".repeat(40));
    let compiled = ledger
        .compile_control(input, &fixture.session, &authority_token)
        .unwrap();
    let claim = ledger.control_next("control_failedclaim").unwrap();
    assert_eq!(claim.action_kind, ControlActionKindV1::ClaimVerification);
    let applied = ledger
        .control_apply_with("control_failedclaim", &claim.token.unwrap(), |_| {
            Ok(crate::ControlEffectResultV1::needs_decision(
                "CLAIM_REFUSED",
            ))
        })
        .unwrap();
    assert_eq!(applied.state, ControlStateV1::NeedsDecision);
    let status = ledger.control_status("control_failedclaim").unwrap();
    ledger
        .adjudicate_control(
            "control_failedclaim",
            status.state_version,
            ControlAdjudicationInputV1 {
                adjudication_version: 1,
                reason_code: "RETRY_CLAIM".into(),
                evidence: vec!["operator rechecked subject".into()],
                expected_manifest_digest: compiled.manifest_digest,
                clear_return_for_decision: true,
            },
            &fixture.session,
            &authority_token,
        )
        .unwrap();
    let retry = ledger.control_next("control_failedclaim").unwrap();
    assert_eq!(retry.action_kind, ControlActionKindV1::ClaimVerification);
    assert_ne!(retry.action_id, claim.action_id);
    assert!(retry.token.is_some());
}
