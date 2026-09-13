#![cfg(unix)]

use edda_core::guided_execution::*;
use edda_ledger::Ledger;
use std::path::PathBuf;
use std::process::{Command, Output};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

fn write_test_secret(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

struct Fixture {
    root: tempfile::TempDir,
    store: tempfile::TempDir,
    session: String,
    issuer_token: PathBuf,
    authority_token: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        Ledger::ensure_initialized(root.path()).unwrap();
        std::fs::create_dir_all(root.path().join(".edda/control-local")).unwrap();
        let issuer_token = root.path().join(".edda/control-local/root.key");
        write_test_secret(&issuer_token, "11".repeat(32).as_bytes());
        let authority_token = root.path().join("authority-token");
        std::fs::write(
            root.path().join(".edda/actors.yaml"),
            "version: 2\nactors:\n  planner:\n    roles: [controller]\n    kind: agent\n    runtime: strong\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join(".edda/policy.yaml"),
            "version: 2\nroles: [controller]\nrules: []\npermissions:\n  default: deny\n  grants:\n    - actions: [control_compile, control_adjudicate]\n      roles: [controller]\n",
        )
        .unwrap();
        let session = "session-cli-control".to_string();
        let project_id = edda_store::project_id(root.path());
        let state = store.path().join("projects").join(project_id).join("state");
        std::fs::create_dir_all(&state).unwrap();
        let now = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        std::fs::write(
            state.join(format!("session.{session}.json")),
            serde_json::to_vec_pretty(&serde_json::json!({
                "session_id": session,
                "started_at": now,
                "last_heartbeat": now,
                "label": "planner",
                "focus_files": [],
                "active_tasks": [],
                "files_modified_count": 0,
                "total_edits": 0,
                "recent_commits": [],
                "branch": "main",
                "current_phase": "planning",
                "pid": std::process::id()
            }))
            .unwrap(),
        )
        .unwrap();
        Self {
            root,
            store,
            session,
            issuer_token,
            authority_token,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_edda"))
            .args(args)
            .current_dir(self.root.path())
            .env("EDDA_STORE_ROOT", self.store.path())
            .env("EDDA_SESSION_ID", &self.session)
            .output()
            .unwrap()
    }

    fn issue_authority(&self) {
        let expiry = (OffsetDateTime::now_utc() + time::Duration::hours(1))
            .format(&Rfc3339)
            .unwrap();
        let out = self.run(&[
            "control",
            "authority",
            "issue",
            "--principal",
            "planner",
            "--session",
            &self.session,
            "--profile",
            "strong",
            "--expires-at",
            &expiry,
            "--issuer-token-file",
            self.issuer_token.to_str().unwrap(),
            "--token-out",
            self.authority_token.to_str().unwrap(),
            "--json",
        ]);
        assert_success(&out);
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(json["status"], "AUTHORITY_ISSUED_LOCAL");
    }
}

#[test]
fn control_cli_compile_status_next_apply_and_adjudicate() {
    let fixture = Fixture::new();
    fixture.issue_authority();
    let mut initial = manifest("control_cliflow");
    initial.return_for_decision = vec!["STRONG_CHOICE".into()];
    let compile_path = fixture.root.path().join("compile.json");
    std::fs::write(
        &compile_path,
        serde_json::to_vec_pretty(&ControlCompileInputV1 {
            compile_version: 1,
            manifest: initial,
            brief_inputs: vec![],
        })
        .unwrap(),
    )
    .unwrap();
    let compile = fixture.run(&[
        "control",
        "compile",
        compile_path.to_str().unwrap(),
        "--authority-token-file",
        fixture.authority_token.to_str().unwrap(),
        "--json",
    ]);
    assert_success(&compile);
    let compiled: serde_json::Value = serde_json::from_slice(&compile.stdout).unwrap();
    let digest = compiled["manifest_digest"].as_str().unwrap();

    let db = fixture.root.path().join(".edda/ledger.db");
    let before = std::fs::metadata(&db).unwrap().modified().unwrap();
    let status = fixture.run(&["control", "status", "control_cliflow", "--json"]);
    assert_success(&status);
    assert_eq!(std::fs::metadata(&db).unwrap().modified().unwrap(), before);
    let status_json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["state"], "prepared");
    assert_eq!(status_json["state_version"], 1);

    let next = fixture.run(&["control", "next", "control_cliflow", "--json"]);
    assert_success(&next);
    let next_json: serde_json::Value = serde_json::from_slice(&next.stdout).unwrap();
    assert_eq!(next_json["action_kind"], "needs_decision");
    let token = next_json["token"].as_str().unwrap();
    let apply = fixture.run(&[
        "control",
        "apply",
        "control_cliflow",
        "--token",
        token,
        "--json",
    ]);
    assert_success(&apply);
    let apply_json: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(apply_json["state"], "needs_decision");

    let decision_path = fixture.root.path().join("decision.json");
    std::fs::write(
        &decision_path,
        serde_json::to_vec_pretty(&ControlAdjudicationInputV1 {
            adjudication_version: 1,
            reason_code: "CONTINUE_LOCAL".into(),
            evidence: vec!["bounded choice".into()],
            expected_manifest_digest: digest.into(),
            clear_return_for_decision: true,
        })
        .unwrap(),
    )
    .unwrap();
    let adjudicate = fixture.run(&[
        "control",
        "adjudicate",
        "control_cliflow",
        "--state-version",
        "2",
        "--decision",
        decision_path.to_str().unwrap(),
        "--authority-token-file",
        fixture.authority_token.to_str().unwrap(),
        "--json",
    ]);
    assert_success(&adjudicate);
    let adjudicated: serde_json::Value = serde_json::from_slice(&adjudicate.stdout).unwrap();
    assert_eq!(adjudicated["manifest_version"], 2);
    assert_eq!(adjudicated["state_version"], 3);
    assert_eq!(adjudicated["state"], "prepared");
}

#[test]
fn task_prepare_uses_real_sealed_path_and_absence_is_unavailable() {
    let fixture = Fixture::new();
    let brief_path = fixture.root.path().join("brief.json");
    std::fs::write(
        &brief_path,
        serde_json::to_vec_pretty(&execution_brief()).unwrap(),
    )
    .unwrap();
    std::fs::write(&fixture.authority_token, "00".repeat(32)).unwrap();
    let absent = fixture.run(&[
        "task",
        "prepare",
        "--file",
        brief_path.to_str().unwrap(),
        "--authority-token-file",
        fixture.authority_token.to_str().unwrap(),
        "--json",
    ]);
    assert!(!absent.status.success());
    assert!(String::from_utf8_lossy(&absent.stderr).contains("CONTROL_UNAVAILABLE"));
    assert_eq!(
        Ledger::open_existing(fixture.root.path())
            .unwrap()
            .count_events()
            .unwrap(),
        0
    );
    std::fs::remove_file(&fixture.authority_token).unwrap();

    fixture.issue_authority();
    let prepared = fixture.run(&[
        "task",
        "prepare",
        "--file",
        brief_path.to_str().unwrap(),
        "--authority-token-file",
        fixture.authority_token.to_str().unwrap(),
        "--json",
    ]);
    assert_success(&prepared);
    let json: serde_json::Value = serde_json::from_slice(&prepared.stdout).unwrap();
    assert_eq!(json["status"], "PREPARED_LOCAL");
    let event = json["brief_event_id"].as_str().unwrap();
    let digest = json["content_digest"].as_str().unwrap();
    let shown = fixture.run(&["task", "show", event, "--digest", digest, "--json"]);
    assert_success(&shown);
    let shown_json: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown_json["brief_id"], "brief_cliprepare");
    assert!(Ledger::open_existing(fixture.root.path())
        .unwrap()
        .task_views()
        .unwrap()
        .is_empty());
}

#[test]
fn caller_asserted_strong_profile_without_bearer_cannot_compile() {
    let fixture = Fixture::new();
    fixture.issue_authority();
    let compile_path = fixture.root.path().join("forged-strong.json");
    std::fs::write(
        &compile_path,
        serde_json::to_vec_pretty(&ControlCompileInputV1 {
            compile_version: 1,
            manifest: manifest("control_forgedstrong"),
            brief_inputs: vec![],
        })
        .unwrap(),
    )
    .unwrap();
    std::fs::write(&fixture.authority_token, vec![b'x'; 129]).unwrap();
    let oversized = fixture.run(&[
        "control",
        "compile",
        compile_path.to_str().unwrap(),
        "--authority-token-file",
        fixture.authority_token.to_str().unwrap(),
        "--json",
    ]);
    assert!(!oversized.status.success());
    assert!(String::from_utf8_lossy(&oversized.stderr).contains("exceeds its bound"));

    std::fs::write(&fixture.authority_token, "22".repeat(32)).unwrap();
    let output = fixture.run(&[
        "control",
        "compile",
        compile_path.to_str().unwrap(),
        "--authority-token-file",
        fixture.authority_token.to_str().unwrap(),
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not possess"));
    assert_eq!(
        Ledger::open_existing(fixture.root.path())
            .unwrap()
            .count_events()
            .unwrap(),
        0
    );
}

#[test]
fn issuer_and_bearer_files_must_be_owner_only_before_read() {
    use std::os::unix::fs::PermissionsExt;

    let issuer = Fixture::new();
    std::fs::set_permissions(&issuer.issuer_token, std::fs::Permissions::from_mode(0o644)).unwrap();
    let expiry = (OffsetDateTime::now_utc() + time::Duration::hours(1))
        .format(&Rfc3339)
        .unwrap();
    let refused = issuer.run(&[
        "control",
        "authority",
        "issue",
        "--principal",
        "planner",
        "--session",
        &issuer.session,
        "--profile",
        "strong",
        "--expires-at",
        &expiry,
        "--issuer-token-file",
        issuer.issuer_token.to_str().unwrap(),
        "--token-out",
        issuer.authority_token.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("permissions are not owner-only"));
    assert!(!issuer.authority_token.exists());

    let bearer = Fixture::new();
    bearer.issue_authority();
    std::fs::set_permissions(
        &bearer.authority_token,
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let compile_path = bearer.root.path().join("owner-only.json");
    std::fs::write(
        &compile_path,
        serde_json::to_vec_pretty(&ControlCompileInputV1 {
            compile_version: 1,
            manifest: manifest("control_owneronly"),
            brief_inputs: vec![],
        })
        .unwrap(),
    )
    .unwrap();
    let refused = bearer.run(&[
        "control",
        "compile",
        compile_path.to_str().unwrap(),
        "--authority-token-file",
        bearer.authority_token.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("permissions are not owner-only"));
    assert_eq!(
        Ledger::open_existing(bearer.root.path())
            .unwrap()
            .count_events()
            .unwrap(),
        0
    );
}

#[test]
fn flash_authority_profile_is_denied_by_the_cli() {
    let fixture = Fixture::new();
    let expiry = (OffsetDateTime::now_utc() + time::Duration::hours(1))
        .format(&Rfc3339)
        .unwrap();
    let output = fixture.run(&[
        "control",
        "authority",
        "issue",
        "--principal",
        "planner",
        "--session",
        &fixture.session,
        "--profile",
        "flash",
        "--expires-at",
        &expiry,
        "--issuer-token-file",
        fixture.issuer_token.to_str().unwrap(),
        "--token-out",
        fixture.authority_token.to_str().unwrap(),
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Flash command profile"));
}

fn manifest(control_id: &str) -> ControlManifestInputV1 {
    ControlManifestInputV1 {
        control_version: 1,
        control_id: control_id.into(),
        program_id: "program_clitest".into(),
        command_profile: ControlCommandProfileV1::Strong,
        goal: "exercise the local control API".into(),
        exclusions: vec!["external effects".into()],
        basis: ControlBasisV1 {
            portable_repo_id: None,
            github_repository: None,
            base_full_sha: "a".repeat(40),
            references: vec!["issue:#1141".into()],
        },
        tasks: vec![],
        capacity: ControlCapacityV1 {
            max_workers: 1,
            verifier_capacity: 1,
        },
        admission_policy: ControlAdmissionPolicyV1 {
            allowed_issue_stage_labels: vec![],
            forbidden_hold_labels: vec![],
            claim_identity: "machine/controller".into(),
            winner_readback_required: true,
            existing_claim_check_required: true,
            delivery_pr_check_required: true,
        },
        routes: vec![],
        retry_cost_policy: ControlRetryCostPolicyV1 {
            per_action_preflight_cost_microusd: 0,
            per_action_incremental_cap_microusd: 0,
            aggregate_stop_microusd: 0,
            missing_cost_needs_decision: true,
            retry_cap: 0,
        },
        review_policy: ControlReviewPolicyV1 {
            verifier_identity: "verifier".into(),
            verifier_profile: "pi:provider/model".into(),
            frozen_surface_source: "manifest".into(),
            one_live_claim_per_pr_head: true,
        },
        merge_policy: ControlMergePolicyV1 {
            required: false,
            pr_number: None,
            expected_head_sha: None,
            expected_base_sha: None,
            authority_capability_ref: None,
            eligibility_product_verb: "edda review merge".into(),
        },
        return_for_decision: vec![],
        completion_condition: ControlCompletionConditionV1::LocalPreparationOnly,
    }
}

fn execution_brief() -> ExecutionBriefInputV1 {
    ExecutionBriefInputV1 {
        brief_version: 1,
        brief_id: "brief_cliprepare".into(),
        task_ref: None,
        runtime_profile: RuntimeProfileV1::Strong,
        intent: ExecutionIntentV1::Document,
        objective: "render one immutable accepted brief".into(),
        basis: ExecutionBasisV1 {
            portable_repo_id: None,
            base_full_sha: "a".repeat(40),
            issue_spec_refs: vec!["issue:#1141".into()],
        },
        scope: ExecutionScopeV1 {
            allowed_paths: vec!["docs/**".into()],
            out_of_scope: vec![],
        },
        read_order: vec![],
        known_facts: vec![],
        allowed_decisions: vec![],
        return_for_decision: vec![],
        procedure: TrustedProcedureV1::ControllerAuthored {
            authored_by: "planner".into(),
            principles: vec!["preserve immutable identity".into()],
            probe_cards: vec![],
            implementation_steps: vec![],
            validation: vec![],
        },
        outcome_codes: vec![OutcomeCodeV1 {
            code: "DONE".into(),
            result_class: ResultClassV1::Success,
        }],
        receipt_schema: ReceiptSchemaV1 {
            receipt_version: 1,
            required_fields: vec![
                ReceiptFieldV1::BriefIdentity,
                ReceiptFieldV1::OutcomeCode,
                ReceiptFieldV1::ChangedPaths,
                ReceiptFieldV1::ValidationRan,
                ReceiptFieldV1::ValidationRead,
                ReceiptFieldV1::RecommendedNextAction,
            ],
        },
    }
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
