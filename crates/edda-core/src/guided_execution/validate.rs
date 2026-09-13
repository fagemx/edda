use super::types::*;
use crate::canon::canonical_json_bytes;
use crate::hash::sha256_hex;
use crate::secret_guard::redact;
use std::collections::BTreeSet;
use std::path::{Component, Path};

const MAX_SHORT_CHARS: usize = 160;
const MAX_TEXT_CHARS: usize = 4_000;
const MAX_ITEM_CHARS: usize = 1_000;
const MAX_PATH_CHARS: usize = 512;
const MAX_LIST_ITEMS: usize = 64;
const MAX_PROBES: usize = 32;
const MAX_OUTCOMES: usize = 16;
const MAX_ARGV_ITEMS: usize = 64;
const MAX_ARG_CHARS: usize = 1_024;

pub fn validate_execution_brief_raw(bytes: &[u8]) -> anyhow::Result<()> {
    validate_raw_secrets(bytes, "execution brief input")
}

pub fn validate_work_receipt_raw(bytes: &[u8]) -> anyhow::Result<()> {
    validate_raw_secrets(bytes, "work receipt input")
}

fn validate_raw_secrets(bytes: &[u8], label: &str) -> anyhow::Result<()> {
    let text = String::from_utf8_lossy(bytes);
    let (_, hits) = redact(&text);
    if let Some(hit) = hits.first() {
        anyhow::bail!("secret content refused in {label} (kind: {})", hit.kind);
    }
    Ok(())
}

pub fn compile_execution_brief(
    input: ExecutionBriefInputV1,
    event_id: String,
    accepted_by: &str,
) -> anyhow::Result<(ExecutionBriefV1, Vec<u8>)> {
    validate_short(accepted_by, "accepted_by")?;
    scan_serialized(&accepted_by, "accepted_by")?;
    validate_event_id(&event_id)?;
    // The raw JSON is scanned before parse by the CLI, then the decoded value
    // is scanned again before any diagnostic could interpolate attacker data.
    // JSON escapes must not turn a hidden credential into a parser/error leak.
    scan_serialized(&input, "execution brief input")?;
    if matches!(&input.procedure, TrustedProcedureV1::ProductRecipe { .. }) {
        anyhow::bail!("unsupported product recipe; install its exact version before acceptance");
    }
    if let TrustedProcedureV1::ControllerAuthored { authored_by, .. } = &input.procedure {
        if authored_by != accepted_by {
            anyhow::bail!(
                "controller-authored procedure identity does not match the accepted authority principal"
            );
        }
    }
    let mut brief = ExecutionBriefV1 {
        brief_version: input.brief_version,
        brief_id: input.brief_id,
        brief_event_id: event_id,
        content_digest: String::new(),
        task_ref: input.task_ref,
        runtime_profile: input.runtime_profile,
        intent: input.intent,
        objective: input.objective,
        basis: input.basis,
        scope: input.scope,
        read_order: input.read_order,
        known_facts: input.known_facts,
        allowed_decisions: input.allowed_decisions,
        return_for_decision: input.return_for_decision,
        procedure: input.procedure,
        outcome_codes: input.outcome_codes,
        receipt_schema: input.receipt_schema,
    };
    let bytes = canonical_runnable_bytes(&brief)?;
    brief.content_digest = sha256_hex(&bytes);
    validate_execution_brief(&brief)?;
    Ok((brief, bytes))
}

pub fn canonical_runnable_bytes(brief: &ExecutionBriefV1) -> anyhow::Result<Vec<u8>> {
    let mut value = serde_json::to_value(brief)?;
    value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("execution brief must be an object"))?
        .remove("content_digest");
    let bytes = canonical_json_bytes(&value)?;
    if bytes.len() > MAX_EXECUTION_BRIEF_INPUT_BYTES {
        anyhow::bail!("canonical execution brief exceeds its size bound");
    }
    Ok(bytes)
}

pub fn validate_execution_brief(brief: &ExecutionBriefV1) -> anyhow::Result<()> {
    if brief.brief_version != EXECUTION_BRIEF_VERSION {
        anyhow::bail!("unsupported brief_version (expected 1)");
    }
    validate_id(&brief.brief_id, "brief_", "brief_id")?;
    validate_event_id(&brief.brief_event_id)?;
    validate_digest(&brief.content_digest, "content_digest")?;
    if brief.task_ref == Some(0) {
        anyhow::bail!("task_ref must be positive");
    }
    validate_text(&brief.objective, MAX_TEXT_CHARS, "objective")?;
    validate_sha(&brief.basis.base_full_sha, "basis.base_full_sha")?;
    if let Some(repo) = &brief.basis.portable_repo_id {
        validate_prefixed_hex(repo, "repo_", 64, "basis.portable_repo_id")?;
    }
    validate_text_list(
        &brief.basis.issue_spec_refs,
        "basis.issue_spec_refs",
        MAX_LIST_ITEMS,
    )?;
    validate_paths(&brief.scope.allowed_paths, "scope.allowed_paths", true)?;
    if brief.scope.allowed_paths.is_empty() {
        anyhow::bail!("scope.allowed_paths must not be empty");
    }
    validate_paths(&brief.scope.out_of_scope, "scope.out_of_scope", false)?;
    validate_unique_ids(
        brief.read_order.iter().map(|item| item.reference.as_str()),
        "read_order.reference",
    )?;
    for item in &brief.read_order {
        validate_text(&item.reference, MAX_PATH_CHARS, "read_order.reference")?;
        validate_text(&item.purpose, MAX_ITEM_CHARS, "read_order.purpose")?;
    }
    if brief.read_order.len() > MAX_LIST_ITEMS {
        anyhow::bail!("read_order exceeds its item bound");
    }
    if brief.known_facts.len() > MAX_LIST_ITEMS {
        anyhow::bail!("known_facts exceeds its item bound");
    }
    for fact in &brief.known_facts {
        validate_text(&fact.statement, MAX_TEXT_CHARS, "known_facts.statement")?;
        validate_text(
            &fact.provenance_ref,
            MAX_PATH_CHARS,
            "known_facts.provenance_ref",
        )?;
    }
    if brief.allowed_decisions.len() > MAX_PROBES {
        anyhow::bail!("allowed_decisions exceeds its item bound");
    }
    for decision in &brief.allowed_decisions {
        validate_text(
            &decision.decision,
            MAX_ITEM_CHARS,
            "allowed_decisions.decision",
        )?;
        validate_text(
            &decision.boundary,
            MAX_ITEM_CHARS,
            "allowed_decisions.boundary",
        )?;
    }
    validate_text_list(
        &brief.return_for_decision,
        "return_for_decision",
        MAX_PROBES,
    )?;
    validate_procedure(&brief.procedure)?;
    if brief.runtime_profile == RuntimeProfileV1::Flash
        && matches!(
            &brief.procedure,
            TrustedProcedureV1::ControllerAuthored {
                probe_cards,
                implementation_steps,
                ..
            } if probe_cards.is_empty() && implementation_steps.is_empty()
        )
    {
        anyhow::bail!(
            "Flash controller-authored procedure requires probe cards or implementation steps"
        );
    }
    validate_outcomes(&brief.outcome_codes)?;
    validate_receipt_schema(&brief.receipt_schema, brief.task_ref.is_some())?;
    scan_serialized(brief, "execution brief")?;
    let bytes = canonical_runnable_bytes(brief)?;
    if sha256_hex(&bytes) != brief.content_digest {
        anyhow::bail!("execution brief content digest mismatch");
    }
    Ok(())
}

pub fn parse_work_receipt(
    bytes: &[u8],
    brief: &ExecutionBriefV1,
    expected: ReceiptExpectationV1<'_>,
) -> anyhow::Result<WorkReceiptV1> {
    if bytes.len() > MAX_WORK_RECEIPT_INPUT_BYTES {
        anyhow::bail!("work receipt input exceeds its size bound");
    }
    validate_work_receipt_raw(bytes)?;
    let receipt: WorkReceiptV1 = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("invalid WorkReceiptV1 input schema"))?;
    validate_work_receipt(brief, &receipt, expected)?;
    Ok(receipt)
}

pub fn validate_work_receipt(
    brief: &ExecutionBriefV1,
    receipt: &WorkReceiptV1,
    expected: ReceiptExpectationV1<'_>,
) -> anyhow::Result<ResultClassV1> {
    validate_execution_brief(brief)?;
    scan_serialized(receipt, "work receipt")?;
    if receipt.receipt_version != WORK_RECEIPT_VERSION {
        anyhow::bail!("unsupported receipt_version (expected 1)");
    }
    validate_id(&receipt.receipt_id, "receipt_", "receipt_id")?;
    if receipt.brief.brief_id != brief.brief_id
        || receipt.brief.brief_event_id != brief.brief_event_id
        || receipt.brief.content_digest != brief.content_digest
    {
        anyhow::bail!("work receipt brief identity mismatch");
    }
    let derived = brief
        .outcome_codes
        .iter()
        .find(|mapping| mapping.code == receipt.outcome_code)
        .map(|mapping| mapping.result_class)
        .ok_or_else(|| anyhow::anyhow!("work receipt uses an unknown outcome code"))?;
    if receipt.result_class != derived {
        anyhow::bail!("work receipt result_class conflicts with its outcome code");
    }
    validate_control_correlation(receipt.control_ref.as_ref(), expected)?;
    validate_task_correlation(brief, receipt.task_ref.as_ref(), expected)?;
    validate_required_receipt_fields(brief, receipt)?;
    validate_short(&receipt.agent.agent_kind, "agent.agent_kind")?;
    if expected
        .agent_kind
        .is_some_and(|value| value != receipt.agent.agent_kind)
    {
        anyhow::bail!("work receipt agent kind mismatch");
    }
    if receipt.agent.runtime_profile != brief.runtime_profile {
        anyhow::bail!("work receipt runtime profile conflicts with execution brief");
    }
    validate_optional_short(receipt.agent.session_id.as_deref(), "agent.session_id")?;
    if expected.session_id.is_some() && receipt.agent.session_id.as_deref() != expected.session_id {
        anyhow::bail!("work receipt session identity mismatch");
    }
    validate_optional_short(receipt.dispatch_handle.as_deref(), "dispatch_handle")?;
    validate_sha(&receipt.basis_full_sha, "basis_full_sha")?;
    if receipt.basis_full_sha != brief.basis.base_full_sha {
        anyhow::bail!("work receipt basis SHA conflicts with execution brief");
    }
    validate_time_order(&receipt.started_at, &receipt.ended_at)?;
    validate_text_list(&receipt.observations, "observations", MAX_LIST_ITEMS)?;
    validate_command_results(&receipt.commands_run)?;
    validate_text_list(
        &receipt.hypotheses_supported,
        "hypotheses_supported",
        MAX_PROBES,
    )?;
    validate_text_list(
        &receipt.hypotheses_rejected,
        "hypotheses_rejected",
        MAX_PROBES,
    )?;
    validate_paths(&receipt.changed_paths, "changed_paths", false)?;
    validate_changed_paths_in_scope(&receipt.changed_paths, &brief.scope.allowed_paths)?;
    validate_delivery(brief, receipt.delivery.as_ref())?;
    validate_validation_receipts(&receipt.validation_ran, "validation_ran")?;
    validate_validation_receipts(&receipt.validation_read, "validation_read")?;
    validate_text_list(&receipt.unknowns, "unknowns", MAX_PROBES)?;
    validate_text(
        &receipt.recommended_next_action,
        MAX_TEXT_CHARS,
        "recommended_next_action",
    )?;
    Ok(derived)
}

fn validate_procedure(procedure: &TrustedProcedureV1) -> anyhow::Result<()> {
    match procedure {
        TrustedProcedureV1::ControllerAuthored {
            authored_by,
            principles,
            probe_cards,
            implementation_steps,
            validation,
        } => {
            validate_short(authored_by, "procedure.authored_by")?;
            validate_text_list(principles, "procedure.principles", MAX_PROBES)?;
            if probe_cards.len() > MAX_PROBES {
                anyhow::bail!("procedure.probe_cards exceeds its item bound");
            }
            validate_unique_ids(
                probe_cards.iter().map(|card| card.probe_id.as_str()),
                "procedure.probe_cards.probe_id",
            )?;
            for card in probe_cards {
                validate_id(&card.probe_id, "probe_", "probe_id")?;
                for (value, field) in [
                    (&card.claim_to_test, "claim_to_test"),
                    (&card.why_it_matters, "why_it_matters"),
                    (&card.input_or_location, "input_or_location"),
                    (&card.on_unknown, "on_unknown"),
                ] {
                    validate_text(value, MAX_TEXT_CHARS, field)?;
                }
                validate_action(&card.action)?;
                if card.possible_results.is_empty() || card.possible_results.len() > MAX_PROBES {
                    anyhow::bail!("probe possible_results is empty or exceeds its item bound");
                }
                for result in &card.possible_results {
                    validate_text(&result.observed_shape, MAX_ITEM_CHARS, "observed_shape")?;
                    validate_text(&result.interpretation, MAX_ITEM_CHARS, "interpretation")?;
                    validate_text(
                        &result.next_probe_or_return,
                        MAX_ITEM_CHARS,
                        "next_probe_or_return",
                    )?;
                }
                validate_text_list(&card.evidence_required, "evidence_required", MAX_PROBES)?;
            }
            if implementation_steps.len() > MAX_LIST_ITEMS || validation.len() > MAX_PROBES {
                anyhow::bail!("procedure steps or validation exceeds its item bound");
            }
            validate_unique_ids(
                implementation_steps
                    .iter()
                    .map(|step| step.step_id.as_str()),
                "procedure.implementation_steps.step_id",
            )?;
            for step in implementation_steps {
                validate_id(&step.step_id, "step_", "step_id")?;
                validate_text(&step.instruction, MAX_TEXT_CHARS, "step.instruction")?;
                if let Some(action) = &step.action {
                    validate_action(action)?;
                }
            }
            validate_unique_ids(
                validation.iter().map(|check| check.check_id.as_str()),
                "procedure.validation.check_id",
            )?;
            for check in validation {
                validate_id(&check.check_id, "check_", "check_id")?;
                validate_text(&check.expectation, MAX_ITEM_CHARS, "check.expectation")?;
                validate_action(&check.action)?;
                validate_text(
                    &check.evidence_required,
                    MAX_ITEM_CHARS,
                    "check.evidence_required",
                )?;
            }
        }
        TrustedProcedureV1::ProductRecipe {
            recipe_id,
            recipe_version,
            parameters,
        } => {
            validate_id(recipe_id, "recipe_", "recipe_id")?;
            if *recipe_version == 0 {
                anyhow::bail!("recipe_version must be positive");
            }
            if parameters.len() > MAX_PROBES {
                anyhow::bail!("recipe parameters exceed their item bound");
            }
            validate_unique_ids(
                parameters.iter().map(|parameter| parameter.name.as_str()),
                "recipe parameter name",
            )?;
            for parameter in parameters {
                validate_short(&parameter.name, "recipe parameter name")?;
                validate_text(&parameter.value, MAX_ITEM_CHARS, "recipe parameter value")?;
            }
        }
    }
    Ok(())
}

fn validate_action(action: &StructuredActionV1) -> anyhow::Result<()> {
    if action.argv.is_empty() || action.argv.len() > MAX_ARGV_ITEMS {
        anyhow::bail!("structured argv is empty or exceeds its item bound");
    }
    for arg in &action.argv {
        validate_text(arg, MAX_ARG_CHARS, "structured argv item")?;
    }
    if action.tool == BoundToolV1::Process {
        let executable = Path::new(&action.argv[0])
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&action.argv[0])
            .to_ascii_lowercase();
        let executable = executable
            .strip_suffix(".exe")
            .or_else(|| executable.strip_suffix(".com"))
            .unwrap_or(&executable);
        if matches!(
            executable,
            "sh" | "bash"
                | "zsh"
                | "fish"
                | "dash"
                | "ash"
                | "ksh"
                | "csh"
                | "tcsh"
                | "nu"
                | "cmd"
                | "command"
                | "powershell"
                | "pwsh"
        ) {
            anyhow::bail!("shell executables are not accepted as structured process actions");
        }
    }
    Ok(())
}

fn validate_outcomes(outcomes: &[OutcomeCodeV1]) -> anyhow::Result<()> {
    if outcomes.is_empty() || outcomes.len() > MAX_OUTCOMES {
        anyhow::bail!("outcome_codes is empty or exceeds its item bound");
    }
    validate_unique_ids(
        outcomes.iter().map(|outcome| outcome.code.as_str()),
        "outcome code",
    )?;
    for outcome in outcomes {
        if outcome.code.is_empty()
            || outcome.code.len() > 64
            || !outcome.code.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
            })
        {
            anyhow::bail!("outcome code has an invalid closed-code shape");
        }
    }
    Ok(())
}

fn validate_receipt_schema(schema: &ReceiptSchemaV1, task_bound: bool) -> anyhow::Result<()> {
    if schema.receipt_version != WORK_RECEIPT_VERSION {
        anyhow::bail!("receipt_schema has an unsupported version");
    }
    let fields: BTreeSet<_> = schema.required_fields.iter().copied().collect();
    if fields.len() != schema.required_fields.len() {
        anyhow::bail!("receipt_schema contains duplicate required fields");
    }
    for required in [
        ReceiptFieldV1::BriefIdentity,
        ReceiptFieldV1::OutcomeCode,
        ReceiptFieldV1::ChangedPaths,
        ReceiptFieldV1::ValidationRan,
        ReceiptFieldV1::ValidationRead,
        ReceiptFieldV1::RecommendedNextAction,
    ] {
        if !fields.contains(&required) {
            anyhow::bail!("receipt_schema omits a mandatory result field");
        }
    }
    if task_bound && !fields.contains(&ReceiptFieldV1::TaskIdentity) {
        anyhow::bail!("task-bound receipt_schema omits task_identity");
    }
    Ok(())
}

fn validate_required_receipt_fields(
    brief: &ExecutionBriefV1,
    receipt: &WorkReceiptV1,
) -> anyhow::Result<()> {
    let required: BTreeSet<_> = brief
        .receipt_schema
        .required_fields
        .iter()
        .copied()
        .collect();
    if required.contains(&ReceiptFieldV1::TaskIdentity) && receipt.task_ref.is_none() {
        anyhow::bail!("work receipt omits required task identity");
    }
    Ok(())
}

fn validate_control_correlation(
    actual: Option<&ControlReceiptRefV1>,
    expected: ReceiptExpectationV1<'_>,
) -> anyhow::Result<()> {
    if expected.control_id.is_some() || expected.step_id.is_some() {
        let actual =
            actual.ok_or_else(|| anyhow::anyhow!("work receipt omits control identity"))?;
        if expected
            .control_id
            .is_some_and(|value| value != actual.control_id)
            || expected
                .step_id
                .is_some_and(|value| value != actual.step_id)
        {
            anyhow::bail!("work receipt control identity mismatch");
        }
    }
    if let Some(actual) = actual {
        validate_short(&actual.control_id, "control_id")?;
        validate_short(&actual.step_id, "control step_id")?;
    }
    Ok(())
}

fn validate_task_correlation(
    brief: &ExecutionBriefV1,
    actual: Option<&TaskReceiptRefV1>,
    expected: ReceiptExpectationV1<'_>,
) -> anyhow::Result<()> {
    if brief.task_ref.is_some()
        || expected.task_id.is_some()
        || expected.attempt.is_some()
        || expected.lease_owner.is_some()
    {
        let actual = actual.ok_or_else(|| anyhow::anyhow!("work receipt omits task identity"))?;
        if brief.task_ref.is_some_and(|value| value != actual.task_id)
            || expected
                .task_id
                .is_some_and(|value| value != actual.task_id)
            || expected
                .attempt
                .is_some_and(|value| value != actual.attempt)
            || expected
                .lease_owner
                .is_some_and(|value| value != actual.lease_owner)
        {
            anyhow::bail!("work receipt task attempt or lease identity mismatch");
        }
    }
    if let Some(actual) = actual {
        if actual.task_id == 0 {
            anyhow::bail!("work receipt task_id must be positive");
        }
        if actual.attempt == 0 {
            anyhow::bail!("work receipt task attempt must be positive");
        }
        validate_short(&actual.lease_owner, "task lease_owner")?;
    }
    Ok(())
}

fn validate_command_results(results: &[CommandResultV1]) -> anyhow::Result<()> {
    if results.len() > MAX_LIST_ITEMS {
        anyhow::bail!("commands_run exceeds its item bound");
    }
    for result in results {
        validate_action(&result.action)?;
        validate_text(&result.result, MAX_ITEM_CHARS, "command result")?;
        validate_text(
            &result.evidence_handle,
            MAX_PATH_CHARS,
            "command evidence_handle",
        )?;
    }
    Ok(())
}

fn validate_changed_paths_in_scope(paths: &[String], allowed: &[String]) -> anyhow::Result<()> {
    let matchers = allowed
        .iter()
        .map(|pattern| {
            globset::GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map(|glob| glob.compile_matcher())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if paths.iter().any(|path| {
        !allowed.iter().zip(&matchers).any(|(pattern, matcher)| {
            matcher.is_match(path)
                || (!pattern.contains(['*', '?', '[', ']'])
                    && path
                        .strip_prefix(pattern)
                        .is_some_and(|suffix| suffix.starts_with('/')))
        })
    }) {
        anyhow::bail!("work receipt changed path is outside execution brief scope");
    }
    Ok(())
}

fn validate_delivery(
    brief: &ExecutionBriefV1,
    delivery: Option<&DeliveryV1>,
) -> anyhow::Result<()> {
    let Some(delivery) = delivery else {
        return Ok(());
    };
    if let Some(repo) = &delivery.portable_repo_id {
        validate_prefixed_hex(repo, "repo_", 64, "delivery.portable_repo_id")?;
        if brief.basis.portable_repo_id.as_ref() != Some(repo) {
            anyhow::bail!("delivery repository identity conflicts with execution brief");
        }
    }
    validate_sha(&delivery.input_sha, "delivery.input_sha")?;
    validate_sha(&delivery.result_head_sha, "delivery.result_head_sha")?;
    if delivery.input_sha != brief.basis.base_full_sha {
        anyhow::bail!("delivery input SHA conflicts with execution brief basis");
    }
    validate_path(&delivery.branch, "delivery.branch")?;
    match delivery.pr_number {
        Some(_) => {
            let base = delivery
                .pr_base_ref
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("PR delivery omits pr_base_ref"))?;
            validate_path(base, "delivery.pr_base_ref")?;
            validate_sha(
                delivery
                    .observed_base_tip_sha
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("PR delivery omits observed_base_tip_sha"))?,
                "delivery.observed_base_tip_sha",
            )?;
        }
        None if delivery.pr_base_ref.is_some() || delivery.observed_base_tip_sha.is_some() => {
            anyhow::bail!("non-PR delivery contains PR-only identity fields");
        }
        None => {}
    }
    Ok(())
}

fn validate_validation_receipts(values: &[ValidationReceiptV1], field: &str) -> anyhow::Result<()> {
    if values.len() > MAX_LIST_ITEMS {
        anyhow::bail!("{field} exceeds its item bound");
    }
    validate_unique_ids(values.iter().map(|item| item.check_id.as_str()), field)?;
    for item in values {
        validate_id(&item.check_id, "check_", "validation check_id")?;
        validate_text(&item.result, MAX_ITEM_CHARS, "validation result")?;
        validate_text(&item.evidence_handle, MAX_PATH_CHARS, "validation evidence")?;
    }
    Ok(())
}

fn validate_time_order(started: &str, ended: &str) -> anyhow::Result<()> {
    let started =
        time::OffsetDateTime::parse(started, &time::format_description::well_known::Rfc3339)
            .map_err(|_| anyhow::anyhow!("started_at must be RFC 3339"))?;
    let ended = time::OffsetDateTime::parse(ended, &time::format_description::well_known::Rfc3339)
        .map_err(|_| anyhow::anyhow!("ended_at must be RFC 3339"))?;
    if ended < started {
        anyhow::bail!("ended_at precedes started_at");
    }
    Ok(())
}

fn validate_text_list(values: &[String], field: &str, max_items: usize) -> anyhow::Result<()> {
    if values.len() > max_items {
        anyhow::bail!("{field} exceeds its item bound");
    }
    for value in values {
        validate_text(value, MAX_ITEM_CHARS, field)?;
    }
    Ok(())
}

fn validate_paths(values: &[String], field: &str, allow_glob: bool) -> anyhow::Result<()> {
    if values.len() > MAX_LIST_ITEMS {
        anyhow::bail!("{field} exceeds its item bound");
    }
    for value in values {
        if !allow_glob && value.contains(['*', '?', '[', ']']) {
            anyhow::bail!("{field} contains a glob where a concrete path is required");
        }
        validate_path(value, field)?;
    }
    Ok(())
}

fn validate_path(value: &str, field: &str) -> anyhow::Result<()> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_PATH_CHARS
        || value.contains('\\')
        || Path::new(value).is_absolute()
        || value.chars().any(char::is_control)
        || !Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("{field} contains an unsafe or oversized path");
    }
    Ok(())
}

fn validate_short(value: &str, field: &str) -> anyhow::Result<()> {
    validate_text(value, MAX_SHORT_CHARS, field)
}

fn validate_optional_short(value: Option<&str>, field: &str) -> anyhow::Result<()> {
    if let Some(value) = value {
        validate_short(value, field)?;
    }
    Ok(())
}

fn validate_text(value: &str, max_chars: usize, field: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() || value.chars().count() > max_chars || value.contains('\0') {
        anyhow::bail!("{field} is blank, contains NUL, or exceeds its character bound");
    }
    Ok(())
}

fn validate_id(value: &str, prefix: &str, field: &str) -> anyhow::Result<()> {
    let suffix = value
        .strip_prefix(prefix)
        .ok_or_else(|| anyhow::anyhow!("{field} has an invalid identifier shape"))?;
    if suffix.is_empty()
        || suffix.len() > 64
        || !suffix
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    {
        anyhow::bail!("{field} has an invalid identifier shape");
    }
    Ok(())
}

fn validate_event_id(value: &str) -> anyhow::Result<()> {
    validate_id(value, "evt_", "brief_event_id")
}

fn validate_sha(value: &str, field: &str) -> anyhow::Result<()> {
    validate_prefixed_hex(value, "", 40, field)
}

fn validate_digest(value: &str, field: &str) -> anyhow::Result<()> {
    validate_prefixed_hex(value, "", 64, field)
}

fn validate_prefixed_hex(
    value: &str,
    prefix: &str,
    digits: usize,
    field: &str,
) -> anyhow::Result<()> {
    let Some(suffix) = value.strip_prefix(prefix) else {
        anyhow::bail!("{field} has an invalid digest shape");
    };
    if suffix.len() != digits
        || !suffix
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        anyhow::bail!("{field} has an invalid digest shape");
    }
    Ok(())
}

fn validate_unique_ids<'a>(
    values: impl Iterator<Item = &'a str>,
    field: &str,
) -> anyhow::Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            anyhow::bail!("{field} contains a duplicate identity");
        }
    }
    Ok(())
}

fn scan_serialized<T: serde::Serialize>(value: &T, label: &str) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let text = String::from_utf8_lossy(&bytes);
    let (_, hits) = redact(&text);
    if let Some(hit) = hits.first() {
        anyhow::bail!("secret content refused in {label} (kind: {})", hit.kind);
    }
    Ok(())
}
