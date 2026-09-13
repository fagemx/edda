use super::{BriefIdentityV1, ExecutionBriefInputV1, RuntimeProfileV1};
use serde::{Deserialize, Serialize};

pub const CONTROL_COMPILE_INPUT_VERSION: u8 = 1;
pub const CONTROL_MANIFEST_VERSION: u8 = 1;
pub const CONTROL_MANIFEST_RECORD_VERSION: u8 = 1;
pub const CONTROL_ADJUDICATION_VERSION: u8 = 1;
pub const CONTROL_INTENT_VERSION: u8 = 1;
pub const CONTROL_RECEIPT_VERSION: u8 = 1;
pub const CONTROL_REVIEW_CLAIM_VERSION: u8 = 3;
pub const CONTROL_MANIFEST_EVENT_TYPE: &str = "control_manifest";
pub const CONTROL_INTENT_EVENT_TYPE: &str = "control_intent";
pub const CONTROL_RECEIPT_EVENT_TYPE: &str = "control_receipt";
pub const MAX_CONTROL_INPUT_BYTES: usize = 512 * 1024;

/// Command authority is sealed into a local capability. A caller-provided
/// string or environment variable never upgrades a Flash profile to Strong.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlCommandProfileV1 {
    Strong,
    Flash,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlBasisV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable_repo_id: Option<String>,
    /// Canonical lowercase GitHub `owner/repo`, observed and frozen by the
    /// trusted compiler. A configured portable key never substitutes for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_repository: Option<String>,
    pub base_full_sha: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryRequirementV1 {
    LocalOnly,
    Commit,
    Branch,
    PullRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlTaskInputV1 {
    pub task_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    pub exact_input_sha: String,
    pub brief_id: String,
    pub runtime_profile: RuntimeProfileV1,
    pub model_target: String,
    pub owned_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_lane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_binding: Option<String>,
    pub local_only: bool,
    pub execution_host_affinity: String,
    pub required_delivery: DeliveryRequirementV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlTaskV1 {
    pub task_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    pub exact_input_sha: String,
    pub brief: BriefIdentityV1,
    pub allowed_outcome_codes: Vec<String>,
    pub runtime_profile: RuntimeProfileV1,
    pub model_target: String,
    pub owned_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_lane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_binding: Option<String>,
    pub local_only: bool,
    pub execution_host_affinity: String,
    pub required_delivery: DeliveryRequirementV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlCapacityV1 {
    pub max_workers: u16,
    pub verifier_capacity: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlAdmissionPolicyV1 {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_issue_stage_labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden_hold_labels: Vec<String>,
    pub claim_identity: String,
    pub winner_readback_required: bool,
    pub existing_claim_check_required: bool,
    pub delivery_pr_check_required: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlActionKindV1 {
    AdmitAndClaimIssue,
    PrepareAttempt,
    DispatchTask,
    WaitForWorkers,
    BindDelivery,
    ClaimVerification,
    RequestVerification,
    RouteKnownFix,
    MergeDelegated,
    Complete,
    NeedsDecision,
    ControlError,
}

impl ControlActionKindV1 {
    pub fn is_local_s6a(self) -> bool {
        matches!(self, Self::Complete | Self::NeedsDecision)
    }

    /// Actions which may be committed as a durable intent. Wait and error are
    /// observations, not effects, so they never receive an action token.
    pub fn is_applicable(self) -> bool {
        !matches!(self, Self::WaitForWorkers | Self::ControlError)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlRouteV1 {
    pub task_key: String,
    pub outcome_code: String,
    pub next_action: ControlActionKindV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlRetryCostPolicyV1 {
    pub per_action_preflight_cost_microusd: u64,
    pub per_action_incremental_cap_microusd: u64,
    pub aggregate_stop_microusd: u64,
    pub missing_cost_needs_decision: bool,
    pub retry_cap: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlReviewPolicyV1 {
    /// Canonical GitHub login: ASCII lowercase, with no display-name or
    /// machine/role normalization. Controlled verdict ingestion compares the
    /// GitHub API's canonical `user.login` to these bytes exactly.
    pub verifier_identity: String,
    pub verifier_profile: String,
    pub frozen_surface_source: String,
    pub one_live_claim_per_pr_head: bool,
}

/// Host-local, authority-sealed claim for one controlled review generation.
/// The seal never travels as authority: another host has a different root key
/// and cannot validate or supersede this claim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlReviewClaimV1 {
    pub claim_version: u8,
    pub repository: String,
    pub control_id: String,
    pub action_id: String,
    pub manifest_digest: String,
    /// Digest of the canonical complete review-bundle payload.
    pub charter_digest: String,
    /// Content-addressed local blob carrying that exact payload.
    pub review_bundle_ref: String,
    pub generation: u64,
    pub state_version: u64,
    pub pr_number: u64,
    pub base_sha: String,
    pub head_sha: String,
    pub claimant: String,
    pub claimant_session: String,
    pub controller_identity: String,
    pub verifier_identity: String,
    pub verifier_profile: String,
    pub frozen_surface_source: String,
    pub seal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlMergePolicyV1 {
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_base_sha: Option<String>,
    /// Must remain absent in a portable manifest. Delegated merge is S6b+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_capability_ref: Option<String>,
    pub eligibility_product_verb: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlCompletionConditionV1 {
    LocalPreparationOnly,
    VerificationSucceeded,
    DelegatedMergeSucceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlManifestInputV1 {
    pub control_version: u8,
    pub control_id: String,
    pub program_id: String,
    pub command_profile: ControlCommandProfileV1,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclusions: Vec<String>,
    pub basis: ControlBasisV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<ControlTaskInputV1>,
    pub capacity: ControlCapacityV1,
    pub admission_policy: ControlAdmissionPolicyV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<ControlRouteV1>,
    pub retry_cost_policy: ControlRetryCostPolicyV1,
    pub review_policy: ControlReviewPolicyV1,
    pub merge_policy: ControlMergePolicyV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub return_for_decision: Vec<String>,
    pub completion_condition: ControlCompletionConditionV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlCompileInputV1 {
    pub compile_version: u8,
    pub manifest: ControlManifestInputV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brief_inputs: Vec<ExecutionBriefInputV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlManifestV1 {
    pub control_version: u8,
    pub manifest_version: u64,
    pub control_id: String,
    pub program_id: String,
    pub command_profile: ControlCommandProfileV1,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclusions: Vec<String>,
    pub basis: ControlBasisV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<ControlTaskV1>,
    pub capacity: ControlCapacityV1,
    pub admission_policy: ControlAdmissionPolicyV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<ControlRouteV1>,
    pub retry_cost_policy: ControlRetryCostPolicyV1,
    pub review_policy: ControlReviewPolicyV1,
    pub merge_policy: ControlMergePolicyV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub return_for_decision: Vec<String>,
    pub completion_condition: ControlCompletionConditionV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlAuthorityProofV1 {
    pub capability_id: String,
    pub principal_id: String,
    pub session_id: String,
    pub command_profile: ControlCommandProfileV1,
    pub local_project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable_repo_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_repository: Option<String>,
    pub permitted_action: String,
    pub expires_at: String,
    pub seal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlAdjudicationRecordV1 {
    pub adjudication_version: u8,
    pub reason_code: String,
    pub evidence: Vec<String>,
    pub prior_state_version: u64,
    pub prior_manifest_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlManifestRecordV1 {
    pub record_version: u8,
    pub manifest_event_id: String,
    pub manifest_digest: String,
    pub canonical_bytes_hex: String,
    pub manifest: ControlManifestV1,
    pub authority: ControlAuthorityProofV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjudication: Option<ControlAdjudicationRecordV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlAdjudicationInputV1 {
    pub adjudication_version: u8,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    pub expected_manifest_digest: String,
    pub clear_return_for_decision: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlStateV1 {
    Prepared,
    Admitting,
    Dispatching,
    WorkersRunning,
    DeliveryReady,
    VerificationClaimed,
    Verifying,
    MergeReady,
    CorrectionReady,
    NeedsDecision,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlTargetV1 {
    Control {
        control_id: String,
    },
    Task {
        task_key: String,
        attempt: u32,
    },
    Delivery {
        task_key: String,
        attempt: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        portable_repo_id: Option<String>,
        branch: String,
        result_head_sha: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pr_number: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pr_base_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_base_tip_sha: Option<String>,
    },
    PullRequest {
        number: u64,
        head_sha: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlIntentV1 {
    pub intent_version: u8,
    pub control_id: String,
    pub step_id: String,
    pub action_id: String,
    pub observed_state_version: u64,
    /// SHA-256 commitment to the exact opaque token which authorized the intent.
    pub action_token: String,
    pub action_kind: ControlActionKindV1,
    pub target: ControlTargetV1,
    pub created_at: String,
    pub seal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlReceiptV1 {
    pub receipt_version: u8,
    pub control_id: String,
    pub step_id: String,
    pub action_id: String,
    pub intent_event_id: String,
    pub observed_state_version: u64,
    /// Commitment to the exact token presented for this result; must equal the intent.
    pub action_token: String,
    pub action_kind: ControlActionKindV1,
    pub target: ControlTargetV1,
    pub product_result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_microusd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    pub next_state: ControlStateV1,
    pub next_wake: String,
    pub recorded_at: String,
    pub seal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlAvailabilityV1 {
    Available,
    ControlUnavailable,
    NeedsDecision,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlStatusV1 {
    pub control_id: String,
    pub manifest_event_id: String,
    pub manifest_version: u64,
    pub manifest_digest: String,
    pub state_version: u64,
    pub state: ControlStateV1,
    pub authority_valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_receipt_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_intent_event_id: Option<String>,
    pub next_wake: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlNextV1 {
    pub control_id: String,
    pub state_version: u64,
    pub availability: ControlAvailabilityV1,
    pub action_kind: ControlActionKindV1,
    pub target: ControlTargetV1,
    pub action_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub reason_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlApplyV1 {
    pub applied: bool,
    pub stale: bool,
    pub control_id: String,
    pub action_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_event_id: Option<String>,
    pub state_version: u64,
    pub state: ControlStateV1,
    pub result: String,
}
