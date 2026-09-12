#[cfg(unix)]
use edda_core::guided_execution::*;
#[cfg(unix)]
use edda_ledger::{ControlAuthorityProvision, Ledger};
#[cfg(unix)]
use edda_store::SessionHeartbeat;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use time::format_description::well_known::Rfc3339;
#[cfg(unix)]
use time::OffsetDateTime;

#[cfg(unix)]
struct EnvRestore {
    key: &'static str,
    value: Option<std::ffi::OsString>,
}

#[cfg(unix)]
impl EnvRestore {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let restore = Self {
            key,
            value: std::env::var_os(key),
        };
        std::env::set_var(key, value);
        restore
    }
}

#[cfg(unix)]
impl Drop for EnvRestore {
    fn drop(&mut self) {
        match self.value.take() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

#[cfg(unix)]
fn compile_input(control_id: &str, base: &str, head: &str) -> ControlCompileInputV1 {
    ControlCompileInputV1 {
        compile_version: 1,
        manifest: ControlManifestInputV1 {
            control_version: 1,
            control_id: control_id.into(),
            program_id: "program_effectpipeline".into(),
            command_profile: ControlCommandProfileV1::Strong,
            goal: "claim one immutable review subject".into(),
            exclusions: vec!["review-result ingestion belongs to S7".into()],
            basis: ControlBasisV1 {
                portable_repo_id: None,
                github_repository: Some("owner/repo".into()),
                base_full_sha: base.into(),
                references: vec!["issue:#1141".into()],
            },
            tasks: vec![],
            capacity: ControlCapacityV1 {
                max_workers: 1,
                verifier_capacity: 1,
            },
            admission_policy: ControlAdmissionPolicyV1 {
                allowed_issue_stage_labels: vec![],
                forbidden_hold_labels: vec!["hold".into()],
                claim_identity: "machine/controller".into(),
                winner_readback_required: true,
                existing_claim_check_required: true,
                delivery_pr_check_required: true,
            },
            routes: vec![],
            retry_cost_policy: ControlRetryCostPolicyV1 {
                per_action_preflight_cost_microusd: 1,
                per_action_incremental_cap_microusd: 10,
                aggregate_stop_microusd: 100,
                missing_cost_needs_decision: true,
                retry_cap: 1,
            },
            review_policy: ControlReviewPolicyV1 {
                verifier_identity: "independent-verifier".into(),
                verifier_profile: "pi:provider/model".into(),
                frozen_surface_source: "manifest".into(),
                one_live_claim_per_pr_head: true,
            },
            merge_policy: ControlMergePolicyV1 {
                required: false,
                pr_number: Some(1141),
                expected_head_sha: Some(head.into()),
                expected_base_sha: Some(base.into()),
                authority_capability_ref: None,
                eligibility_product_verb: "edda review merge".into(),
            },
            return_for_decision: vec![],
            completion_condition: ControlCompletionConditionV1::VerificationSucceeded,
        },
        brief_inputs: vec![],
    }
}

#[cfg(unix)]
#[test]
fn next_apply_product_adapter_records_one_atomic_review_claim_and_receipt() {
    let _env_lock = crate::claim_guard::GH_BIN_ENV_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let _store = edda_store::test_support::isolated_store_root().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(root.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ])
        .current_dir(root.path())
        .status()
        .unwrap();
    for (key, value) in [("user.email", "test@example.com"), ("user.name", "Test")] {
        std::process::Command::new("git")
            .args(["config", key, value])
            .current_dir(root.path())
            .status()
            .unwrap();
    }
    std::fs::write(root.path().join("subject.txt"), "base\n").unwrap();
    std::fs::write(root.path().join("REVIEW.md"), "# Base review policy\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "subject.txt", "REVIEW.md"])
        .current_dir(root.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "base"])
        .current_dir(root.path())
        .status()
        .unwrap();
    let rev = |name: &str| {
        String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", name])
                .current_dir(root.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned()
    };
    let base = rev("HEAD");
    std::fs::write(root.path().join("subject.txt"), "head\n").unwrap();
    std::process::Command::new("git")
        .args(["commit", "-qam", "head"])
        .current_dir(root.path())
        .status()
        .unwrap();
    let head = rev("HEAD");
    std::fs::write(
        root.path().join(".git/info/exclude"),
        ".edda/\nfake-gh.sh\natomic-review-ref\natomic-review-message\nobserved-gh-context\nauthority-token\n",
    )
    .unwrap();
    Ledger::ensure_initialized(root.path()).unwrap();
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
    std::fs::create_dir_all(root.path().join(".edda/control-local")).unwrap();
    write_private(
        &root.path().join(".edda/control-local/root.key"),
        "11".repeat(32).as_bytes(),
    );
    let session = "session-effect-pipeline";
    let now = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let project_id = edda_store::project_id(root.path());
    edda_store::ensure_dirs(&project_id).unwrap();
    edda_store::write_heartbeat(
        &project_id,
        &SessionHeartbeat {
            session_id: session.into(),
            started_at: now.clone(),
            last_heartbeat: now,
            label: "planner".into(),
            focus_files: vec![],
            active_tasks: vec![],
            files_modified_count: 0,
            total_edits: 0,
            recent_commits: vec![],
            branch: Some("main".into()),
            current_phase: Some("control".into()),
            parent_session_id: None,
            plan: Some("effect-pipeline".into()),
            phase: Some("claim".into()),
            attempt: Some(1),
            stage: Some("running".into()),
            pid: Some(std::process::id()),
        },
    )
    .unwrap();
    let fake = root.path().join("fake-gh.sh");
    let state = root.path().join("atomic-review-ref");
    let message = root.path().join("atomic-review-message");
    let observed = root.path().join("observed-gh-context");
    std::fs::write(
        &fake,
        format!(
            r#"#!/bin/sh
state='{}'
message='{}'
observed='{}'
printf '%s|%s|%s|%s\n' "${{GH_REPO-}}" "${{EDDA_REPO-}}" "${{GH_HOST-}}" "$*" >> "$observed"
if [ "$1" = pr ]; then
  echo '{{"headRefOid":"{}","headRefName":"feature","baseRefName":"main","baseRefOid":"{}","state":"OPEN"}}'
  exit 0
fi
if [ "$1" = issue ]; then
  echo '{{"body":"frozen issue acceptance"}}'
  exit 0
fi
if [ "$1" = api ] && [ "$2" = user ]; then
  echo '{{"login":"controller"}}'
  exit 0
fi
endpoint=''
for arg in "$@"; do case "$arg" in repos/*) endpoint="$arg";; esac; done
case "$endpoint" in
  */git/ref/tags/edda/review-claims/*)
    [ -s "$state" ] || exit 1
    ref=$(cat "$state")
    printf '{{"ref":"%s","object":{{"type":"tag","sha":"{}"}}}}\n' "$ref"
    ;;
  */git/tags/{})
    escaped=$(sed 's/\\/\\\\/g;s/"/\\"/g' "$message")
    printf '{{"object":{{"type":"commit","sha":"{}"}},"message":"%s"}}\n' "$escaped"
    ;;
  */git/tags)
    for arg in "$@"; do case "$arg" in message=*) printf '%s' "${{arg#message=}}" > "$message";; esac; done
    echo '{{"sha":"{}"}}'
    ;;
  */git/refs)
    [ -e "$state" ] && exit 1
    for arg in "$@"; do case "$arg" in ref=*) printf '%s' "${{arg#ref=}}" > "$state";; esac; done
    echo '{{"created":true}}'
    ;;
  *) exit 1;;
esac
"#,
            state.display(),
            message.display(),
            observed.display(),
            &head,
            &base,
            "a".repeat(40),
            "a".repeat(40),
            &head,
            "a".repeat(40),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let transport = crate::cmd_review::github::ControlledGitHubTransport::injected(&fake);
    let _gh_repo = EnvRestore::set("GH_REPO", "attacker/foreign");
    let _edda_repo = EnvRestore::set("EDDA_REPO", "attacker/foreign");
    let _gh_host = EnvRestore::set("GH_HOST", "example.invalid");

    let ledger = Ledger::open(root.path()).unwrap();
    let portable = edda_store::continuity::derive_portable_repository_identity(
        root.path(),
        &ledger.paths.config_json,
    )
    .unwrap()
    .portable_repo_id
    .unwrap();
    let expiry = (OffsetDateTime::now_utc() + time::Duration::hours(1))
        .format(&Rfc3339)
        .unwrap();
    ledger
        .provision_control_authority(ControlAuthorityProvision {
            principal_id: "planner",
            session_id: session,
            command_profile: ControlCommandProfileV1::Strong,
            portable_repo_id: Some(&portable),
            expires_at: &expiry,
            issuer_token: &"11".repeat(32),
            authority_token_out: &root.path().join("authority-token"),
        })
        .unwrap();
    let authority_token = std::fs::read_to_string(root.path().join("authority-token")).unwrap();
    let mut input = compile_input("control_effectpipeline", &base, &head);
    input.manifest.basis.portable_repo_id = Some(portable);
    ledger
        .compile_control(input, session, &authority_token)
        .unwrap();
    let next = ledger.control_next("control_effectpipeline").unwrap();
    assert_eq!(next.action_kind, ControlActionKindV1::ClaimVerification);
    let applied = ledger
        .control_apply_with("control_effectpipeline", &next.token.unwrap(), |request| {
            crate::cmd_control_effects::apply_with_transport(
                root.path(),
                &ledger,
                request,
                &transport,
            )
        })
        .unwrap();
    assert!(applied.applied);
    assert_eq!(applied.state, ControlStateV1::VerificationClaimed);
    assert_eq!(
        ledger
            .control_next("control_effectpipeline")
            .unwrap()
            .action_kind,
        ControlActionKindV1::RequestVerification
    );
    assert!(state.is_file());
    let calls = std::fs::read_to_string(observed).unwrap();
    assert!(calls.lines().all(|line| line.starts_with("|||")));
    assert!(calls.contains("--repo owner/repo"));
    assert!(calls.contains("repos/owner/repo/git/refs"));
    assert_eq!(
        ledger
            .control_receipts("control_effectpipeline")
            .unwrap()
            .len(),
        1
    );
}
