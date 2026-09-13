#[path = "cmd_control_review_bundle.rs"]
mod control_review_bundle;
#[path = "cmd_control_verification.rs"]
mod control_verification;
#[path = "control_dispatch_safety.rs"]
mod dispatch_safety;

use self::control_verification::{claim_verification, request_verification};
use self::dispatch_safety::has_ambiguous_dispatch_predecessor;
use crate::agent_kind::AgentKind;
use crate::cmd_dispatch::DispatchArgs;
use crate::cmd_reconcile::runner::{
    attempt_branch, attempt_worktree_path, controlled_attempt_descriptor_from_data,
    ensure_control_attempt_worktree, git, git_success, worktree_registered_for_branch,
};
use crate::cmd_review::github::{
    pr_delivery_subject_for, ControlledGitHubTransport, GitHubRepository,
};
use crate::detached_dispatch::{self, IdempotentLaunch};
use anyhow::Context;
use edda_core::guided_execution::{
    parse_work_receipt, render_execution_brief, ControlActionKindV1, ControlStateV1,
    ControlTargetV1, DeliveryRequirementV1, DeliveryV1, ReceiptExpectationV1, WorkReceiptV1,
};
use edda_ledger::task_actions::{self, CONTROLLED_TASK_LEASE_PREFIX};
use edda_ledger::{
    ControlEffectRequestV1, ControlEffectResultV1, Ledger, TaskLease, TaskStatus, WorkspaceLock,
};
use std::path::{Path, PathBuf};

pub(crate) fn apply(
    repo_root: &Path,
    ledger: &Ledger,
    request: &ControlEffectRequestV1,
) -> anyhow::Result<ControlEffectResultV1> {
    let transport = match ControlledGitHubTransport::production() {
        Ok(transport) => transport,
        Err(_) => {
            return Ok(ControlEffectResultV1::needs_decision(
                "CONTROLLED_GITHUB_TRANSPORT_REFUSED",
            ));
        }
    };
    apply_with_transport(repo_root, ledger, request, &transport)
}

pub(crate) fn apply_with_transport(
    repo_root: &Path,
    ledger: &Ledger,
    request: &ControlEffectRequestV1,
    transport: &ControlledGitHubTransport,
) -> anyhow::Result<ControlEffectResultV1> {
    if !request.intent.action_kind.is_local_s6a() {
        if let Err(_error) = validate_portable_repo_binding(repo_root, request) {
            return Ok(ControlEffectResultV1::needs_decision(
                "PORTABLE_REPOSITORY_BINDING_REFUSED",
            ));
        }
    }
    let result = match request.intent.action_kind {
        ControlActionKindV1::AdmitAndClaimIssue => admit(repo_root, ledger, request, transport),
        ControlActionKindV1::PrepareAttempt => prepare(repo_root, ledger, request, transport),
        ControlActionKindV1::DispatchTask => dispatch(repo_root, ledger, request),
        ControlActionKindV1::BindDelivery => bind_delivery(repo_root, ledger, request, transport),
        ControlActionKindV1::ClaimVerification => {
            claim_verification(repo_root, ledger, request, transport)
        }
        ControlActionKindV1::RequestVerification => {
            request_verification(repo_root, ledger, request, transport)
        }
        ControlActionKindV1::MergeDelegated => merge(repo_root, ledger, request),
        action if action.is_local_s6a() => {
            ControlEffectResultV1::applied(action, "local_ledger_transition_applied")
        }
        _ => Ok(ControlEffectResultV1::needs_decision(
            "CONTROL_ACTION_NOT_IMPLEMENTED",
        )),
    };
    match result {
        Ok(result) => Ok(result),
        Err(_) => {
            let mut refused = ControlEffectResultV1::needs_decision("CONTROL_EFFECT_REFUSED");
            if request.intent.action_kind == ControlActionKindV1::RequestVerification {
                refused.ephemeral_artifact_digest = ledger
                    .control_receipts(&request.manifest.control_id)
                    .ok()
                    .and_then(|receipts| {
                        receipts.into_iter().find(|receipt| {
                            receipt.action_kind == ControlActionKindV1::ClaimVerification
                                && receipt.target == request.intent.target
                                && receipt.observed_state_version.checked_add(1)
                                    == Some(request.intent.observed_state_version)
                        })
                    })
                    .and_then(|receipt| receipt.external_identity)
                    .and_then(|identity| {
                        identity
                            .rsplit_once(":audit-sha256:")
                            .map(|(_, digest)| digest.to_owned())
                    });
            }
            Ok(refused)
        }
    }
}

fn validate_portable_repo_binding(
    repo_root: &Path,
    request: &ControlEffectRequestV1,
) -> anyhow::Result<()> {
    let Some(expected) = request.manifest.basis.portable_repo_id.as_deref() else {
        return Ok(());
    };
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    let observed =
        edda_store::continuity::derive_portable_repository_identity(repo_root, &paths.config_json)?;
    anyhow::ensure!(
        observed.portable_repo_id.as_deref() == Some(expected),
        "control action repository differs from the portable manifest binding"
    );
    if let Some(repository) = request.manifest.basis.github_repository.as_deref() {
        GitHubRepository::for_control(repo_root, expected, repository)?;
    }
    Ok(())
}

fn github_repository(
    repo_root: &Path,
    request: &ControlEffectRequestV1,
) -> anyhow::Result<GitHubRepository> {
    let portable = request
        .manifest
        .basis
        .portable_repo_id
        .as_deref()
        .context("GitHub control effect requires a portable repository binding")?;
    let repository = request
        .manifest
        .basis
        .github_repository
        .as_deref()
        .context("GitHub control effect requires a canonical repository binding")?;
    GitHubRepository::for_control(repo_root, portable, repository)
}

fn task(
    request: &ControlEffectRequestV1,
) -> anyhow::Result<&edda_core::guided_execution::ControlTaskV1> {
    let key = match &request.intent.target {
        ControlTargetV1::Task { task_key, .. } | ControlTargetV1::Delivery { task_key, .. } => {
            task_key
        }
        ControlTargetV1::PullRequest { .. } | ControlTargetV1::Control { .. } => {
            anyhow::bail!("control action has no task target")
        }
    };
    request
        .manifest
        .tasks
        .iter()
        .find(|task| &task.task_key == key)
        .context("control action target names an unknown task")
}

fn target_attempt(request: &ControlEffectRequestV1) -> anyhow::Result<u32> {
    match request.intent.target {
        ControlTargetV1::Task { attempt, .. } | ControlTargetV1::Delivery { attempt, .. } => {
            Ok(attempt)
        }
        _ => anyhow::bail!("control action target omits its task attempt"),
    }
}

fn issue_number(binding: &str) -> anyhow::Result<u64> {
    let value = binding
        .strip_prefix("issue:#")
        .context("control issue binding must be issue:#<number>")?;
    let number = value.parse::<u64>()?;
    anyhow::ensure!(number > 0, "control issue number must be positive");
    Ok(number)
}

fn admit(
    repo_root: &Path,
    ledger: &Ledger,
    request: &ControlEffectRequestV1,
    transport: &ControlledGitHubTransport,
) -> anyhow::Result<ControlEffectResultV1> {
    let task = task(request)?;
    if task.local_only {
        return ControlEffectResultV1::applied(
            request.intent.action_kind,
            "local_task_admission_not_required",
        );
    }
    let issue = issue_number(
        task.issue_binding
            .as_deref()
            .context("issue-bound control task omits issue binding")?,
    )?;
    let predecessor_action_ids = ledger
        .control_receipts(&request.manifest.control_id)?
        .into_iter()
        .filter(|receipt| {
            receipt.action_kind == ControlActionKindV1::AdmitAndClaimIssue
                && receipt.target == request.intent.target
                && receipt.next_state == ControlStateV1::NeedsDecision
        })
        .map(|receipt| receipt.action_id)
        .collect::<Vec<_>>();
    let repository = github_repository(repo_root, request)?;
    match crate::claim_guard::admit_control_issue(
        issue,
        &request.manifest.admission_policy.claim_identity,
        &request.intent.action_id,
        &predecessor_action_ids,
        &request.manifest.admission_policy.allowed_issue_stage_labels,
        &request.manifest.admission_policy.forbidden_hold_labels,
        repo_root,
        &repository,
        transport,
    )? {
        crate::claim_guard::ControlIssueAdmission::Won {
            adopted,
            external_identity,
        } => {
            let mut result = ControlEffectResultV1::applied(
                request.intent.action_kind,
                if adopted {
                    "issue_claim_adopted"
                } else {
                    "issue_claim_won"
                },
            )?;
            result.external_identity = Some(external_identity);
            Ok(result)
        }
        crate::claim_guard::ControlIssueAdmission::Refused { .. } => Ok(
            ControlEffectResultV1::needs_decision("ISSUE_ADMISSION_REFUSED"),
        ),
    }
}

fn prepare(
    repo_root: &Path,
    ledger: &Ledger,
    request: &ControlEffectRequestV1,
    transport: &ControlledGitHubTransport,
) -> anyhow::Result<ControlEffectResultV1> {
    let control_task = task(request)?;
    validate_host_affinity(control_task)?;
    if !control_task.local_only {
        let issue = issue_number(
            control_task
                .issue_binding
                .as_deref()
                .context("controlled product task omits issue binding")?,
        )?;
        let repository = github_repository(repo_root, request)?;
        crate::claim_guard::ensure_control_issue_ready(
            issue,
            &request.manifest.admission_policy.claim_identity,
            &request.manifest.admission_policy.forbidden_hold_labels,
            repo_root,
            &repository,
            transport,
        )?;
    }
    let task_id = control_task
        .task_id
        .context("controlled attempt requires a Task Rail task ID")?;
    let attempt = target_attempt(request)?;
    anyhow::ensure!(
        attempt.saturating_sub(1) <= u32::from(request.manifest.retry_cost_policy.retry_cap),
        "controlled task retry cap is exhausted"
    );
    let mut view = task_actions::find_view(&ledger.task_views()?, task_id)?.clone();
    match view.status {
        TaskStatus::Ready | TaskStatus::Failed => {
            let started = task_actions::start_task(repo_root, task_id, 300, &|_, _| {})?;
            anyhow::ensure!(
                started.attempt == attempt,
                "controlled attempt number changed"
            );
            view = task_actions::find_view(&ledger.task_views()?, task_id)?.clone();
        }
        TaskStatus::Running => anyhow::ensure!(
            view.attempts == attempt,
            "controlled task is running a different attempt"
        ),
        _ => anyhow::bail!("controlled task is not eligible for attempt preparation"),
    }
    anyhow::ensure!(
        view.scope_paths == control_task.owned_paths,
        "control task scope differs from Task Rail scope"
    );
    let owner = ensure_control_lease(ledger, control_task, attempt, &request.manifest.control_id)?;
    let accepted = ledger.load_execution_brief(
        &control_task.brief.brief_event_id,
        &control_task.brief.content_digest,
    )?;
    let worktree =
        ensure_control_attempt_worktree(repo_root, &view, attempt, &control_task.exact_input_sha)?;
    let descriptor = controlled_attempt_descriptor_from_data(
        repo_root,
        task_id,
        attempt,
        &owner,
        &control_task.brief.brief_event_id,
        &control_task.brief.content_digest,
        ledger,
        accepted,
    )?;
    anyhow::ensure!(
        descriptor.base_full_sha == control_task.exact_input_sha
            && descriptor.suggested_worktree == worktree
            && descriptor.execution == "none",
        "reconcile returned a descriptor for a different controlled attempt"
    );
    let mut result = ControlEffectResultV1::applied(
        request.intent.action_kind,
        if request.recovering {
            "attempt_descriptor_adopted"
        } else {
            "attempt_descriptor_prepared"
        },
    )?;
    result.external_identity = Some(format!(
        "attempt:{task_id}:{attempt}:{}",
        descriptor.current_lease_owner
    ));
    Ok(result)
}

fn validate_host_affinity(task: &edda_core::guided_execution::ControlTaskV1) -> anyhow::Result<()> {
    if task.execution_host_affinity == "local" {
        return Ok(());
    }
    let machine = std::env::var("EDDA_MACHINE")
        .context("non-local execution host affinity requires explicit EDDA_MACHINE")?;
    anyhow::ensure!(
        machine == task.execution_host_affinity,
        "control task is bound to a different execution host"
    );
    Ok(())
}

fn ensure_control_lease(
    ledger: &Ledger,
    task: &edda_core::guided_execution::ControlTaskV1,
    attempt: u32,
    control_id: &str,
) -> anyhow::Result<String> {
    let task_id = task.task_id.context("control task omits task ID")?;
    let owner = format!(
        "{CONTROLLED_TASK_LEASE_PREFIX}{}:{}:{control_id}:{}:{attempt}",
        task.brief.brief_event_id, task.brief.content_digest, task.task_key
    );
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;
    let now = chrono::Utc::now();
    if let Some(mut lease) = ledger.task_lease(task_id)? {
        anyhow::ensure!(
            lease.attempt == attempt && lease.owner == owner,
            "controlled attempt lease belongs to another control attempt"
        );
        lease.heartbeat_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        lease.expires_at = (now + chrono::Duration::minutes(35))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        ledger.upsert_task_lease(&lease)?;
        return Ok(lease.owner);
    }
    let lease = TaskLease {
        task_id,
        attempt,
        owner,
        heartbeat_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        expires_at: (now + chrono::Duration::minutes(35))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    ledger.upsert_task_lease(&lease)?;
    Ok(lease.owner)
}

fn dispatch(
    repo_root: &Path,
    ledger: &Ledger,
    request: &ControlEffectRequestV1,
) -> anyhow::Result<ControlEffectResultV1> {
    let control_task = task(request)?;
    validate_host_affinity(control_task)?;
    let task_id = control_task
        .task_id
        .context("dispatch task omits task ID")?;
    let attempt = target_attempt(request)?;
    let view = task_actions::find_view(&ledger.task_views()?, task_id)?.clone();
    let worktree = attempt_worktree_path(repo_root, task_id, attempt)?;
    let session_id =
        edda_conductor::agent::launcher::phase_session_id("control", &request.intent.action_id)
            .to_string();
    let agent = parse_agent(
        view.agent_kind
            .as_deref()
            .context("controlled task omits its agent kind")?,
    )?;
    let cap = request
        .manifest
        .retry_cost_policy
        .per_action_incremental_cap_microusd;
    let preflight = request
        .manifest
        .retry_cost_policy
        .per_action_preflight_cost_microusd;
    if let Some(outcome) = detached_dispatch::inspect_control_action(
        &worktree,
        &session_id,
        &request.intent.action_id,
    )? {
        let (refusal, reserved) =
            dispatch_cost_reservation(ledger, request, agent, cap, preflight)?;
        if let Some(reason) = refusal {
            return Ok(ControlEffectResultV1::needs_decision(reason));
        }
        return dispatch_outcome_result(request, outcome, reserved, preflight);
    }
    anyhow::ensure!(
        view.status == TaskStatus::Running && view.attempts == attempt,
        "dispatch task is not running at the manifested attempt"
    );
    anyhow::ensure!(
        worktree.is_dir(),
        "controlled attempt worktree is unavailable"
    );
    ensure_control_lease(ledger, control_task, attempt, &request.manifest.control_id)?;
    if has_ambiguous_dispatch_predecessor(ledger, request)? {
        return Ok(ControlEffectResultV1::needs_decision(
            "DISPATCH_PREDECESSOR_SPAWN_BOUNDARY_AMBIGUOUS",
        ));
    }
    let accepted = ledger.load_execution_brief(
        &control_task.brief.brief_event_id,
        &control_task.brief.content_digest,
    )?;
    let (cost_refusal, reserved_cost) =
        dispatch_cost_reservation(ledger, request, agent, cap, preflight)?;
    if let Some(reason) = cost_refusal {
        return Ok(ControlEffectResultV1::needs_decision(reason));
    }
    if (agent.is_acp()
        && !matches!(control_task.model_target.as_str(), "strong")
        && control_task.model_target != agent.as_str())
        || (!agent.is_acp()
            && !agent.supports_model()
            && !matches!(control_task.model_target.as_str(), "" | "inherited"))
    {
        return Ok(ControlEffectResultV1::needs_decision(
            "DISPATCH_MODEL_TARGET_UNENFORCEABLE",
        ));
    }
    let prompt_file = if agent.is_acp() {
        None
    } else {
        Some(write_dispatch_prompt(
            repo_root,
            &request.intent.action_id,
            &with_control_receipt_binding(
                render_execution_brief(&accepted.brief)?,
                &request.manifest.control_id,
                &request.intent.step_id,
            ),
        )?)
    };
    let args = DispatchArgs {
        owns: control_task.owned_paths.clone(),
        detach: true,
        build_lane: control_task.build_lane.clone(),
        detach_log_dir: None,
        agent,
        task_id: agent.is_acp().then_some(task_id),
        brief_event_id: agent
            .is_acp()
            .then(|| control_task.brief.brief_event_id.clone()),
        brief_digest: agent
            .is_acp()
            .then(|| control_task.brief.content_digest.clone()),
        prompt_file: prompt_file.map(|path| path.to_string_lossy().into_owned()),
        session_id: Some(session_id.clone()),
        resume: false,
        cwd: Some(worktree.to_string_lossy().into_owned()),
        budget_usd: (reserved_cost > 0).then_some(reserved_cost as f64 / 1_000_000.0),
        timeout_sec: None,
        permission_mode: None,
        model: (!agent.is_acp()
            && agent.supports_model()
            && !matches!(control_task.model_target.as_str(), "" | "inherited"))
        .then(|| control_task.model_target.clone()),
        thinking: None,
        tools: None,
        exclude_tools: None,
        session_dir: None,
        list_models: None,
        issue: None,
        machine: None,
        json: true,
    };
    validate_dispatch_worktree(repo_root, task_id, attempt, &control_task.exact_input_sha)?;
    if !agent.is_acp() {
        ensure_non_acp_task_session(ledger, &view, control_task, attempt, agent, &session_id)?;
    }
    let outcome = detached_dispatch::launch_idempotent(
        &args,
        &worktree,
        &session_id,
        &request.intent.action_id,
    )?;
    dispatch_outcome_result(request, outcome, reserved_cost, preflight)
}

fn dispatch_outcome_result(
    request: &ControlEffectRequestV1,
    outcome: IdempotentLaunch,
    reserved_cost: u64,
    preflight: u64,
) -> anyhow::Result<ControlEffectResultV1> {
    let (output, product_result, ambiguous) = match outcome {
        IdempotentLaunch::Launched(output) => (output, "dispatch_started", false),
        IdempotentLaunch::Adopted(output) => (output, "dispatch_handle_adopted", false),
        IdempotentLaunch::Failed(output) => (output, "DISPATCH_START_FAILED", true),
        IdempotentLaunch::NeedsDecision(output) => {
            (output, "DISPATCH_SPAWN_BOUNDARY_AMBIGUOUS", true)
        }
    };
    let mut result = if ambiguous {
        ControlEffectResultV1::needs_decision(product_result)
    } else {
        ControlEffectResultV1::applied(request.intent.action_kind, product_result)?
    };
    result.dispatch_handle = Some(output.handle.clone());
    result.external_identity = Some(format!("dispatch:{}", output.handle));
    if !ambiguous {
        result.cost_microusd = Some(reserved_cost.max(preflight));
        result.next_wake = format!("dispatch_manifest:{}", output.handle);
    }
    Ok(result)
}

fn validate_dispatch_worktree(
    repo_root: &Path,
    task_id: u64,
    attempt: u32,
    exact_input_sha: &str,
) -> anyhow::Result<PathBuf> {
    let worktree = attempt_worktree_path(repo_root, task_id, attempt)?;
    anyhow::ensure!(
        worktree.is_dir(),
        "controlled attempt worktree is unavailable"
    );
    let branch = attempt_branch(task_id, attempt);
    let registered = git(repo_root, ["worktree", "list", "--porcelain"])?;
    anyhow::ensure!(
        worktree_registered_for_branch(&registered, &worktree, &branch),
        "controlled attempt worktree path and branch are no longer registered together"
    );
    anyhow::ensure!(
        git(&worktree, ["rev-parse", "--abbrev-ref", "HEAD"])?.trim() == branch,
        "controlled attempt worktree moved to another branch"
    );
    anyhow::ensure!(
        git(&worktree, ["rev-parse", "HEAD"])?.trim() == exact_input_sha,
        "controlled attempt worktree moved from its exact input"
    );
    anyhow::ensure!(
        git(&worktree, ["status", "--porcelain"])?.trim().is_empty(),
        "controlled attempt worktree changed before dispatch"
    );
    Ok(worktree)
}

pub(super) fn cost_usd_to_microusd(cost: f64) -> anyhow::Result<u64> {
    anyhow::ensure!(cost.is_finite() && cost >= 0.0, "dispatch cost is invalid");
    let microusd = (cost * 1_000_000.0).ceil();
    anyhow::ensure!(
        microusd <= u64::MAX as f64,
        "dispatch cost exceeds its bound"
    );
    Ok(microusd as u64)
}

pub(super) fn dispatch_cost_reservation(
    ledger: &Ledger,
    request: &ControlEffectRequestV1,
    agent: AgentKind,
    cap: u64,
    preflight: u64,
) -> anyhow::Result<(Option<&'static str>, u64)> {
    let policy = &request.manifest.retry_cost_policy;
    let prior = ledger
        .control_receipts(&request.manifest.control_id)?
        .into_iter()
        .filter(|receipt| {
            matches!(
                receipt.action_kind,
                ControlActionKindV1::DispatchTask | ControlActionKindV1::RequestVerification
            ) && (receipt.next_state != ControlStateV1::NeedsDecision
                || receipt.dispatch_handle.is_some())
        })
        .try_fold(0_u64, |sum, receipt| match receipt.cost_microusd {
            Some(cost) => sum
                .checked_add(cost)
                .context("control dispatch cost total overflowed"),
            None if policy.missing_cost_needs_decision => {
                anyhow::bail!("prior controlled dispatch has no cost evidence")
            }
            None => Ok(sum),
        })?;
    if (cap > 0 && preflight > cap)
        || (policy.aggregate_stop_microusd > 0
            && (prior >= policy.aggregate_stop_microusd
                || prior
                    .checked_add(preflight)
                    .is_none_or(|total| total > policy.aggregate_stop_microusd)))
    {
        return Ok((Some("DISPATCH_PREFLIGHT_COST_EXHAUSTED"), 0));
    }
    let aggregate_remaining = if policy.aggregate_stop_microusd > 0 {
        policy.aggregate_stop_microusd.checked_sub(prior)
    } else {
        None
    };
    let reserved = match (cap > 0, aggregate_remaining) {
        (true, Some(remaining)) => cap.min(remaining),
        (true, None) => cap,
        (false, Some(remaining)) => remaining,
        (false, None) => 0,
    };
    if reserved > 0
        && (agent.is_acp() || crate::cmd_conduct::budget_warning_for_agent(agent, true).is_some())
    {
        return Ok((Some("DISPATCH_COST_CAP_UNENFORCEABLE"), 0));
    }
    Ok((None, reserved))
}

fn ensure_non_acp_task_session(
    ledger: &Ledger,
    view: &edda_ledger::TaskView,
    task: &edda_core::guided_execution::ControlTaskV1,
    attempt: u32,
    agent: AgentKind,
    session_id: &str,
) -> anyhow::Result<()> {
    let task_id = task.task_id.context("dispatch task omits Task Rail ID")?;
    let lease = ledger
        .task_lease(task_id)?
        .context("controlled dispatch lease is missing")?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;
    let current = task_actions::find_view(&ledger.task_views()?, task_id)?.clone();
    if current.session_id.is_some() {
        anyhow::ensure!(
            current.session_id.as_deref() == Some(session_id)
                && current.session_agent_kind.as_deref() == Some(agent.as_str())
                && current.session_attempt == Some(attempt)
                && current.session_lease_owner.as_deref() == Some(lease.owner.as_str())
                && current.session_brief_event_id.as_deref()
                    == Some(task.brief.brief_event_id.as_str())
                && current.session_brief_digest.as_deref()
                    == Some(task.brief.content_digest.as_str()),
            "controlled task session belongs to a different dispatch"
        );
        return Ok(());
    }
    anyhow::ensure!(
        view.status == TaskStatus::Running && lease.attempt == attempt,
        "controlled task or lease moved before session binding"
    );
    let event = edda_core::event::new_task_host_session_event_with_execution_brief(
        &ledger.head_branch()?,
        ledger.last_event_hash()?.as_deref(),
        task_id,
        &edda_core::event::ControlledTaskSessionParams {
            agent_kind: agent.as_str(),
            session_id,
            attempt,
            lease_owner: &lease.owner,
            brief_event_id: &task.brief.brief_event_id,
            brief_digest: &task.brief.content_digest,
        },
    )?;
    ledger.append_event(&event)
}

pub(crate) fn with_control_receipt_binding(
    mut prompt: String,
    control_id: &str,
    step_id: &str,
) -> String {
    let binding = serde_json::json!({
        "control_id": control_id,
        "step_id": step_id,
    });
    prompt.push_str(
        "\n\n## Product-owned control receipt binding\nYour WorkReceiptV1 `control_ref` must equal this exact JSON object:\n```json\n",
    );
    prompt.push_str(&serde_json::to_string_pretty(&binding).expect("string-only JSON"));
    prompt.push_str("\n```\nDo not infer, alter, or omit either field.\n");
    prompt
}

fn write_dispatch_prompt(
    repo_root: &Path,
    action_id: &str,
    prompt: &str,
) -> anyhow::Result<PathBuf> {
    let dir = repo_root.join(".edda/control-local/dispatch-prompts");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{action_id}.txt"));
    if path.exists() {
        anyhow::ensure!(
            std::fs::read_to_string(&path)? == prompt,
            "deterministic dispatch prompt conflicts with its accepted brief"
        );
    } else {
        edda_store::write_atomic(&path, prompt.as_bytes())?;
    }
    Ok(path)
}

fn parse_agent(value: &str) -> anyhow::Result<AgentKind> {
    match value {
        "claude" => Ok(AgentKind::Claude),
        "pi" => Ok(AgentKind::Pi),
        "codex" => Ok(AgentKind::Codex),
        "acp:grok" => Ok(AgentKind::AcpGrok),
        "acp:kilo" => Ok(AgentKind::AcpKilo),
        "acp:pi" => Ok(AgentKind::AcpPi),
        "acp:claude" => Ok(AgentKind::AcpClaude),
        _ => anyhow::bail!("controlled task agent kind is unsupported"),
    }
}

fn bind_delivery(
    repo_root: &Path,
    ledger: &Ledger,
    request: &ControlEffectRequestV1,
    transport: &ControlledGitHubTransport,
) -> anyhow::Result<ControlEffectResultV1> {
    let control_task = task(request)?;
    let task_id = control_task
        .task_id
        .context("delivery task omits task ID")?;
    let attempt = match request.intent.target {
        ControlTargetV1::PullRequest { .. } => ledger
            .task_views()?
            .into_iter()
            .find(|view| view.task_id == task_id)
            .map(|view| view.attempts)
            .context("delivery task is absent")?,
        _ => target_attempt(request)?,
    };
    let view = task_actions::find_view(&ledger.task_views()?, task_id)?.clone();
    anyhow::ensure!(
        view.status == TaskStatus::Done,
        "work receipt is not available"
    );
    let receipt_bytes = view
        .receipt
        .as_deref()
        .context("task omits WorkReceiptV1")?;
    let structured: WorkReceiptV1 = serde_json::from_str(receipt_bytes)
        .context("task receipt is not structured WorkReceiptV1")?;
    let control_ref = structured
        .control_ref
        .as_ref()
        .context("delivery receipt omits control correlation")?;
    let dispatch_step = ledger
        .control_receipts(&request.manifest.control_id)?
        .into_iter()
        .find(|receipt| {
            receipt.next_state == edda_core::guided_execution::ControlStateV1::WorkersRunning
                && receipt.action_kind == ControlActionKindV1::DispatchTask
                && receipt.step_id == control_ref.step_id
                && matches!(
                    &receipt.target,
                    ControlTargetV1::Task { task_key, attempt: target_attempt }
                        if task_key == &control_task.task_key && *target_attempt == attempt
                )
        })
        .context("delivery has no matching preceding dispatch receipt")?;
    let accepted = ledger.load_execution_brief(
        &control_task.brief.brief_event_id,
        &control_task.brief.content_digest,
    )?;
    let receipt: WorkReceiptV1 = parse_work_receipt(
        receipt_bytes.as_bytes(),
        &accepted.brief,
        ReceiptExpectationV1 {
            control_id: Some(&request.manifest.control_id),
            step_id: Some(&dispatch_step.step_id),
            task_id: Some(task_id),
            attempt: Some(attempt),
            lease_owner: view.session_lease_owner.as_deref(),
            agent_kind: view.session_agent_kind.as_deref(),
            session_id: view.session_id.as_deref(),
        },
    )?;
    validate_delivery(
        repo_root,
        request,
        control_task,
        attempt,
        &receipt,
        transport,
    )?;
    let delivery = receipt.delivery.context("work receipt omits delivery")?;
    let delivery_target = ControlTargetV1::Delivery {
        task_key: control_task.task_key.clone(),
        attempt,
        portable_repo_id: delivery.portable_repo_id.clone(),
        branch: delivery.branch.clone(),
        result_head_sha: delivery.result_head_sha.clone(),
        pr_number: delivery.pr_number,
        pr_base_ref: delivery.pr_base_ref.clone(),
        observed_base_tip_sha: delivery.observed_base_tip_sha.clone(),
    };
    anyhow::ensure!(
        request.intent.target == delivery_target,
        "delivery changed after the token-bound product snapshot"
    );
    let mut result = ControlEffectResultV1::applied(
        request.intent.action_kind,
        if delivery.pr_number.is_some() {
            "delivery_bound_to_repository_and_pr"
        } else {
            "delivery_bound_to_repository"
        },
    )?;
    result.external_identity = Some(match delivery.pr_number {
        Some(pr) => format!("pr:{pr}:{}", delivery.result_head_sha),
        None => format!("branch:{}:{}", delivery.branch, delivery.result_head_sha),
    });
    Ok(result)
}

fn validate_delivery(
    repo_root: &Path,
    request: &ControlEffectRequestV1,
    task: &edda_core::guided_execution::ControlTaskV1,
    attempt: u32,
    receipt: &WorkReceiptV1,
    transport: &ControlledGitHubTransport,
) -> anyhow::Result<()> {
    let delivery = receipt
        .delivery
        .as_ref()
        .context("work receipt omits delivery")?;
    let expected_branch = attempt_branch(task.task_id.context("delivery task omits ID")?, attempt);
    validate_declared_delivery_binding(
        request.manifest.basis.portable_repo_id.as_deref(),
        &task.exact_input_sha,
        &expected_branch,
        delivery,
    )?;
    let branch_head = git(repo_root, ["rev-parse", &delivery.branch])?;
    anyhow::ensure!(
        branch_head.trim() == delivery.result_head_sha,
        "delivery result head differs from its local branch"
    );
    anyhow::ensure!(
        git_success(
            repo_root,
            [
                "merge-base",
                "--is-ancestor",
                &delivery.input_sha,
                &delivery.result_head_sha,
            ],
        )?,
        "delivery result head does not descend from exact attempt input"
    );
    match task.required_delivery {
        DeliveryRequirementV1::PullRequest => {
            let number = delivery.pr_number.context("PR delivery omits PR number")?;
            let repository = github_repository(repo_root, request)?;
            let subject = pr_delivery_subject_for(transport, repo_root, &repository, number)?;
            anyhow::ensure!(subject.state == "OPEN", "delivery PR is not open");
            anyhow::ensure!(
                subject.head_sha == delivery.result_head_sha
                    && subject.head_ref == delivery.branch
                    && delivery.pr_base_ref.as_deref() == Some(subject.base_ref.as_str())
                    && delivery.observed_base_tip_sha.as_deref() == Some(subject.base_sha.as_str()),
                "delivery PR head, branch, or base binding moved"
            );
            if request.manifest.merge_policy.required {
                anyhow::ensure!(
                    request.manifest.merge_policy.pr_number == Some(number)
                        && request.manifest.merge_policy.expected_head_sha.as_deref()
                            == Some(subject.head_sha.as_str())
                        && request.manifest.merge_policy.expected_base_sha.as_deref()
                            == Some(subject.base_sha.as_str()),
                    "delivery conflicts with delegated merge subject"
                );
            }
        }
        DeliveryRequirementV1::LocalOnly => {}
        DeliveryRequirementV1::Commit | DeliveryRequirementV1::Branch => {
            anyhow::ensure!(
                delivery.pr_number.is_none(),
                "non-PR delivery unexpectedly carries a PR"
            );
        }
    }
    Ok(())
}

fn validate_declared_delivery_binding(
    expected_repo: Option<&str>,
    expected_input: &str,
    expected_branch: &str,
    delivery: &DeliveryV1,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        delivery.portable_repo_id.as_deref() == expected_repo,
        "delivery repository identity does not exactly match control manifest"
    );
    anyhow::ensure!(
        delivery.input_sha == expected_input,
        "delivery input SHA differs from manifested attempt input"
    );
    anyhow::ensure!(
        delivery.branch == expected_branch,
        "delivery branch differs from the prepared attempt branch"
    );
    Ok(())
}

fn merge(
    _repo_root: &Path,
    _ledger: &Ledger,
    _request: &ControlEffectRequestV1,
) -> anyhow::Result<ControlEffectResultV1> {
    // GitHub's merge endpoint can atomically match the head, but it exposes no
    // exact-base compare-and-swap. Delegated control therefore never reaches
    // the effect from this provider. The manual/strong `edda review merge`
    // route remains separate and unchanged.
    Ok(ControlEffectResultV1::needs_decision(
        "DELEGATED_MERGE_ATOMIC_BASE_PRECONDITION_UNAVAILABLE",
    ))
}

#[cfg(test)]
#[path = "cmd_control_effects_tests.rs"]
mod tests;
