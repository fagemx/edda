"""GENERATED FILE — do not edit by hand.

Source: pinned event spec registry.json + *.schema.json.
Layer 1 types (stability "stable-v1") are stable; Layer 2 types are experimental.
"""
from __future__ import annotations

from typing import Literal, NotRequired, Required, TypeAlias, TypedDict


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
        'scope': NotRequired[object | None],
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

DeviceRevokePayload = TypedDict(
    'DeviceRevokePayload',
    {
        'device_name': NotRequired[str],
        'revoke_all': NotRequired[bool],
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
        'triggerType': Required[object],
        'eventType': Required[str],
        'sourceLayer': Required[object],
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
        'scope': NotRequired[object | None],
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
        'level': Required[object],
        'detail': Required[str],
    },
    total=False,
)

ReviewBundlePayloadRiskAssessment = TypedDict(
    'ReviewBundlePayloadRiskAssessment',
    {
        'level': Required[object],
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
        'suggested_action': Required[object],
        'suggested_reason': Required[str],
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

TaskDonePayload = TypedDict(
    'TaskDonePayload',
    {
        'task_id': Required[int],
        'receipt': Required[str],
        'evidence_paths': Required[list[str]],
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

TaskSessionPayload = TypedDict(
    'TaskSessionPayload',
    {
        'task_id': Required[int],
        'acp_session_id': NotRequired[str],
        'agent_kind': NotRequired[str],
        'session_id': NotRequired[str],
        'attempt': NotRequired[int],
    },
    total=False,
)

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
        'decision': Required[object],
        'sha': Required[str],
        'comment': NotRequired[str],
        'actor': Required[str],
    },
    total=False,
)

# Stability-partitioned unions (client contract §3).
Layer1Payload: TypeAlias = BranchCreatePayload | BranchSwitchPayload | CheckpointPayload | CmdPayload | CommitPayload | DecisionImportPayload | DecisionRatifyPayload | MergePayload | NotePayload | RebuildPayload
Layer2Payload: TypeAlias = AgentPhaseChangePayload | ApprovalPayload | ApprovalPolicyMatchPayload | ApprovalRequestPayload | CycleTelemetryPayload | DecideSnapshotPayload | DevicePairPayload | DeviceRevokePayload | ExecutionEventPayload | IngestionPayload | PrPayload | ReviewBundlePayload | TaskCreatedPayload | TaskDonePayload | TaskFailedPayload | TaskRequeuedPayload | TaskSessionPayload | TaskStartedPayload | TaskIntakePayload | VerdictRecordedPayload
