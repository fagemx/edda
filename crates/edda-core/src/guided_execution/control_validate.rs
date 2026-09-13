use super::control_types::*;
use super::{BriefIdentityV1, ExecutionBriefV1};
use crate::canon::canonical_json_bytes;
use crate::continuity::validate_portable_repo_id;
use crate::secret_guard::redact;
use std::collections::{BTreeMap, BTreeSet};

const MAX_TEXT: usize = 4_000;
const MAX_ITEM: usize = 1_000;
const MAX_SHORT: usize = 160;
const MAX_PATH: usize = 512;
const MAX_ITEMS: usize = 64;

pub fn validate_control_raw(bytes: &[u8], label: &str) -> anyhow::Result<()> {
    if bytes.len() > MAX_CONTROL_INPUT_BYTES {
        anyhow::bail!("{label} exceeds its size bound");
    }
    let text = String::from_utf8_lossy(bytes);
    if let Some(hit) = redact(&text).1.first() {
        anyhow::bail!("secret content refused in {label} (kind: {})", hit.kind);
    }
    Ok(())
}

pub fn compile_control_manifest(
    input: ControlManifestInputV1,
    briefs: &[ExecutionBriefV1],
    manifest_version: u64,
) -> anyhow::Result<(ControlManifestV1, Vec<u8>)> {
    validate_manifest_input(&input)?;
    if input.command_profile != ControlCommandProfileV1::Strong {
        anyhow::bail!("Flash command profile cannot compile or adjudicate control policy");
    }
    let by_id: BTreeMap<&str, &ExecutionBriefV1> = briefs
        .iter()
        .map(|brief| (brief.brief_id.as_str(), brief))
        .collect();
    if by_id.len() != briefs.len() {
        anyhow::bail!("control compile contains duplicate brief IDs");
    }
    let mut tasks = Vec::with_capacity(input.tasks.len());
    for task in input.tasks {
        let brief = by_id.get(task.brief_id.as_str()).ok_or_else(|| {
            anyhow::anyhow!("control task does not have one compiled brief input")
        })?;
        if brief.basis.portable_repo_id != input.basis.portable_repo_id {
            anyhow::bail!("compiled brief repository identity does not match its control manifest");
        }
        if brief.runtime_profile != task.runtime_profile
            || brief.basis.base_full_sha != task.exact_input_sha
            || brief.task_ref != task.task_id
        {
            anyhow::bail!("compiled brief identity does not match its control task");
        }
        let task_paths: BTreeSet<_> = task.owned_paths.iter().collect();
        let brief_paths: BTreeSet<_> = brief.scope.allowed_paths.iter().collect();
        if task_paths != brief_paths {
            anyhow::bail!("compiled brief scope does not match its control task");
        }
        tasks.push(ControlTaskV1 {
            task_key: task.task_key,
            task_id: task.task_id,
            depends_on: task.depends_on,
            exact_input_sha: task.exact_input_sha,
            brief: BriefIdentityV1 {
                brief_id: brief.brief_id.clone(),
                brief_event_id: brief.brief_event_id.clone(),
                content_digest: brief.content_digest.clone(),
            },
            allowed_outcome_codes: brief
                .outcome_codes
                .iter()
                .map(|outcome| outcome.code.clone())
                .collect(),
            runtime_profile: task.runtime_profile,
            model_target: task.model_target,
            owned_paths: task.owned_paths,
            build_lane: task.build_lane,
            issue_binding: task.issue_binding,
            local_only: task.local_only,
            execution_host_affinity: task.execution_host_affinity,
            required_delivery: task.required_delivery,
        });
    }
    if briefs.len() != tasks.len() {
        anyhow::bail!("every compiled brief must be referenced by exactly one control task");
    }
    let manifest = ControlManifestV1 {
        control_version: input.control_version,
        manifest_version,
        control_id: input.control_id,
        program_id: input.program_id,
        command_profile: input.command_profile,
        goal: input.goal,
        exclusions: input.exclusions,
        basis: input.basis,
        tasks,
        capacity: input.capacity,
        admission_policy: input.admission_policy,
        routes: input.routes,
        retry_cost_policy: input.retry_cost_policy,
        review_policy: input.review_policy,
        merge_policy: input.merge_policy,
        return_for_decision: input.return_for_decision,
        completion_condition: input.completion_condition,
    };
    validate_control_manifest(&manifest)?;
    let bytes = canonical_manifest_bytes(&manifest)?;
    Ok((manifest, bytes))
}

pub fn canonical_manifest_bytes(manifest: &ControlManifestV1) -> anyhow::Result<Vec<u8>> {
    let bytes = canonical_json_bytes(&serde_json::to_value(manifest)?)?;
    if bytes.len() > MAX_CONTROL_INPUT_BYTES {
        anyhow::bail!("canonical control manifest exceeds its size bound");
    }
    Ok(bytes)
}

pub fn validate_control_manifest(manifest: &ControlManifestV1) -> anyhow::Result<()> {
    if manifest.control_version != CONTROL_MANIFEST_VERSION || manifest.manifest_version == 0 {
        anyhow::bail!("unsupported control manifest version");
    }
    validate_id(&manifest.control_id, "control_", "control_id")?;
    validate_id(&manifest.program_id, "program_", "program_id")?;
    if manifest.command_profile != ControlCommandProfileV1::Strong {
        anyhow::bail!("Flash command profile cannot compile or adjudicate control policy");
    }
    validate_text(&manifest.goal, MAX_TEXT, "goal")?;
    validate_list(&manifest.exclusions, MAX_ITEMS, MAX_ITEM, "exclusions")?;
    validate_basis(&manifest.basis)?;
    validate_policy_fields(manifest)?;
    validate_tasks_and_routes(manifest)?;
    let bytes = serde_json::to_vec(manifest)?;
    validate_control_raw(&bytes, "decoded control manifest")
}

fn validate_manifest_input(input: &ControlManifestInputV1) -> anyhow::Result<()> {
    if input.control_version != CONTROL_MANIFEST_VERSION {
        anyhow::bail!("unsupported control_version (expected 1)");
    }
    let value = serde_json::to_vec(input)?;
    validate_control_raw(&value, "decoded control manifest input")
}

fn validate_basis(basis: &ControlBasisV1) -> anyhow::Result<()> {
    validate_sha(&basis.base_full_sha, "basis.base_full_sha")?;
    if let Some(repo) = &basis.portable_repo_id {
        validate_portable_repo_id(repo)?;
    }
    if let Some(repository) = &basis.github_repository {
        validate_canonical_github_repository(repository, "basis.github_repository")?;
    }
    validate_list(&basis.references, MAX_ITEMS, MAX_ITEM, "basis.references")
}

fn validate_policy_fields(manifest: &ControlManifestV1) -> anyhow::Result<()> {
    if manifest.capacity.max_workers == 0 || manifest.capacity.verifier_capacity == 0 {
        anyhow::bail!("control capacity values must be positive");
    }
    validate_list(
        &manifest.admission_policy.allowed_issue_stage_labels,
        MAX_ITEMS,
        MAX_SHORT,
        "admission_policy.allowed_issue_stage_labels",
    )?;
    validate_list(
        &manifest.admission_policy.forbidden_hold_labels,
        MAX_ITEMS,
        MAX_SHORT,
        "admission_policy.forbidden_hold_labels",
    )?;
    validate_text(
        &manifest.admission_policy.claim_identity,
        MAX_SHORT,
        "admission_policy.claim_identity",
    )?;
    for (value, field) in [
        (
            &manifest.review_policy.verifier_identity,
            "review_policy.verifier_identity",
        ),
        (
            &manifest.review_policy.verifier_profile,
            "review_policy.verifier_profile",
        ),
        (
            &manifest.review_policy.frozen_surface_source,
            "review_policy.frozen_surface_source",
        ),
        (
            &manifest.merge_policy.eligibility_product_verb,
            "merge_policy.eligibility_product_verb",
        ),
    ] {
        validate_text(value, MAX_ITEM, field)?;
    }
    if manifest.completion_condition != ControlCompletionConditionV1::LocalPreparationOnly {
        if manifest.basis.portable_repo_id.is_none() || manifest.basis.github_repository.is_none() {
            anyhow::bail!(
                "controlled GitHub workflow requires portable and canonical GitHub repository bindings"
            );
        }
        if !manifest.review_policy.one_live_claim_per_pr_head {
            anyhow::bail!("controlled verification requires one live claim per PR/head");
        }
        validate_canonical_github_login(
            &manifest.review_policy.verifier_identity,
            "review_policy.verifier_identity",
        )?;
        if manifest.tasks.is_empty()
            && (manifest.merge_policy.pr_number.is_none()
                || manifest.merge_policy.expected_head_sha.is_none()
                || manifest.merge_policy.expected_base_sha.is_none())
        {
            anyhow::bail!("taskless controlled verification must bind exact PR, base, and head");
        }
        let (backend, model) = manifest
            .review_policy
            .verifier_profile
            .split_once(':')
            .ok_or_else(|| {
                anyhow::anyhow!("review_policy.verifier_profile must be <pi|claude>:<exact-model>")
            })?;
        if !matches!(backend, "pi" | "claude") || model.is_empty() || model.trim() != model {
            anyhow::bail!("review_policy.verifier_profile has no exact supported dispatch model");
        }
    }
    if manifest.merge_policy.authority_capability_ref.is_some() {
        anyhow::bail!("portable control manifest cannot carry an authority capability reference");
    }
    for (value, field) in [
        (
            &manifest.merge_policy.expected_head_sha,
            "merge_policy.expected_head_sha",
        ),
        (
            &manifest.merge_policy.expected_base_sha,
            "merge_policy.expected_base_sha",
        ),
    ] {
        if let Some(sha) = value {
            validate_sha(sha, field)?;
        }
    }
    if manifest.merge_policy.pr_number == Some(0) {
        anyhow::bail!("merge policy PR number must be positive");
    }
    if manifest.merge_policy.required
        && (manifest.merge_policy.pr_number.is_none()
            || manifest.merge_policy.expected_head_sha.is_none()
            || manifest.merge_policy.expected_base_sha.is_none()
            || manifest.basis.portable_repo_id.is_none())
    {
        anyhow::bail!(
            "required merge policy must bind repository, PR, expected head, and expected base"
        );
    }
    match manifest.completion_condition {
        ControlCompletionConditionV1::LocalPreparationOnly if manifest.merge_policy.required => {
            anyhow::bail!("local preparation cannot claim required merge completion")
        }
        ControlCompletionConditionV1::VerificationSucceeded if manifest.merge_policy.required => {
            anyhow::bail!("verification-only completion cannot claim a required merge")
        }
        ControlCompletionConditionV1::DelegatedMergeSucceeded
            if !manifest.merge_policy.required =>
        {
            anyhow::bail!("delegated merge completion requires a bound merge policy")
        }
        _ => {}
    }
    if manifest.return_for_decision.len() > MAX_ITEMS {
        anyhow::bail!("return_for_decision exceeds its list bound");
    }
    for reason_code in &manifest.return_for_decision {
        validate_outcome(reason_code)?;
    }
    Ok(())
}

fn validate_tasks_and_routes(manifest: &ControlManifestV1) -> anyhow::Result<()> {
    if manifest.tasks.len() > MAX_ITEMS || manifest.routes.len() > MAX_ITEMS {
        anyhow::bail!("control tasks or routes exceed their list bound");
    }
    let mut keys = BTreeSet::new();
    let mut briefs = BTreeSet::new();
    let mut task_ids = BTreeSet::new();
    for task in &manifest.tasks {
        validate_id(&task.task_key, "step_", "tasks.task_key")?;
        if !keys.insert(task.task_key.as_str()) || !briefs.insert(task.brief.brief_id.as_str()) {
            anyhow::bail!("control task keys and brief IDs must be unique");
        }
        if task.task_id == Some(0) {
            anyhow::bail!("control task_id must be positive");
        }
        if task
            .task_id
            .is_some_and(|task_id| !task_ids.insert(task_id))
        {
            anyhow::bail!("control Task Rail IDs must be unique");
        }
        if !task.local_only {
            if manifest.basis.portable_repo_id.is_none()
                || manifest.basis.github_repository.is_none()
            {
                anyhow::bail!(
                    "product-controlled task requires portable and canonical GitHub repository bindings"
                );
            }
            if task.task_id.is_none()
                || !task.issue_binding.as_deref().is_some_and(|binding| {
                    binding
                        .strip_prefix("issue:#")
                        .is_some_and(|number| number.parse::<u64>().is_ok_and(|number| number > 0))
                })
            {
                anyhow::bail!(
                    "product-controlled task must bind a Task Rail ID and issue:#<number>"
                );
            }
            let identity_parts = manifest
                .admission_policy
                .claim_identity
                .split('/')
                .collect::<Vec<_>>();
            if identity_parts.len() != 2
                || identity_parts.iter().any(|part| part.is_empty())
                || manifest
                    .admission_policy
                    .claim_identity
                    .chars()
                    .any(char::is_whitespace)
            {
                anyhow::bail!("product control claim identity must be <machine>/<role>");
            }
            if !manifest.admission_policy.winner_readback_required
                || !manifest.admission_policy.existing_claim_check_required
                || !manifest.admission_policy.delivery_pr_check_required
                || manifest
                    .admission_policy
                    .allowed_issue_stage_labels
                    .is_empty()
                || manifest.admission_policy.forbidden_hold_labels.is_empty()
            {
                anyhow::bail!(
                    "product-controlled task requires PR, claim, winner-reread, stage, and hold admission checks"
                );
            }
        }
        validate_sha(&task.exact_input_sha, "tasks.exact_input_sha")?;
        validate_text(&task.model_target, MAX_SHORT, "tasks.model_target")?;
        validate_text(
            &task.execution_host_affinity,
            MAX_SHORT,
            "tasks.execution_host_affinity",
        )?;
        if let Some(build_lane) = &task.build_lane {
            validate_text(build_lane, MAX_SHORT, "tasks.build_lane")?;
        }
        if let Some(issue_binding) = &task.issue_binding {
            validate_text(issue_binding, MAX_ITEM, "tasks.issue_binding")?;
        }
        validate_list(&task.owned_paths, MAX_ITEMS, MAX_PATH, "tasks.owned_paths")?;
        if task.depends_on.len() > MAX_ITEMS {
            anyhow::bail!("control task dependencies exceed their list bound");
        }
        if task.allowed_outcome_codes.is_empty() || task.allowed_outcome_codes.len() > MAX_ITEMS {
            anyhow::bail!("control task outcome codes exceed their list bound or are empty");
        }
        for outcome in &task.allowed_outcome_codes {
            validate_outcome(outcome)?;
        }
        if task.owned_paths.is_empty() {
            anyhow::bail!("control task must own at least one path");
        }
        if !task.local_only && task.issue_binding.is_none() {
            anyhow::bail!("issue-bound control task must declare its issue binding");
        }
    }
    for task in &manifest.tasks {
        for dependency in &task.depends_on {
            if dependency == &task.task_key || !keys.contains(dependency.as_str()) {
                anyhow::bail!("control task dependency is missing or self-referential");
            }
        }
    }
    ensure_acyclic(&manifest.tasks)?;
    let mut route_keys = BTreeSet::new();
    for route in &manifest.routes {
        if !route_keys.insert((route.task_key.as_str(), route.outcome_code.as_str())) {
            anyhow::bail!("control routes must be unique by task and outcome code");
        }
        let task = manifest
            .tasks
            .iter()
            .find(|task| task.task_key == route.task_key)
            .ok_or_else(|| anyhow::anyhow!("control route names an unknown task"))?;
        validate_outcome(&route.outcome_code)?;
        if !task
            .allowed_outcome_codes
            .iter()
            .any(|outcome| outcome == &route.outcome_code)
        {
            anyhow::bail!("control route outcome is not declared by its compiled brief");
        }
    }
    Ok(())
}

fn ensure_acyclic(tasks: &[ControlTaskV1]) -> anyhow::Result<()> {
    let deps: BTreeMap<&str, Vec<&str>> = tasks
        .iter()
        .map(|task| {
            (
                task.task_key.as_str(),
                task.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    fn visit<'a>(
        key: &'a str,
        deps: &BTreeMap<&'a str, Vec<&'a str>>,
        active: &mut BTreeSet<&'a str>,
        done: &mut BTreeSet<&'a str>,
    ) -> anyhow::Result<()> {
        if done.contains(key) {
            return Ok(());
        }
        if !active.insert(key) {
            anyhow::bail!("control task dependency graph contains a cycle");
        }
        if let Some(children) = deps.get(key) {
            for child in children {
                visit(child, deps, active, done)?;
            }
        }
        active.remove(key);
        done.insert(key);
        Ok(())
    }
    let mut active = BTreeSet::new();
    let mut done = BTreeSet::new();
    for key in deps.keys() {
        visit(key, &deps, &mut active, &mut done)?;
    }
    Ok(())
}

fn validate_id(value: &str, prefix: &str, field: &str) -> anyhow::Result<()> {
    let suffix = value
        .strip_prefix(prefix)
        .ok_or_else(|| anyhow::anyhow!("{field} must start with {prefix}"))?;
    if suffix.is_empty()
        || value.len() > 80
        || !suffix
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    {
        anyhow::bail!("{field} has an invalid identifier shape");
    }
    Ok(())
}

fn validate_sha(value: &str, field: &str) -> anyhow::Result<()> {
    if value.len() != 40
        || !value
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        anyhow::bail!("{field} must be a lowercase full SHA");
    }
    Ok(())
}

/// GitHub resolves logins case-insensitively, but its API emits one canonical
/// login. Controlled review stores that lowercase canonical form and later
/// compares API `user.login` bytes exactly; no case-folding happens at verdict
/// ingestion, where folding could authenticate a different declared identity.
pub fn validate_canonical_github_login(value: &str, field: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 39
            && value == value.to_ascii_lowercase()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !value.starts_with('-')
            && !value.ends_with('-')
            && !value.contains("--"),
        "{field} must be a canonical lowercase GitHub login"
    );
    Ok(())
}

pub fn validate_canonical_github_repository(value: &str, field: &str) -> anyhow::Result<()> {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    anyhow::ensure!(
        parts.next().is_none()
            && !owner.is_empty()
            && !name.is_empty()
            && owner.len() <= 100
            && name.len() <= 100
            && value == value.to_ascii_lowercase()
            && owner.bytes().all(repository_component_byte)
            && name.bytes().all(repository_component_byte),
        "{field} must be canonical lowercase GitHub owner/repo"
    );
    Ok(())
}

pub fn validate_control_review_claim(claim: &ControlReviewClaimV1) -> anyhow::Result<()> {
    anyhow::ensure!(
        claim.claim_version == CONTROL_REVIEW_CLAIM_VERSION,
        "unsupported controlled review claim version"
    );
    validate_id(&claim.control_id, "control_", "review claim control_id")?;
    validate_id(&claim.action_id, "action_", "review claim action_id")?;
    anyhow::ensure!(
        claim.generation > 0,
        "review claim generation must be positive"
    );
    anyhow::ensure!(
        claim.state_version > 0,
        "review claim state version must be positive"
    );
    anyhow::ensure!(claim.pr_number > 0, "review claim PR must be positive");
    validate_sha(&claim.base_sha, "review claim base")?;
    validate_sha(&claim.head_sha, "review claim head")?;
    for (digest, field) in [
        (&claim.manifest_digest, "review claim manifest digest"),
        (&claim.charter_digest, "review claim charter digest"),
    ] {
        anyhow::ensure!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| { byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase() }),
            "{field} must be lowercase SHA-256"
        );
    }
    let blob_digest = claim
        .review_bundle_ref
        .strip_prefix("blob:sha256:")
        .ok_or_else(|| anyhow::anyhow!("review claim bundle ref must be blob:sha256:<digest>"))?;
    anyhow::ensure!(
        blob_digest.len() == 64
            && blob_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "review claim bundle ref must contain lowercase SHA-256"
    );
    validate_canonical_github_repository(&claim.repository, "review claim repository")?;
    validate_text(&claim.claimant, MAX_SHORT, "review claim claimant")?;
    validate_text(
        &claim.claimant_session,
        MAX_SHORT,
        "review claim claimant_session",
    )?;
    validate_canonical_github_login(
        &claim.controller_identity,
        "review claim controller_identity",
    )?;
    validate_canonical_github_login(&claim.verifier_identity, "review claim verifier_identity")?;
    anyhow::ensure!(
        claim.controller_identity != claim.verifier_identity,
        "review claim controller and verifier must differ"
    );
    validate_text(
        &claim.verifier_profile,
        MAX_SHORT,
        "review claim verifier_profile",
    )?;
    validate_text(
        &claim.frozen_surface_source,
        MAX_ITEM,
        "review claim frozen_surface_source",
    )?;
    if !claim.seal.is_empty() {
        anyhow::ensure!(
            claim.seal.len() == 64
                && claim
                    .seal
                    .bytes()
                    .all(|byte| { byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase() }),
            "review claim seal must be lowercase HMAC-SHA256"
        );
    }
    Ok(())
}

fn repository_component_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

fn validate_outcome(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        anyhow::bail!("route outcome code has an invalid shape");
    }
    Ok(())
}

fn validate_text(value: &str, max: usize, field: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() || value.chars().count() > max || value.contains('\0') {
        anyhow::bail!("{field} must contain 1-{max} characters");
    }
    Ok(())
}

fn validate_list(
    values: &[String],
    max_items: usize,
    max_chars: usize,
    field: &str,
) -> anyhow::Result<()> {
    if values.len() > max_items {
        anyhow::bail!("{field} exceeds its list bound");
    }
    for value in values {
        validate_text(value, max_chars, field)?;
    }
    Ok(())
}

pub fn validate_control_adjudication(input: &ControlAdjudicationInputV1) -> anyhow::Result<()> {
    if input.adjudication_version != CONTROL_ADJUDICATION_VERSION {
        anyhow::bail!("unsupported control adjudication version");
    }
    validate_outcome(&input.reason_code)?;
    validate_list(
        &input.evidence,
        MAX_ITEMS,
        MAX_ITEM,
        "adjudication.evidence",
    )?;
    if input.expected_manifest_digest.len() != 64
        || !input
            .expected_manifest_digest
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        anyhow::bail!("expected_manifest_digest must be lowercase SHA-256");
    }
    validate_control_raw(&serde_json::to_vec(input)?, "decoded control adjudication")
}

pub fn next_state_for_action(action: ControlActionKindV1) -> anyhow::Result<ControlStateV1> {
    match action {
        ControlActionKindV1::AdmitAndClaimIssue => Ok(ControlStateV1::Admitting),
        ControlActionKindV1::PrepareAttempt => Ok(ControlStateV1::Dispatching),
        ControlActionKindV1::DispatchTask => Ok(ControlStateV1::WorkersRunning),
        ControlActionKindV1::BindDelivery => Ok(ControlStateV1::DeliveryReady),
        ControlActionKindV1::ClaimVerification => Ok(ControlStateV1::VerificationClaimed),
        ControlActionKindV1::RequestVerification => Ok(ControlStateV1::Verifying),
        ControlActionKindV1::RouteKnownFix => Ok(ControlStateV1::CorrectionReady),
        ControlActionKindV1::MergeDelegated | ControlActionKindV1::Complete => {
            Ok(ControlStateV1::Completed)
        }
        ControlActionKindV1::NeedsDecision => Ok(ControlStateV1::NeedsDecision),
        ControlActionKindV1::WaitForWorkers | ControlActionKindV1::ControlError => {
            anyhow::bail!("observation-only control action cannot be applied")
        }
    }
}
