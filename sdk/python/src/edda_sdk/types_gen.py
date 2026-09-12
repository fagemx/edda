"""GENERATED FILE — do not edit by hand.

Source: pinned event spec registry.json + *.schema.json.
Layer 1 types (stability "stable-v1") are stable; Layer 2 types are experimental.
"""
from __future__ import annotations

from typing import Literal, NoReturn, NotRequired, Required, TypeAlias, TypedDict


EnvelopeRefsProvenanceItem = TypedDict(
    'EnvelopeRefsProvenanceItem',
    {
        'target': Required[str],
        'rel': Required[str],
        'note': NotRequired[str],
    },
    total=False,
)

EnvelopeRefs = TypedDict(
    'EnvelopeRefs',
    {
        'blobs': NotRequired[list[str]],
        'events': NotRequired[list[str]],
        'provenance': NotRequired[list[EnvelopeRefsProvenanceItem]],
    },
    total=False,
)

EnvelopeDigestsItem = TypedDict(
    'EnvelopeDigestsItem',
    {
        'alg': Required[str],
        'canon': Required[str],
        'value': Required[str],
    },
    total=False,
)

Envelope = TypedDict(
    'Envelope',
    {
        'event_id': Required[str],
        'ts': Required[str],
        'type': Required[str],
        'branch': Required[str],
        'parent_hash': NotRequired[str | None],
        'hash': Required[str],
        'payload': Required[object],
        'refs': NotRequired[EnvelopeRefs],
        'schema_version': NotRequired[int],
        'digests': NotRequired[list[EnvelopeDigestsItem]],
        'event_family': NotRequired[str | None],
        'event_level': NotRequired[str | None],
    },
    total=False,
)

AgentPhaseChangePayload = TypedDict(
    'AgentPhaseChangePayload',
    {
        'session_id': Required[str],
        'label': NotRequired[str],
        'from': Required[str],
        'to': Required[str],
        'issue': NotRequired[int],
        'confidence': Required[float],
        'signals': Required[list[str]],
    },
    total=False,
)

ApprovalPayload = TypedDict(
    'ApprovalPayload',
    {
        'draft_id': Required[str],
        'draft_sha256': Required[str],
        'decision': Required[str],
        'actor': Required[str],
        'note': Required[str],
        'stage_id': Required[str],
        'role': Required[str],
        'device_id': NotRequired[str],
    },
    total=False,
)

ApprovalPolicyMatchPayload = TypedDict(
    'ApprovalPolicyMatchPayload',
    {
        'task_id': Required[str],
        'step': Required[str],
        'matched_rule': NotRequired[str],
        'action': Required[str],
        'reason': Required[str],
        'risk_level': NotRequired[str],
        'files_changed': NotRequired[int],
    },
    total=False,
)

ApprovalRequestPayload = TypedDict(
    'ApprovalRequestPayload',
    {
        'draft_id': Required[str],
        'draft_sha256': Required[str],
        'route_rule_id': Required[str],
        'stage_id': Required[str],
        'role': Required[str],
        'assignees': Required[list[str]],
        'reason': Required[str],
    },
    total=False,
)

BranchCreatePayload = TypedDict(
    'BranchCreatePayload',
    {
        'name': Required[str],
        'purpose': Required[str],
        'from_branch': Required[str],
        'from_event_id': Required[str],
    },
    total=False,
)

BranchSwitchPayload = TypedDict(
    'BranchSwitchPayload',
    {
        'from': Required[str],
        'to': Required[str],
    },
    total=False,
)

CheckpointPayloadRejectedItem = TypedDict(
    'CheckpointPayloadRejectedItem',
    {
        'hypothesis': Required[str],
        'reason': Required[str],
    },
    total=False,
)

CheckpointPayload = TypedDict(
    'CheckpointPayload',
    {
        'role': Required[str],
        'tags': Required[list[str]],
        'hypotheses': Required[list[str]],
        'rejected': Required[list[CheckpointPayloadRejectedItem]],
        'open': Required[list[str]],
        'next': Required[str],
    },
    total=False,
)

CmdPayload = TypedDict(
    'CmdPayload',
    {
        'argv': Required[list[str]],
        'cwd': Required[str],
        'exit_code': Required[int],
        'duration_ms': Required[int],
        'stdout_blob': Required[str],
        'stderr_blob': Required[str],
        'source': NotRequired[str],
        'session_id': NotRequired[str],
    },
    total=False,
)

CommitPayload = TypedDict(
    'CommitPayload',
    {
        'title': Required[str],
        'purpose': Required[str],
        'prev_summary': Required[str],
        'contribution': Required[str],
        'evidence': Required[list[object]],
        'labels': Required[list[str]],
    },
    total=False,
)

ControlIntentPayloadControlIntentTarget = TypedDict(
    'ControlIntentPayloadControlIntentTarget',
    {
        'kind': Required[Literal['control']],
        'control_id': Required[str],
    },
    total=False,
)

ControlIntentPayloadControlIntentTarget2 = TypedDict(
    'ControlIntentPayloadControlIntentTarget2',
    {
        'kind': Required[Literal['task']],
        'task_key': Required[str],
        'attempt': Required[int],
    },
    total=False,
)

ControlIntentPayloadControlIntentTarget3 = TypedDict(
    'ControlIntentPayloadControlIntentTarget3',
    {
        'kind': Required[Literal['pull_request']],
        'number': Required[int],
        'head_sha': Required[str],
    },
    total=False,
)

ControlIntentPayloadControlIntent = TypedDict(
    'ControlIntentPayloadControlIntent',
    {
        'intent_version': Required[Literal[1]],
        'control_id': Required[str],
        'step_id': Required[str],
        'action_id': Required[str],
        'observed_state_version': Required[int],
        'action_token': Required[str],
        'action_kind': Required[Literal['complete', 'needs_decision']],
        'target': Required[ControlIntentPayloadControlIntentTarget | ControlIntentPayloadControlIntentTarget2 | ControlIntentPayloadControlIntentTarget3],
        'created_at': Required[str],
        'seal': Required[str],
    },
    total=False,
)

ControlIntentPayload = TypedDict(
    'ControlIntentPayload',
    {
        'control_intent': Required[ControlIntentPayloadControlIntent],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifestBasis = TypedDict(
    'ControlManifestPayloadControlManifestManifestBasis',
    {
        'portable_repo_id': NotRequired[str],
        'github_repository': NotRequired[str],
        'base_full_sha': Required[str],
        'references': NotRequired[list[str]],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifestTasksItemBrief = TypedDict(
    'ControlManifestPayloadControlManifestManifestTasksItemBrief',
    {
        'brief_id': Required[str],
        'brief_event_id': Required[str],
        'content_digest': Required[str],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifestTasksItem = TypedDict(
    'ControlManifestPayloadControlManifestManifestTasksItem',
    {
        'task_key': Required[str],
        'task_id': NotRequired[int],
        'depends_on': NotRequired[list[str]],
        'exact_input_sha': Required[str],
        'brief': Required[ControlManifestPayloadControlManifestManifestTasksItemBrief],
        'allowed_outcome_codes': Required[list[str]],
        'runtime_profile': Required[Literal['strong', 'flash']],
        'model_target': Required[str],
        'owned_paths': Required[list[str]],
        'build_lane': NotRequired[str],
        'issue_binding': NotRequired[str],
        'local_only': Required[bool],
        'execution_host_affinity': Required[str],
        'required_delivery': Required[Literal['local_only', 'commit', 'branch', 'pull_request']],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifestCapacity = TypedDict(
    'ControlManifestPayloadControlManifestManifestCapacity',
    {
        'max_workers': Required[int],
        'verifier_capacity': Required[int],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifestAdmissionPolicy = TypedDict(
    'ControlManifestPayloadControlManifestManifestAdmissionPolicy',
    {
        'allowed_issue_stage_labels': NotRequired[list[str]],
        'forbidden_hold_labels': NotRequired[list[str]],
        'claim_identity': Required[str],
        'winner_readback_required': Required[bool],
        'existing_claim_check_required': Required[bool],
        'delivery_pr_check_required': Required[bool],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifestRoutesItem = TypedDict(
    'ControlManifestPayloadControlManifestManifestRoutesItem',
    {
        'task_key': Required[str],
        'outcome_code': Required[str],
        'next_action': Required[Literal['admit_and_claim_issue', 'prepare_attempt', 'dispatch_task', 'wait_for_workers', 'bind_delivery', 'claim_verification', 'request_verification', 'route_known_fix', 'merge_delegated', 'complete', 'needs_decision', 'control_error']],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifestRetryCostPolicy = TypedDict(
    'ControlManifestPayloadControlManifestManifestRetryCostPolicy',
    {
        'per_action_preflight_cost_microusd': Required[int],
        'per_action_incremental_cap_microusd': Required[int],
        'aggregate_stop_microusd': Required[int],
        'missing_cost_needs_decision': Required[bool],
        'retry_cap': Required[int],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifestReviewPolicy = TypedDict(
    'ControlManifestPayloadControlManifestManifestReviewPolicy',
    {
        'verifier_identity': Required[str],
        'verifier_profile': Required[str],
        'frozen_surface_source': Required[str],
        'one_live_claim_per_pr_head': Required[bool],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifestMergePolicy = TypedDict(
    'ControlManifestPayloadControlManifestManifestMergePolicy',
    {
        'required': Required[bool],
        'pr_number': NotRequired[int],
        'expected_head_sha': NotRequired[str],
        'expected_base_sha': NotRequired[str],
        'eligibility_product_verb': Required[str],
    },
    total=False,
)

ControlManifestPayloadControlManifestManifest = TypedDict(
    'ControlManifestPayloadControlManifestManifest',
    {
        'control_version': Required[Literal[1]],
        'manifest_version': Required[int],
        'control_id': Required[str],
        'program_id': Required[str],
        'command_profile': Required[Literal['strong']],
        'goal': Required[str],
        'exclusions': NotRequired[list[str]],
        'basis': Required[ControlManifestPayloadControlManifestManifestBasis],
        'tasks': NotRequired[list[ControlManifestPayloadControlManifestManifestTasksItem]],
        'capacity': Required[ControlManifestPayloadControlManifestManifestCapacity],
        'admission_policy': Required[ControlManifestPayloadControlManifestManifestAdmissionPolicy],
        'routes': NotRequired[list[ControlManifestPayloadControlManifestManifestRoutesItem]],
        'retry_cost_policy': Required[ControlManifestPayloadControlManifestManifestRetryCostPolicy],
        'review_policy': Required[ControlManifestPayloadControlManifestManifestReviewPolicy],
        'merge_policy': Required[ControlManifestPayloadControlManifestManifestMergePolicy],
        'return_for_decision': NotRequired[list[str]],
        'completion_condition': Required[Literal['local_preparation_only', 'verification_succeeded', 'delegated_merge_succeeded']],
    },
    total=False,
)

ControlManifestPayloadControlManifestAuthority = TypedDict(
    'ControlManifestPayloadControlManifestAuthority',
    {
        'capability_id': Required[str],
        'principal_id': Required[str],
        'session_id': Required[str],
        'command_profile': Required[Literal['strong']],
        'local_project_id': Required[str],
        'portable_repo_id': NotRequired[str],
        'github_repository': NotRequired[str],
        'permitted_action': Required[Literal['control_compile', 'control_adjudicate']],
        'expires_at': Required[str],
        'seal': Required[str],
    },
    total=False,
)

ControlManifestPayloadControlManifestAdjudication = TypedDict(
    'ControlManifestPayloadControlManifestAdjudication',
    {
        'adjudication_version': Required[Literal[1]],
        'reason_code': Required[str],
        'evidence': Required[list[str]],
        'prior_state_version': Required[int],
        'prior_manifest_digest': Required[str],
    },
    total=False,
)

ControlManifestPayloadControlManifest = TypedDict(
    'ControlManifestPayloadControlManifest',
    {
        'record_version': Required[Literal[1]],
        'manifest_event_id': Required[str],
        'manifest_digest': Required[str],
        'canonical_bytes_hex': Required[str],
        'manifest': Required[ControlManifestPayloadControlManifestManifest],
        'authority': Required[ControlManifestPayloadControlManifestAuthority],
        'adjudication': NotRequired[ControlManifestPayloadControlManifestAdjudication],
    },
    total=False,
)

ControlManifestPayload = TypedDict(
    'ControlManifestPayload',
    {
        'control_manifest': Required[ControlManifestPayloadControlManifest],
    },
    total=False,
)

ControlReceiptPayloadControlReceiptTarget = TypedDict(
    'ControlReceiptPayloadControlReceiptTarget',
    {
        'kind': Required[Literal['control']],
        'control_id': Required[str],
    },
    total=False,
)

ControlReceiptPayloadControlReceiptTarget2 = TypedDict(
    'ControlReceiptPayloadControlReceiptTarget2',
    {
        'kind': Required[Literal['task']],
        'task_key': Required[str],
        'attempt': Required[int],
    },
    total=False,
)

ControlReceiptPayloadControlReceiptTarget3 = TypedDict(
    'ControlReceiptPayloadControlReceiptTarget3',
    {
        'kind': Required[Literal['pull_request']],
        'number': Required[int],
        'head_sha': Required[str],
    },
    total=False,
)

ControlReceiptPayloadControlReceipt = TypedDict(
    'ControlReceiptPayloadControlReceipt',
    {
        'receipt_version': Required[Literal[1]],
        'control_id': Required[str],
        'step_id': Required[str],
        'action_id': Required[str],
        'intent_event_id': Required[str],
        'observed_state_version': Required[int],
        'action_token': Required[str],
        'action_kind': Required[Literal['complete', 'needs_decision']],
        'target': Required[ControlReceiptPayloadControlReceiptTarget | ControlReceiptPayloadControlReceiptTarget2 | ControlReceiptPayloadControlReceiptTarget3],
        'product_result': Required[str],
        'dispatch_handle': NotRequired[str],
        'event_ids': NotRequired[list[str]],
        'external_identity': NotRequired[str],
        'cost_microusd': NotRequired[int],
        'elapsed_ms': NotRequired[int],
        'next_state': Required[Literal['needs_decision', 'completed']],
        'next_wake': Required[str],
        'recorded_at': Required[str],
        'seal': Required[str],
    },
    total=False,
)

ControlReceiptPayload = TypedDict(
    'ControlReceiptPayload',
    {
        'control_receipt': Required[ControlReceiptPayloadControlReceipt],
    },
    total=False,
)

ContinuityCapsulePayloadContinuityOrigin = TypedDict(
    'ContinuityCapsulePayloadContinuityOrigin',
    {
        'capsule_id': Required[str],
        'event_id': Required[str],
        'portable_repo_id': NotRequired[str],
    },
    total=False,
)

ContinuityCapsulePayloadContinuityCapsuleState = TypedDict(
    'ContinuityCapsulePayloadContinuityCapsuleState',
    {
        'title': Required[str],
        'summary': Required[str],
        'goal': Required[str],
        'current': Required[str],
        'hypotheses': Required[list[object]],
        'rejected': Required[list[object]],
        'open_questions': Required[list[object]],
        'next_action': Required[str],
    },
    total=False,
)

ContinuityCapsulePayloadContinuityCapsule = TypedDict(
    'ContinuityCapsulePayloadContinuityCapsule',
    {
        'capsule_version': Required[Literal[1]],
        'capsule_id': Required[str],
        'created_at': Required[str],
        'source': Required[dict[str, object]],
        'repository': Required[dict[str, object]],
        'state': Required[ContinuityCapsulePayloadContinuityCapsuleState],
        'git': Required[dict[str, object]],
        'references': Required[dict[str, object]],
        'truncation': NotRequired[list[object]],
    },
    total=False,
)

ContinuityCapsulePayloadContinuity = TypedDict(
    'ContinuityCapsulePayloadContinuity',
    {
        'record_version': Required[Literal[1]],
        'data_authority': Required[Literal['data_only']],
        'origin': Required[ContinuityCapsulePayloadContinuityOrigin],
        'capsule_sha256': Required[str],
        'capsule_bytes_hex': Required[str],
        'capsule': Required[ContinuityCapsulePayloadContinuityCapsule],
        'imported': Required[bool],
    },
    total=False,
)

ContinuityCapsulePayload = TypedDict(
    'ContinuityCapsulePayload',
    {
        'data_authority': Required[Literal['data_only']],
        'continuity': Required[ContinuityCapsulePayloadContinuity],
    },
    total=False,
)

CycleTelemetryPayloadOperationsItemTokenUsage = TypedDict(
    'CycleTelemetryPayloadOperationsItemTokenUsage',
    {
        'input_tokens': Required[int],
        'output_tokens': Required[int],
    },
    total=False,
)

CycleTelemetryPayloadOperationsItem = TypedDict(
    'CycleTelemetryPayloadOperationsItem',
    {
        'name': Required[str],
        'duration_ms': Required[int],
        'token_usage': Required[CycleTelemetryPayloadOperationsItemTokenUsage | None],
        'status': Required[str | None],
    },
    total=False,
)

CycleTelemetryPayloadCost = TypedDict(
    'CycleTelemetryPayloadCost',
    {
        'total_usd': Required[float],
        'breakdown': NotRequired[list[object]],
    },
    total=False,
)

CycleTelemetryPayload = TypedDict(
    'CycleTelemetryPayload',
    {
        'cycle_id': NotRequired[str],
        'source': NotRequired[str],
        'started_at': NotRequired[str],
        'total_duration_ms': NotRequired[int],
        'operations': NotRequired[list[CycleTelemetryPayloadOperationsItem]],
        'cost': NotRequired[CycleTelemetryPayloadCost | None],
        'tags': NotRequired[list[str]],
        'metadata': NotRequired[object],
    },
    total=False,
)

DecideSnapshotPayload = TypedDict(
    'DecideSnapshotPayload',
    {
        'context_hash': Required[str],
        'engine_version': Required[str],
        'schema_version': NotRequired[str],
        'redaction_level': NotRequired[str],
        'village_id': NotRequired[str],
        'cycle_id': NotRequired[str],
        'context_blob': NotRequired[str],
        'result_blob': NotRequired[str],
        'context_inline': NotRequired[object],
        'result_inline': NotRequired[object],
    },
    total=False,
)

DecisionImportPayloadDecision = TypedDict(
    'DecisionImportPayloadDecision',
    {
        'key': Required[str],
        'value': Required[str],
        'reason': NotRequired[str | None],
        'scope': NotRequired[Literal['local', 'shared', 'global'] | None],
        'authority': NotRequired[str | None],
        'affected_paths': NotRequired[list[str] | None],
        'tags': NotRequired[list[str] | None],
        'review_after': NotRequired[str | None],
        'reversibility': NotRequired[str | None],
        'village_id': NotRequired[str | None],
    },
    total=False,
)

DecisionImportPayload = TypedDict(
    'DecisionImportPayload',
    {
        'role': Required[str],
        'text': Required[str],
        'tags': Required[list[str]],
        'decision': Required[DecisionImportPayloadDecision],
        'source_project_id': Required[str],
        'source_project_name': Required[str],
        'source_event_id': Required[str],
    },
    total=False,
)

DecisionRatifyPayload = TypedDict(
    'DecisionRatifyPayload',
    {
        'key': Required[str],
        'ratified_by': Required[str],
        'note': NotRequired[str],
    },
    total=False,
)

DevicePairPayload = TypedDict(
    'DevicePairPayload',
    {
        'device_name': Required[str],
        'paired_from_ip': Required[str],
        'token_hash_prefix': Required[str],
    },
    total=False,
)

DeviceRevokePayloadUncontrolled1 = TypedDict(
    'DeviceRevokePayloadUncontrolled1',
    {
        'device_name': Required[str],
        'revoke_all': NotRequired[bool],
    },
    total=False,
)

DeviceRevokePayloadUncontrolled2 = TypedDict(
    'DeviceRevokePayloadUncontrolled2',
    {
        'device_name': NotRequired[str],
        'revoke_all': Required[bool],
    },
    total=False,
)

DeviceRevokePayload: TypeAlias = DeviceRevokePayloadUncontrolled1 | DeviceRevokePayloadUncontrolled2

ExecutionBriefPayloadExecutionBriefAuthority = TypedDict(
    'ExecutionBriefPayloadExecutionBriefAuthority',
    {
        'principal_id': Required[str],
        'session_id': Required[str],
        'authority_event_id': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileBasis = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileBasis',
    {
        'portable_repo_id': NotRequired[str],
        'base_full_sha': Required[str],
        'issue_spec_refs': NotRequired[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileScope = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileScope',
    {
        'allowed_paths': Required[list[str]],
        'out_of_scope': NotRequired[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileReadOrderItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileReadOrderItem',
    {
        'reference': Required[str],
        'purpose': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileKnownFactsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileKnownFactsItem',
    {
        'statement': Required[str],
        'provenance_kind': Required[Literal['issue', 'task', 'capsule', 'repository_text', 'tool_output', 'controller_observation']],
        'provenance_ref': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileAllowedDecisionsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileAllowedDecisionsItem',
    {
        'decision': Required[str],
        'boundary': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureProbeCardsItemAction = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureProbeCardsItemAction',
    {
        'tool': Required[Literal['process', 'read_file', 'search']],
        'argv': Required[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureProbeCardsItemPossibleResultsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureProbeCardsItemPossibleResultsItem',
    {
        'observed_shape': Required[str],
        'interpretation': Required[str],
        'next_probe_or_return': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureProbeCardsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureProbeCardsItem',
    {
        'probe_id': Required[str],
        'claim_to_test': Required[str],
        'why_it_matters': Required[str],
        'input_or_location': Required[str],
        'action': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureProbeCardsItemAction],
        'possible_results': Required[list[ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureProbeCardsItemPossibleResultsItem]],
        'evidence_required': Required[list[str]],
        'on_unknown': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureImplementationStepsItemAction = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureImplementationStepsItemAction',
    {
        'tool': Required[Literal['process', 'read_file', 'search']],
        'argv': Required[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureImplementationStepsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureImplementationStepsItem',
    {
        'step_id': Required[str],
        'instruction': Required[str],
        'action': NotRequired[ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureImplementationStepsItemAction],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureValidationItemAction = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureValidationItemAction',
    {
        'tool': Required[Literal['process', 'read_file', 'search']],
        'argv': Required[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureValidationItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureValidationItem',
    {
        'check_id': Required[str],
        'expectation': Required[str],
        'action': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureValidationItemAction],
        'evidence_required': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedure = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedure',
    {
        'kind': Required[Literal['controller_authored']],
        'authored_by': Required[str],
        'principles': NotRequired[list[str]],
        'probe_cards': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureProbeCardsItem]],
        'implementation_steps': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureImplementationStepsItem]],
        'validation': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedureValidationItem]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedure2ParametersItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedure2ParametersItem',
    {
        'name': Required[str],
        'value': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedure2 = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedure2',
    {
        'kind': Required[Literal['product_recipe']],
        'recipe_id': Required[str],
        'recipe_version': Required[int],
        'parameters': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedure2ParametersItem]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileOutcomeCodesItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileOutcomeCodesItem',
    {
        'code': Required[str],
        'result_class': Required[Literal['success', 'needs_decision', 'inconclusive', 'failure']],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfileReceiptSchema = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfileReceiptSchema',
    {
        'receipt_version': Required[Literal[1]],
        'required_fields': Required[list[Literal['brief_identity', 'task_identity', 'outcome_code', 'observations', 'changed_paths', 'validation_ran', 'validation_read', 'unknowns', 'recommended_next_action']]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProfile = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProfile',
    {
        'brief_version': Required[Literal[1]],
        'brief_id': Required[str],
        'brief_event_id': Required[str],
        'content_digest': Required[str],
        'task_ref': NotRequired[int],
        'runtime_profile': Required[Literal['strong']],
        'intent': Required[Literal['investigate', 'fix', 'implement', 'refactor', 'test', 'document']],
        'objective': Required[str],
        'basis': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProfileBasis],
        'scope': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProfileScope],
        'read_order': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProfileReadOrderItem]],
        'known_facts': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProfileKnownFactsItem]],
        'allowed_decisions': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProfileAllowedDecisionsItem]],
        'return_for_decision': NotRequired[list[str]],
        'procedure': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedure | ExecutionBriefPayloadExecutionBriefBriefOtherProfileProcedure2],
        'outcome_codes': Required[list[ExecutionBriefPayloadExecutionBriefBriefOtherProfileOutcomeCodesItem]],
        'receipt_schema': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProfileReceiptSchema],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedureBasis = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedureBasis',
    {
        'portable_repo_id': NotRequired[str],
        'base_full_sha': Required[str],
        'issue_spec_refs': NotRequired[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedureScope = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedureScope',
    {
        'allowed_paths': Required[list[str]],
        'out_of_scope': NotRequired[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedureReadOrderItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedureReadOrderItem',
    {
        'reference': Required[str],
        'purpose': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedureKnownFactsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedureKnownFactsItem',
    {
        'statement': Required[str],
        'provenance_kind': Required[Literal['issue', 'task', 'capsule', 'repository_text', 'tool_output', 'controller_observation']],
        'provenance_ref': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedureAllowedDecisionsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedureAllowedDecisionsItem',
    {
        'decision': Required[str],
        'boundary': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedureProcedureParametersItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedureProcedureParametersItem',
    {
        'name': Required[str],
        'value': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedureProcedure = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedureProcedure',
    {
        'kind': Required[Literal['product_recipe']],
        'recipe_id': Required[str],
        'recipe_version': Required[int],
        'parameters': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProcedureProcedureParametersItem]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedureOutcomeCodesItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedureOutcomeCodesItem',
    {
        'code': Required[str],
        'result_class': Required[Literal['success', 'needs_decision', 'inconclusive', 'failure']],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedureReceiptSchema = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedureReceiptSchema',
    {
        'receipt_version': Required[Literal[1]],
        'required_fields': Required[list[Literal['brief_identity', 'task_identity', 'outcome_code', 'observations', 'changed_paths', 'validation_ran', 'validation_read', 'unknowns', 'recommended_next_action']]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefOtherProcedure = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefOtherProcedure',
    {
        'brief_version': Required[Literal[1]],
        'brief_id': Required[str],
        'brief_event_id': Required[str],
        'content_digest': Required[str],
        'task_ref': NotRequired[int],
        'runtime_profile': Required[Literal['flash']],
        'intent': Required[Literal['investigate', 'fix', 'implement', 'refactor', 'test', 'document']],
        'objective': Required[str],
        'basis': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProcedureBasis],
        'scope': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProcedureScope],
        'read_order': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProcedureReadOrderItem]],
        'known_facts': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProcedureKnownFactsItem]],
        'allowed_decisions': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefOtherProcedureAllowedDecisionsItem]],
        'return_for_decision': NotRequired[list[str]],
        'procedure': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProcedureProcedure],
        'outcome_codes': Required[list[ExecutionBriefPayloadExecutionBriefBriefOtherProcedureOutcomeCodesItem]],
        'receipt_schema': Required[ExecutionBriefPayloadExecutionBriefBriefOtherProcedureReceiptSchema],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1Basis = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1Basis',
    {
        'portable_repo_id': NotRequired[str],
        'base_full_sha': Required[str],
        'issue_spec_refs': NotRequired[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1Scope = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1Scope',
    {
        'allowed_paths': Required[list[str]],
        'out_of_scope': NotRequired[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1ReadOrderItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1ReadOrderItem',
    {
        'reference': Required[str],
        'purpose': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1KnownFactsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1KnownFactsItem',
    {
        'statement': Required[str],
        'provenance_kind': Required[Literal['issue', 'task', 'capsule', 'repository_text', 'tool_output', 'controller_observation']],
        'provenance_ref': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1AllowedDecisionsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1AllowedDecisionsItem',
    {
        'decision': Required[str],
        'boundary': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureProbeCardsItemAction = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureProbeCardsItemAction',
    {
        'tool': Required[Literal['process', 'read_file', 'search']],
        'argv': Required[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureProbeCardsItemPossibleResultsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureProbeCardsItemPossibleResultsItem',
    {
        'observed_shape': Required[str],
        'interpretation': Required[str],
        'next_probe_or_return': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureProbeCardsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureProbeCardsItem',
    {
        'probe_id': Required[str],
        'claim_to_test': Required[str],
        'why_it_matters': Required[str],
        'input_or_location': Required[str],
        'action': Required[ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureProbeCardsItemAction],
        'possible_results': Required[list[ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureProbeCardsItemPossibleResultsItem]],
        'evidence_required': Required[list[str]],
        'on_unknown': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureImplementationStepsItemAction = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureImplementationStepsItemAction',
    {
        'tool': Required[Literal['process', 'read_file', 'search']],
        'argv': Required[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureImplementationStepsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureImplementationStepsItem',
    {
        'step_id': Required[str],
        'instruction': Required[str],
        'action': NotRequired[ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureImplementationStepsItemAction],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureValidationItemAction = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureValidationItemAction',
    {
        'tool': Required[Literal['process', 'read_file', 'search']],
        'argv': Required[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureValidationItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureValidationItem',
    {
        'check_id': Required[str],
        'expectation': Required[str],
        'action': Required[ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureValidationItemAction],
        'evidence_required': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1Procedure = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1Procedure',
    {
        'kind': Required[Literal['controller_authored']],
        'authored_by': Required[str],
        'principles': NotRequired[list[str]],
        'probe_cards': Required[list[ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureProbeCardsItem]],
        'implementation_steps': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureImplementationStepsItem]],
        'validation': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional1ProcedureValidationItem]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1OutcomeCodesItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1OutcomeCodesItem',
    {
        'code': Required[str],
        'result_class': Required[Literal['success', 'needs_decision', 'inconclusive', 'failure']],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1ReceiptSchema = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1ReceiptSchema',
    {
        'receipt_version': Required[Literal[1]],
        'required_fields': Required[list[Literal['brief_identity', 'task_identity', 'outcome_code', 'observations', 'changed_paths', 'validation_ran', 'validation_read', 'unknowns', 'recommended_next_action']]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional1 = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional1',
    {
        'brief_version': Required[Literal[1]],
        'brief_id': Required[str],
        'brief_event_id': Required[str],
        'content_digest': Required[str],
        'task_ref': NotRequired[int],
        'runtime_profile': Required[Literal['flash']],
        'intent': Required[Literal['investigate', 'fix', 'implement', 'refactor', 'test', 'document']],
        'objective': Required[str],
        'basis': Required[ExecutionBriefPayloadExecutionBriefBriefConditional1Basis],
        'scope': Required[ExecutionBriefPayloadExecutionBriefBriefConditional1Scope],
        'read_order': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional1ReadOrderItem]],
        'known_facts': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional1KnownFactsItem]],
        'allowed_decisions': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional1AllowedDecisionsItem]],
        'return_for_decision': NotRequired[list[str]],
        'procedure': Required[ExecutionBriefPayloadExecutionBriefBriefConditional1Procedure],
        'outcome_codes': Required[list[ExecutionBriefPayloadExecutionBriefBriefConditional1OutcomeCodesItem]],
        'receipt_schema': Required[ExecutionBriefPayloadExecutionBriefBriefConditional1ReceiptSchema],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2Basis = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2Basis',
    {
        'portable_repo_id': NotRequired[str],
        'base_full_sha': Required[str],
        'issue_spec_refs': NotRequired[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2Scope = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2Scope',
    {
        'allowed_paths': Required[list[str]],
        'out_of_scope': NotRequired[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2ReadOrderItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2ReadOrderItem',
    {
        'reference': Required[str],
        'purpose': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2KnownFactsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2KnownFactsItem',
    {
        'statement': Required[str],
        'provenance_kind': Required[Literal['issue', 'task', 'capsule', 'repository_text', 'tool_output', 'controller_observation']],
        'provenance_ref': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2AllowedDecisionsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2AllowedDecisionsItem',
    {
        'decision': Required[str],
        'boundary': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureProbeCardsItemAction = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureProbeCardsItemAction',
    {
        'tool': Required[Literal['process', 'read_file', 'search']],
        'argv': Required[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureProbeCardsItemPossibleResultsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureProbeCardsItemPossibleResultsItem',
    {
        'observed_shape': Required[str],
        'interpretation': Required[str],
        'next_probe_or_return': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureProbeCardsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureProbeCardsItem',
    {
        'probe_id': Required[str],
        'claim_to_test': Required[str],
        'why_it_matters': Required[str],
        'input_or_location': Required[str],
        'action': Required[ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureProbeCardsItemAction],
        'possible_results': Required[list[ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureProbeCardsItemPossibleResultsItem]],
        'evidence_required': Required[list[str]],
        'on_unknown': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureImplementationStepsItemAction = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureImplementationStepsItemAction',
    {
        'tool': Required[Literal['process', 'read_file', 'search']],
        'argv': Required[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureImplementationStepsItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureImplementationStepsItem',
    {
        'step_id': Required[str],
        'instruction': Required[str],
        'action': NotRequired[ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureImplementationStepsItemAction],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureValidationItemAction = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureValidationItemAction',
    {
        'tool': Required[Literal['process', 'read_file', 'search']],
        'argv': Required[list[str]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureValidationItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureValidationItem',
    {
        'check_id': Required[str],
        'expectation': Required[str],
        'action': Required[ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureValidationItemAction],
        'evidence_required': Required[str],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2Procedure = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2Procedure',
    {
        'kind': Required[Literal['controller_authored']],
        'authored_by': Required[str],
        'principles': NotRequired[list[str]],
        'probe_cards': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureProbeCardsItem]],
        'implementation_steps': Required[list[ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureImplementationStepsItem]],
        'validation': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional2ProcedureValidationItem]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2OutcomeCodesItem = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2OutcomeCodesItem',
    {
        'code': Required[str],
        'result_class': Required[Literal['success', 'needs_decision', 'inconclusive', 'failure']],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2ReceiptSchema = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2ReceiptSchema',
    {
        'receipt_version': Required[Literal[1]],
        'required_fields': Required[list[Literal['brief_identity', 'task_identity', 'outcome_code', 'observations', 'changed_paths', 'validation_ran', 'validation_read', 'unknowns', 'recommended_next_action']]],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBriefConditional2 = TypedDict(
    'ExecutionBriefPayloadExecutionBriefBriefConditional2',
    {
        'brief_version': Required[Literal[1]],
        'brief_id': Required[str],
        'brief_event_id': Required[str],
        'content_digest': Required[str],
        'task_ref': NotRequired[int],
        'runtime_profile': Required[Literal['flash']],
        'intent': Required[Literal['investigate', 'fix', 'implement', 'refactor', 'test', 'document']],
        'objective': Required[str],
        'basis': Required[ExecutionBriefPayloadExecutionBriefBriefConditional2Basis],
        'scope': Required[ExecutionBriefPayloadExecutionBriefBriefConditional2Scope],
        'read_order': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional2ReadOrderItem]],
        'known_facts': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional2KnownFactsItem]],
        'allowed_decisions': NotRequired[list[ExecutionBriefPayloadExecutionBriefBriefConditional2AllowedDecisionsItem]],
        'return_for_decision': NotRequired[list[str]],
        'procedure': Required[ExecutionBriefPayloadExecutionBriefBriefConditional2Procedure],
        'outcome_codes': Required[list[ExecutionBriefPayloadExecutionBriefBriefConditional2OutcomeCodesItem]],
        'receipt_schema': Required[ExecutionBriefPayloadExecutionBriefBriefConditional2ReceiptSchema],
    },
    total=False,
)

ExecutionBriefPayloadExecutionBriefBrief: TypeAlias = ExecutionBriefPayloadExecutionBriefBriefOtherProfile | ExecutionBriefPayloadExecutionBriefBriefOtherProcedure | ExecutionBriefPayloadExecutionBriefBriefConditional1 | ExecutionBriefPayloadExecutionBriefBriefConditional2

ExecutionBriefPayloadExecutionBrief = TypedDict(
    'ExecutionBriefPayloadExecutionBrief',
    {
        'record_version': Required[Literal[1]],
        'trust': Required[Literal['locally_accepted']],
        'authority': Required[ExecutionBriefPayloadExecutionBriefAuthority],
        'brief_event_id': Required[str],
        'content_digest': Required[str],
        'canonical_bytes_hex': Required[str],
        'brief': Required[ExecutionBriefPayloadExecutionBriefBrief],
    },
    total=False,
)

ExecutionBriefPayload = TypedDict(
    'ExecutionBriefPayload',
    {
        'trust': Required[Literal['locally_accepted']],
        'execution_brief': Required[ExecutionBriefPayloadExecutionBrief],
    },
    total=False,
)

ExecutionEventPayload = TypedDict(
    'ExecutionEventPayload',
    {
        'version': NotRequired[str],
        'event_id': NotRequired[str],
        'event_type': NotRequired[str],
        'occurred_at': NotRequired[str],
        'trace_id': NotRequired[str | None],
        'task_id': NotRequired[str | None],
        'step_id': NotRequired[str | None],
        'project': NotRequired[str | None],
        'runtime': NotRequired[str | None],
        'model': NotRequired[str | None],
        'actor': NotRequired[object],
        'usage': NotRequired[object],
        'result': NotRequired[object],
        'decision_ref': NotRequired[str | None],
    },
    total=False,
)

IngestionPayloadSourceRefsItem = TypedDict(
    'IngestionPayloadSourceRefsItem',
    {
        'layer': Required[str],
        'kind': Required[str],
        'id': Required[str],
        'note': NotRequired[str],
    },
    total=False,
)

IngestionPayload = TypedDict(
    'IngestionPayload',
    {
        'id': Required[str],
        'triggerType': Required[Literal['auto', 'suggested', 'manual']],
        'eventType': Required[str],
        'sourceLayer': Required[Literal['L0', 'L1', 'L2', 'L3', 'L4', 'L5']],
        'sourceRefs': NotRequired[list[IngestionPayloadSourceRefsItem]],
        'summary': Required[str],
        'detail': Required[object],
        'tags': NotRequired[list[str]],
        'createdAt': Required[str],
    },
    total=False,
)

MergePayload = TypedDict(
    'MergePayload',
    {
        'src': Required[str],
        'dst': Required[str],
        'reason': Required[str],
        'adopted_commits': Required[list[str]],
    },
    total=False,
)

NotePayloadDecision = TypedDict(
    'NotePayloadDecision',
    {
        'key': Required[str],
        'value': Required[str],
        'reason': NotRequired[str | None],
        'scope': NotRequired[Literal['local', 'shared', 'global'] | None],
        'authority': NotRequired[str | None],
        'affected_paths': NotRequired[list[str] | None],
        'tags': NotRequired[list[str] | None],
        'review_after': NotRequired[str | None],
        'reversibility': NotRequired[str | None],
        'village_id': NotRequired[str | None],
    },
    total=False,
)

NotePayloadSessionStatsTasksSnapshotItem = TypedDict(
    'NotePayloadSessionStatsTasksSnapshotItem',
    {
        'subject': Required[str],
        'status': Required[str],
    },
    total=False,
)

NotePayloadSessionStats = TypedDict(
    'NotePayloadSessionStats',
    {
        'tool_calls': NotRequired[int],
        'tool_failures': NotRequired[int],
        'user_prompts': NotRequired[int],
        'duration_minutes': NotRequired[int],
        'nudge_count': NotRequired[int],
        'decide_count': NotRequired[int],
        'signal_count': NotRequired[int],
        'input_tokens': NotRequired[int],
        'output_tokens': NotRequired[int],
        'cache_read_tokens': NotRequired[int],
        'cache_creation_tokens': NotRequired[int],
        'files_modified': NotRequired[list[str]],
        'failed_commands': NotRequired[list[str]],
        'commits_made': NotRequired[list[str]],
        'deps_added': NotRequired[list[str]],
        'notes': NotRequired[list[str]],
        'tasks_snapshot': NotRequired[list[NotePayloadSessionStatsTasksSnapshotItem]],
        'outcome': NotRequired[str],
        'activity': NotRequired[str],
        'model': NotRequired[str],
        'edit_ratio': NotRequired[float],
        'search_ratio': NotRequired[float],
        'estimated_cost_usd': NotRequired[float | None],
        'tool_call_breakdown': NotRequired[dict[str, object]],
        'file_edit_counts': NotRequired[list[list[object]]],
    },
    total=False,
)

NotePayloadDigestWatermark = TypedDict(
    'NotePayloadDigestWatermark',
    {
        'offset': Required[int],
        'prefix_hash': Required[str],
    },
    total=False,
)

NotePayload = TypedDict(
    'NotePayload',
    {
        'role': Required[str],
        'text': Required[str],
        'tags': Required[list[str]],
        'decision': NotRequired[NotePayloadDecision],
        'source': NotRequired[str],
        'session_id': NotRequired[str],
        'session_stats': NotRequired[NotePayloadSessionStats],
        'digest_watermark': NotRequired[NotePayloadDigestWatermark],
    },
    total=False,
)

PrPayload = TypedDict(
    'PrPayload',
    {
        'pr_number': Required[int],
        'pr_status': Required[str],
        'review_result': Required[str | None],
        'blocker_count': Required[int],
        'time_to_merge_hours': Required[float | None],
        'created_at': Required[str],
        'merged_at': Required[str | None],
        'author': Required[str],
        'title': Required[str],
    },
    total=False,
)

RebuildPayload = TypedDict(
    'RebuildPayload',
    {
        'scope': Required[str],
        'branch': Required[str],
        'reason': Required[str],
    },
    total=False,
)

ReviewBundlePayloadChangeSummaryFilesItem = TypedDict(
    'ReviewBundlePayloadChangeSummaryFilesItem',
    {
        'path': Required[str],
        'added': Required[int],
        'deleted': Required[int],
    },
    total=False,
)

ReviewBundlePayloadChangeSummary = TypedDict(
    'ReviewBundlePayloadChangeSummary',
    {
        'files': Required[list[ReviewBundlePayloadChangeSummaryFilesItem]],
        'total_added': Required[int],
        'total_deleted': Required[int],
        'diff_ref': Required[str],
    },
    total=False,
)

ReviewBundlePayloadTestResults = TypedDict(
    'ReviewBundlePayloadTestResults',
    {
        'passed': Required[int],
        'failed': Required[int],
        'ignored': Required[int],
        'total': Required[int],
        'failures': Required[list[str]],
        'command': Required[str],
    },
    total=False,
)

ReviewBundlePayloadRiskAssessmentFactorsItem = TypedDict(
    'ReviewBundlePayloadRiskAssessmentFactorsItem',
    {
        'signal': Required[str],
        'level': Required[Literal['low', 'medium', 'high', 'critical']],
        'detail': Required[str],
    },
    total=False,
)

ReviewBundlePayloadRiskAssessment = TypedDict(
    'ReviewBundlePayloadRiskAssessment',
    {
        'level': Required[Literal['low', 'medium', 'high', 'critical']],
        'factors': Required[list[ReviewBundlePayloadRiskAssessmentFactorsItem]],
    },
    total=False,
)

ReviewBundlePayload = TypedDict(
    'ReviewBundlePayload',
    {
        'bundle_id': Required[str],
        'change_summary': Required[ReviewBundlePayloadChangeSummary],
        'test_results': Required[ReviewBundlePayloadTestResults],
        'risk_assessment': Required[ReviewBundlePayloadRiskAssessment],
        'suggested_action': Required[Literal['approve', 'review', 'request_changes', 'reject']],
        'suggested_reason': Required[str],
    },
    total=False,
)

ReviewVerdictPayloadSubject = TypedDict(
    'ReviewVerdictPayloadSubject',
    {
        'base_sha': Required[str],
        'head_sha': Required[str],
        'files': Required[int],
        'lines': Required[int],
        'coverage': Required[str],
        'subject_seen': NotRequired[str],
        'worktree_check': NotRequired[str],
    },
    total=False,
)

ReviewVerdictPayloadRefs = TypedDict(
    'ReviewVerdictPayloadRefs',
    {
        'pr': NotRequired[int],
        'issue': NotRequired[int],
        'supersedes': NotRequired[str],
        'previous': NotRequired[str],
        'round': NotRequired[int],
        'history_rewritten': NotRequired[bool],
    },
    total=False,
)

ReviewVerdictPayloadSpec = TypedDict(
    'ReviewVerdictPayloadSpec',
    {
        'mode': Required[str],
        'source': Required[str],
        'trust': Required[str],
    },
    total=False,
)

ReviewVerdictPayloadBrief = TypedDict(
    'ReviewVerdictPayloadBrief',
    {
        'core': Required[str],
        'review_md_sha': NotRequired[str],
        'classes': NotRequired[list[str]],
    },
    total=False,
)

ReviewVerdictPayloadReviewer = TypedDict(
    'ReviewVerdictPayloadReviewer',
    {
        'agent': Required[str],
        'transport': Required[str],
        'model_requested': Required[str],
        'model_observed': Required[str],
        'observed_via': Required[str],
        'model_self_report': NotRequired[str],
        'session_id': Required[str],
        'session_label': Required[str],
        'tool_policy': Required[str],
    },
    total=False,
)

ReviewVerdictPayloadGatesReadItem = TypedDict(
    'ReviewVerdictPayloadGatesReadItem',
    {
        'kind': Required[str],
        'ref': Required[str],
        'cmd': Required[str],
        'result': Required[str],
    },
    total=False,
)

ReviewVerdictPayloadGatesRanItem = TypedDict(
    'ReviewVerdictPayloadGatesRanItem',
    {
        'cmd': Required[str],
        'exit': Required[int],
        'duration_ms': Required[int],
        'stdout_blob': NotRequired[str],
        'timed_out': NotRequired[bool],
    },
    total=False,
)

ReviewVerdictPayloadGates = TypedDict(
    'ReviewVerdictPayloadGates',
    {
        'status': Required[str],
        'declared_by': NotRequired[list[str]],
        'read': NotRequired[list[ReviewVerdictPayloadGatesReadItem]],
        'ran': NotRequired[list[ReviewVerdictPayloadGatesRanItem]],
    },
    total=False,
)

ReviewVerdictPayloadProbesItem = TypedDict(
    'ReviewVerdictPayloadProbesItem',
    {
        'cmd': Required[str],
        'exit': Required[int],
    },
    total=False,
)

ReviewVerdictPayloadFindingsItem = TypedDict(
    'ReviewVerdictPayloadFindingsItem',
    {
        'id': Required[str],
        'severity': Required[str],
        'file': Required[str],
        'line': NotRequired[int],
        'claim': Required[str],
        'evidence': Required[str],
        'rule': Required[str],
        'status': Required[str],
    },
    total=False,
)

ReviewVerdictPayloadChecklistItem = TypedDict(
    'ReviewVerdictPayloadChecklistItem',
    {
        'item': Required[str],
        'result': Required[str],
        'measure': Required[str],
    },
    total=False,
)

ReviewVerdictPayloadCost = TypedDict(
    'ReviewVerdictPayloadCost',
    {
        'usd': NotRequired[float],
        'measured': Required[bool],
        'duration_ms': Required[int],
    },
    total=False,
)

ReviewVerdictPayload = TypedDict(
    'ReviewVerdictPayload',
    {
        'schema': Required[str],
        'subject': Required[ReviewVerdictPayloadSubject],
        'refs': Required[ReviewVerdictPayloadRefs],
        'spec': Required[ReviewVerdictPayloadSpec],
        'brief': Required[ReviewVerdictPayloadBrief],
        'reviewer': Required[ReviewVerdictPayloadReviewer],
        'independence': Required[str],
        'independence_policy': Required[str],
        'gates': Required[ReviewVerdictPayloadGates],
        'probes': NotRequired[list[ReviewVerdictPayloadProbesItem]],
        'verdict': Required[str],
        'outcome': Required[str],
        'qualified': Required[bool],
        'disqualifiers': NotRequired[list[str]],
        'findings': NotRequired[list[ReviewVerdictPayloadFindingsItem]],
        'checklist': NotRequired[list[ReviewVerdictPayloadChecklistItem]],
        'escalations': NotRequired[list[str]],
        'cost': Required[ReviewVerdictPayloadCost],
        'parse': Required[str],
        'notes': NotRequired[str],
    },
    total=False,
)

TaskCreatedPayload = TypedDict(
    'TaskCreatedPayload',
    {
        'task_id': Required[int],
        'title': Required[str],
        'after': Required[list[int]],
        'scope_paths': NotRequired[list[str]],
        'assignee': NotRequired[str],
        'agent_kind': NotRequired[str],
        'plan_id': NotRequired[str],
        'work_unit_ref': NotRequired[str],
        'brief_ref': NotRequired[str],
        'idempotency_key': NotRequired[str],
    },
    total=False,
)

TaskDonePayloadControlledCompletion = TypedDict(
    'TaskDonePayloadControlledCompletion',
    {
        'attempt': Required[int],
        'lease_owner': Required[str],
        'session_id': Required[str],
        'agent_kind': Required[str],
        'brief_event_id': Required[str],
        'brief_digest': Required[str],
        'outcome_code': Required[str],
    },
    total=False,
)

TaskDonePayload = TypedDict(
    'TaskDonePayload',
    {
        'task_id': Required[int],
        'receipt': Required[str],
        'evidence_paths': Required[list[str]],
        'controlled_completion': NotRequired[TaskDonePayloadControlledCompletion],
    },
    total=False,
)

TaskFailedPayload = TypedDict(
    'TaskFailedPayload',
    {
        'task_id': Required[int],
        'reason': Required[str],
    },
    total=False,
)

TaskRequeuedPayload = TypedDict(
    'TaskRequeuedPayload',
    {
        'task_id': Required[int],
        'attempt': Required[int],
    },
    total=False,
)

TaskSessionPayloadUncontrolled1 = TypedDict(
    'TaskSessionPayloadUncontrolled1',
    {
        'task_id': Required[int],
        'acp_session_id': Required[str],
        'agent_kind': NotRequired[str],
        'session_id': NotRequired[str],
        'attempt': NotRequired[int],
        'brief_event_id': NotRequired[NoReturn],
        'brief_digest': NotRequired[NoReturn],
        'lease_owner': NotRequired[NoReturn],
    },
    total=False,
)

TaskSessionPayloadUncontrolled2 = TypedDict(
    'TaskSessionPayloadUncontrolled2',
    {
        'task_id': Required[int],
        'acp_session_id': NotRequired[str],
        'agent_kind': Required[str],
        'session_id': Required[str],
        'attempt': Required[int],
        'brief_event_id': NotRequired[NoReturn],
        'brief_digest': NotRequired[NoReturn],
        'lease_owner': NotRequired[NoReturn],
    },
    total=False,
)

TaskSessionPayloadControlled1 = TypedDict(
    'TaskSessionPayloadControlled1',
    {
        'task_id': Required[int],
        'acp_session_id': NotRequired[str],
        'agent_kind': Required[str],
        'session_id': Required[str],
        'attempt': Required[int],
        'brief_event_id': Required[str],
        'brief_digest': Required[str],
        'lease_owner': Required[str],
    },
    total=False,
)

TaskSessionPayload: TypeAlias = TaskSessionPayloadUncontrolled1 | TaskSessionPayloadUncontrolled2 | TaskSessionPayloadControlled1

TaskStartedPayload = TypedDict(
    'TaskStartedPayload',
    {
        'task_id': Required[int],
        'lease_ttl_s': Required[int],
        'attempt': Required[int],
    },
    total=False,
)

TaskIntakePayload = TypedDict(
    'TaskIntakePayload',
    {
        'source': Required[str],
        'source_id': Required[str],
        'source_url': Required[str],
        'title': Required[str],
        'intent': Required[str],
        'labels': Required[list[str]],
        'priority': Required[str],
        'constraints': Required[list[str]],
    },
    total=False,
)

VerdictRecordedPayload = TypedDict(
    'VerdictRecordedPayload',
    {
        'subject': Required[str],
        'decision': Required[Literal['approved', 'rejected']],
        'sha': Required[str],
        'comment': NotRequired[str],
        'actor': Required[str],
    },
    total=False,
)

# Stability-partitioned unions (client contract §3).
Layer1Payload: TypeAlias = BranchCreatePayload | BranchSwitchPayload | CheckpointPayload | CmdPayload | CommitPayload | ContinuityCapsulePayload | DecisionImportPayload | DecisionRatifyPayload | MergePayload | NotePayload | RebuildPayload
Layer2Payload: TypeAlias = AgentPhaseChangePayload | ApprovalPayload | ApprovalPolicyMatchPayload | ApprovalRequestPayload | ControlIntentPayload | ControlManifestPayload | ControlReceiptPayload | CycleTelemetryPayload | DecideSnapshotPayload | DevicePairPayload | DeviceRevokePayload | ExecutionBriefPayload | ExecutionEventPayload | IngestionPayload | PrPayload | ReviewBundlePayload | ReviewVerdictPayload | TaskCreatedPayload | TaskDonePayload | TaskFailedPayload | TaskRequeuedPayload | TaskSessionPayload | TaskStartedPayload | TaskIntakePayload | VerdictRecordedPayload
