"""GENERATED FILE — do not edit by hand.

Source: event spec (GH-608) registry.json + *.schema.json
Layer 1 types (stability "stable-v1") are stable; Layer 2 ("unstable")
types are experimental and may change in any release
(docs/reference/client-contract.md §3).
"""

from __future__ import annotations

from typing import TypedDict


class Envelope(TypedDict, total=False):
    """Event envelope (stable). All keys optional at the type level; readers must treat documented-required keys as required at runtime."""

    "event_id": str  # required
    "ts": str  # required
    "type": str  # required
    "branch": str  # required
    "parent_hash": str | None
    "hash": str  # required
    "payload": object  # required
    "refs": {
        "blobs": list[str]
        "events": list[str]
        "provenance": list[{
            "target": str
            "rel": str
            "note": str
        }]
    }
    "schema_version": int  # integer
    "digests": list[{
        "alg": str
        "canon": str
        "value": str
    }]
    "event_family": str | None
    "event_level": str | None

class AgentPhaseChangePayload(TypedDict, total=False):
    """Event type ``agent_phase_change`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "session_id": str
    "label": str
    "from": str
    "to": str
    "issue": int  # integer
    "confidence": float
    "signals": list[str]

class ApprovalPayload(TypedDict, total=False):
    """Event type ``approval`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "draft_id": str
    "draft_sha256": str
    "decision": str
    "actor": str
    "note": str
    "stage_id": str
    "role": str
    "device_id": str

class ApprovalPolicyMatchPayload(TypedDict, total=False):
    """Event type ``approval_policy_match`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "task_id": str
    "step": str
    "matched_rule": str
    "action": str
    "reason": str
    "risk_level": str
    "files_changed": int  # integer

class ApprovalRequestPayload(TypedDict, total=False):
    """Event type ``approval_request`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "draft_id": str
    "draft_sha256": str
    "route_rule_id": str
    "stage_id": str
    "role": str
    "assignees": list[str]
    "reason": str

class BranchCreatePayload(TypedDict, total=False):
    """Event type ``branch_create`` — stability: stable-v1 (source: crates/edda-core/src/event.rs)."""

    "name": str
    "purpose": str
    "from_branch": str
    "from_event_id": str

class BranchSwitchPayload(TypedDict, total=False):
    """Event type ``branch_switch`` — stability: stable-v1 (source: crates/edda-core/src/event.rs)."""

    "from": str
    "to": str

class CheckpointPayload(TypedDict, total=False):
    """Event type ``checkpoint`` — stability: stable-v1 (source: crates/edda-core/src/event.rs)."""

    "role": str
    "tags": list[str]
    "hypotheses": list[str]
    "rejected": list[{
        "hypothesis": str
        "reason": str
    }]
    "open": list[str]
    "next": str

class CmdPayload(TypedDict, total=False):
    """Event type ``cmd`` — stability: stable-v1 (source: crates/edda-core/src/event.rs)."""

    "argv": list[str]
    "cwd": str
    "exit_code": int  # integer
    "duration_ms": int  # integer
    "stdout_blob": str
    "stderr_blob": str
    "source": str
    "session_id": str

class CommitPayload(TypedDict, total=False):
    """Event type ``commit`` — stability: stable-v1 (source: crates/edda-core/src/event.rs)."""

    "title": str
    "purpose": str
    "prev_summary": str
    "contribution": str
    "evidence": list[object]
    "labels": list[str]

class CycleTelemetryPayload(TypedDict, total=False):
    """Event type ``cycle_telemetry`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "cycle_id": str
    "source": str
    "started_at": str
    "total_duration_ms": int  # integer
    "operations": list[{
        "name": str
        "duration_ms": int  # integer
        "token_usage": {
            "input_tokens": int  # integer
            "output_tokens": int  # integer
        } | None
        "status": str | None
    }]
    "cost": {
        "total_usd": float
        "breakdown": list[object]
    } | None
    "tags": list[str]
    "metadata": object

class DecideSnapshotPayload(TypedDict, total=False):
    """Event type ``decide_snapshot`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "context_hash": str
    "engine_version": str
    "schema_version": str
    "redaction_level": str
    "village_id": str
    "cycle_id": str
    "context_blob": str
    "result_blob": str
    "context_inline": object
    "result_inline": object

class DecisionImportPayload(TypedDict, total=False):
    """Event type ``decision_import`` — stability: stable-v1 (source: crates/edda-ledger/src/sync.rs)."""

    "role": str
    "text": str
    "tags": list[str]
    "decision": {
        "key": str
        "value": str
        "reason": str | None
        "scope": object | None
        "authority": str | None
        "affected_paths": list[str] | None
        "tags": list[str] | None
        "review_after": str | None
        "reversibility": str | None
        "village_id": str | None
    }
    "source_project_id": str
    "source_project_name": str
    "source_event_id": str

class DecisionRatifyPayload(TypedDict, total=False):
    """Event type ``decision_ratify`` — stability: stable-v1 (source: crates/edda-core/src/event.rs)."""

    "key": str
    "ratified_by": str
    "note": str

class DevicePairPayload(TypedDict, total=False):
    """Event type ``device_pair`` — stability: unstable (source: crates/edda-cli/src/cmd_pair.rs)."""

    "device_name": str
    "paired_from_ip": str
    "token_hash_prefix": str

class DeviceRevokePayload(TypedDict, total=False):
    """Event type ``device_revoke`` — stability: unstable (source: crates/edda-cli/src/cmd_pair.rs)."""

    "device_name": str
    "revoke_all": bool

class ExecutionEventPayload(TypedDict, total=False):
    """Event type ``execution_event`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "version": str
    "event_id": str
    "event_type": str
    "occurred_at": str
    "trace_id": str | None
    "task_id": str | None
    "step_id": str | None
    "project": str | None
    "runtime": str | None
    "model": str | None
    "actor": object
    "usage": object
    "result": object
    "decision_ref": str | None

class IngestionPayload(TypedDict, total=False):
    """Event type ``ingestion`` — stability: unstable (source: crates/edda-ingestion/src/writer.rs)."""

    "id": str
    "triggerType": object
    "eventType": str
    "sourceLayer": object
    "sourceRefs": list[{
        "layer": str
        "kind": str
        "id": str
        "note": str
    }]
    "summary": str
    "detail": object
    "tags": list[str]
    "createdAt": str

class MergePayload(TypedDict, total=False):
    """Event type ``merge`` — stability: stable-v1 (source: crates/edda-core/src/event.rs)."""

    "src": str
    "dst": str
    "reason": str
    "adopted_commits": list[str]

class NotePayload(TypedDict, total=False):
    """Event type ``note`` — stability: stable-v1 (source: crates/edda-core/src/event.rs)."""

    "role": str
    "text": str
    "tags": list[str]
    "decision": {
        "key": str
        "value": str
        "reason": str | None
        "scope": object | None
        "authority": str | None
        "affected_paths": list[str] | None
        "tags": list[str] | None
        "review_after": str | None
        "reversibility": str | None
        "village_id": str | None
    }
    "source": str
    "session_id": str
    "session_stats": {
        "tool_calls": int  # integer
        "tool_failures": int  # integer
        "user_prompts": int  # integer
        "duration_minutes": int  # integer
        "nudge_count": int  # integer
        "decide_count": int  # integer
        "signal_count": int  # integer
        "input_tokens": int  # integer
        "output_tokens": int  # integer
        "cache_read_tokens": int  # integer
        "cache_creation_tokens": int  # integer
        "files_modified": list[str]
        "failed_commands": list[str]
        "commits_made": list[str]
        "deps_added": list[str]
        "notes": list[str]
        "tasks_snapshot": list[{
            "subject": str
            "status": str
        }]
        "outcome": str
        "activity": str
        "model": str
        "edit_ratio": float
        "search_ratio": float
        "estimated_cost_usd": float | None
        "tool_call_breakdown": dict[str, object]
        "file_edit_counts": list[list[object]]
    }
    "digest_watermark": {
        "offset": int  # integer
        "prefix_hash": str
    }

class PrPayload(TypedDict, total=False):
    """Event type ``pr`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "pr_number": int  # integer
    "pr_status": str
    "review_result": str | None
    "blocker_count": int  # integer
    "time_to_merge_hours": float | None
    "created_at": str
    "merged_at": str | None
    "author": str
    "title": str

class RebuildPayload(TypedDict, total=False):
    """Event type ``rebuild`` — stability: stable-v1 (source: crates/edda-core/src/event.rs)."""

    "scope": str
    "branch": str
    "reason": str

class ReviewBundlePayload(TypedDict, total=False):
    """Event type ``review_bundle`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "bundle_id": str
    "change_summary": {
        "files": list[{
            "path": str
            "added": int  # integer
            "deleted": int  # integer
        }]
        "total_added": int  # integer
        "total_deleted": int  # integer
        "diff_ref": str
    }
    "test_results": {
        "passed": int  # integer
        "failed": int  # integer
        "ignored": int  # integer
        "total": int  # integer
        "failures": list[str]
        "command": str
    }
    "risk_assessment": {
        "level": object
        "factors": list[{
            "signal": str
            "level": object
            "detail": str
        }]
    }
    "suggested_action": object
    "suggested_reason": str

class TaskCreatedPayload(TypedDict, total=False):
    """Event type ``task.created`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "task_id": int  # integer
    "title": str
    "after": list[int  # integer]
    "scope_paths": list[str]
    "assignee": str
    "agent_kind": str
    "plan_id": str
    "work_unit_ref": str
    "brief_ref": str
    "idempotency_key": str

class TaskDonePayload(TypedDict, total=False):
    """Event type ``task.done`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "task_id": int  # integer
    "receipt": str
    "evidence_paths": list[str]

class TaskFailedPayload(TypedDict, total=False):
    """Event type ``task.failed`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "task_id": int  # integer
    "reason": str

class TaskRequeuedPayload(TypedDict, total=False):
    """Event type ``task.requeued`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "task_id": int  # integer
    "attempt": int  # integer

class TaskSessionPayload(TypedDict, total=False):
    """Event type ``task.session`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "task_id": int  # integer
    "acp_session_id": str
    "agent_kind": str
    "session_id": str
    "attempt": int  # integer

class TaskStartedPayload(TypedDict, total=False):
    """Event type ``task.started`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "task_id": int  # integer
    "lease_ttl_s": int  # integer
    "attempt": int  # integer

class TaskIntakePayload(TypedDict, total=False):
    """Event type ``task_intake`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "source": str
    "source_id": str
    "source_url": str
    "title": str
    "intent": str
    "labels": list[str]
    "priority": str
    "constraints": list[str]

class VerdictRecordedPayload(TypedDict, total=False):
    """Event type ``verdict.recorded`` — stability: unstable (source: crates/edda-core/src/event.rs)."""

    "subject": str
    "decision": object
    "sha": str
    "comment": str
    "actor": str

# ── Stability-partitioned unions (contract §3) ──

Layer1Payload = BranchCreatePayload | BranchSwitchPayload | CheckpointPayload | CmdPayload | CommitPayload | DecisionImportPayload | DecisionRatifyPayload | MergePayload | NotePayload | RebuildPayload | object
"""Layer 1 stable event payload union (registry stability "stable-v1")."""

Layer2Payload = AgentPhaseChangePayload | ApprovalPayload | ApprovalPolicyMatchPayload | ApprovalRequestPayload | CycleTelemetryPayload | DecideSnapshotPayload | DevicePairPayload | DeviceRevokePayload | ExecutionEventPayload | IngestionPayload | PrPayload | ReviewBundlePayload | TaskCreatedPayload | TaskDonePayload | TaskFailedPayload | TaskRequeuedPayload | TaskSessionPayload | TaskStartedPayload | TaskIntakePayload | VerdictRecordedPayload | object
"""Layer 2 experimental payload union — may change in any release."""
