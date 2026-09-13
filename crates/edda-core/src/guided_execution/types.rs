use serde::{Deserialize, Serialize};

pub const EXECUTION_BRIEF_VERSION: u8 = 1;
pub const EXECUTION_BRIEF_RECORD_VERSION: u8 = 1;
pub const WORK_RECEIPT_VERSION: u8 = 1;
pub const EXECUTION_BRIEF_EVENT_TYPE: &str = "execution_brief";
pub const MAX_EXECUTION_BRIEF_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_WORK_RECEIPT_INPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfileV1 {
    Strong,
    Flash,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionIntentV1 {
    Investigate,
    Fix,
    Implement,
    Refactor,
    Test,
    Document,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBasisV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable_repo_id: Option<String>,
    pub base_full_sha: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issue_spec_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionScopeV1 {
    pub allowed_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub out_of_scope: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FactSourceKindV1 {
    Issue,
    Task,
    Capsule,
    RepositoryText,
    ToolOutput,
    ControllerObservation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KnownFactV1 {
    pub statement: String,
    pub provenance_kind: FactSourceKindV1,
    pub provenance_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReadReferenceV1 {
    pub reference: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AllowedDecisionV1 {
    pub decision: String,
    pub boundary: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoundToolV1 {
    Process,
    ReadFile,
    Search,
}

/// A tool invocation is always represented as argv. There is deliberately no
/// environment map, shell source, script body, diff, or transcript field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StructuredActionV1 {
    pub tool: BoundToolV1,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProbeResultV1 {
    pub observed_shape: String,
    pub interpretation: String,
    pub next_probe_or_return: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProbeCardV1 {
    pub probe_id: String,
    pub claim_to_test: String,
    pub why_it_matters: String,
    pub input_or_location: String,
    pub action: StructuredActionV1,
    pub possible_results: Vec<ProbeResultV1>,
    pub evidence_required: Vec<String>,
    pub on_unknown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImplementationStepV1 {
    pub step_id: String,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<StructuredActionV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ValidationCheckV1 {
    pub check_id: String,
    pub expectation: String,
    pub action: StructuredActionV1,
    pub evidence_required: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum TrustedProcedureV1 {
    ControllerAuthored {
        authored_by: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        principles: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        probe_cards: Vec<ProbeCardV1>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        implementation_steps: Vec<ImplementationStepV1>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        validation: Vec<ValidationCheckV1>,
    },
    /// Product recipes carry only an immutable product identifier and bounded
    /// parameters. They cannot smuggle caller-authored procedure fields.
    ProductRecipe {
        recipe_id: String,
        recipe_version: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parameters: Vec<RecipeParameterV1>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecipeParameterV1 {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultClassV1 {
    Success,
    NeedsDecision,
    Inconclusive,
    Failure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeCodeV1 {
    pub code: String,
    pub result_class: ResultClassV1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptFieldV1 {
    BriefIdentity,
    TaskIdentity,
    OutcomeCode,
    Observations,
    ChangedPaths,
    ValidationRan,
    ValidationRead,
    Unknowns,
    RecommendedNextAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReceiptSchemaV1 {
    pub receipt_version: u8,
    pub required_fields: Vec<ReceiptFieldV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBriefInputV1 {
    pub brief_version: u8,
    pub brief_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_ref: Option<u64>,
    pub runtime_profile: RuntimeProfileV1,
    pub intent: ExecutionIntentV1,
    pub objective: String,
    pub basis: ExecutionBasisV1,
    pub scope: ExecutionScopeV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_order: Vec<ReadReferenceV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_facts: Vec<KnownFactV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_decisions: Vec<AllowedDecisionV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub return_for_decision: Vec<String>,
    pub procedure: TrustedProcedureV1,
    pub outcome_codes: Vec<OutcomeCodeV1>,
    pub receipt_schema: ReceiptSchemaV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBriefV1 {
    pub brief_version: u8,
    pub brief_id: String,
    pub brief_event_id: String,
    pub content_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_ref: Option<u64>,
    pub runtime_profile: RuntimeProfileV1,
    pub intent: ExecutionIntentV1,
    pub objective: String,
    pub basis: ExecutionBasisV1,
    pub scope: ExecutionScopeV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_order: Vec<ReadReferenceV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_facts: Vec<KnownFactV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_decisions: Vec<AllowedDecisionV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub return_for_decision: Vec<String>,
    pub procedure: TrustedProcedureV1,
    pub outcome_codes: Vec<OutcomeCodeV1>,
    pub receipt_schema: ReceiptSchemaV1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BriefTrustV1 {
    LocallyAccepted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBriefAuthorityV1 {
    /// Principal authenticated by the authority carrier, never CLI prose.
    pub principal_id: String,
    /// Authenticated controller session bound by that carrier.
    pub session_id: String,
    /// Immutable event that conferred the capability for this exact input.
    pub authority_event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBriefRecordV1 {
    pub record_version: u8,
    pub trust: BriefTrustV1,
    pub authority: ExecutionBriefAuthorityV1,
    pub brief_event_id: String,
    pub content_digest: String,
    pub canonical_bytes_hex: String,
    pub brief: ExecutionBriefV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BriefIdentityV1 {
    pub brief_id: String,
    pub brief_event_id: String,
    pub content_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlReceiptRefV1 {
    pub control_id: String,
    pub step_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskReceiptRefV1 {
    pub task_id: u64,
    pub attempt: u32,
    pub lease_owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentRuntimeIdentityV1 {
    pub agent_kind: String,
    pub runtime_profile: RuntimeProfileV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommandResultV1 {
    pub action: StructuredActionV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub result: String,
    pub evidence_handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable_repo_id: Option<String>,
    pub input_sha: String,
    pub branch: String,
    pub result_head_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_base_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_base_tip_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ValidationReceiptV1 {
    pub check_id: String,
    pub result: String,
    pub evidence_handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkReceiptV1 {
    pub receipt_version: u8,
    pub receipt_id: String,
    pub brief: BriefIdentityV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_ref: Option<ControlReceiptRefV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_ref: Option<TaskReceiptRefV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_handle: Option<String>,
    pub agent: AgentRuntimeIdentityV1,
    pub basis_full_sha: String,
    pub started_at: String,
    pub ended_at: String,
    pub outcome_code: String,
    pub observations: Vec<String>,
    pub commands_run: Vec<CommandResultV1>,
    pub hypotheses_supported: Vec<String>,
    pub hypotheses_rejected: Vec<String>,
    pub changed_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryV1>,
    pub validation_ran: Vec<ValidationReceiptV1>,
    pub validation_read: Vec<ValidationReceiptV1>,
    pub unknowns: Vec<String>,
    pub recommended_next_action: String,
    pub result_class: ResultClassV1,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReceiptExpectationV1<'a> {
    pub control_id: Option<&'a str>,
    pub step_id: Option<&'a str>,
    pub task_id: Option<u64>,
    pub attempt: Option<u32>,
    pub lease_owner: Option<&'a str>,
    pub agent_kind: Option<&'a str>,
    pub session_id: Option<&'a str>,
}
