// GENERATED FILE — do not edit by hand.
// Source: event spec (GH-608) registry.json + *.schema.json
// Layer 1 types (stability "stable-v1") are stable; Layer 2 ("unstable")
// types are experimental and may change in any release
// (docs/reference/client-contract.md §3).

// ── Event envelope (stable) ──

export interface Envelope {
  "event_id": string;
  "ts": string;
  "type": string;
  "branch": string;
  "parent_hash"?: string | null;
  "hash": string;
  "payload": unknown;
  "refs"?: {
    "blobs"?: Array<string>;
    "events"?: Array<string>;
    "provenance"?: Array<{
      "target": string;
      "rel": string;
      "note"?: string;
      [k: string]: unknown;
    }>;
    [k: string]: unknown;
  };
  "schema_version"?: number /* integer */;
  "digests"?: Array<{
    "alg": string;
    "canon": string;
    "value": string;
    [k: string]: unknown;
  }>;
  "event_family"?: string | null;
  "event_level"?: string | null;
  [k: string]: unknown;
}

/** Event type `agent_phase_change` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface AgentPhaseChangePayload {
  "session_id": string;
  "label"?: string;
  "from": string;
  "to": string;
  "issue"?: number /* integer */;
  "confidence": number;
  "signals": Array<string>;
  [k: string]: unknown;
}

/** Event type `approval` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface ApprovalPayload {
  "draft_id": string;
  "draft_sha256": string;
  "decision": string;
  "actor": string;
  "note": string;
  "stage_id": string;
  "role": string;
  "device_id"?: string;
  [k: string]: unknown;
}

/** Event type `approval_policy_match` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface ApprovalPolicyMatchPayload {
  "task_id": string;
  "step": string;
  "matched_rule"?: string;
  "action": string;
  "reason": string;
  "risk_level"?: string;
  "files_changed"?: number /* integer */;
  [k: string]: unknown;
}

/** Event type `approval_request` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface ApprovalRequestPayload {
  "draft_id": string;
  "draft_sha256": string;
  "route_rule_id": string;
  "stage_id": string;
  "role": string;
  "assignees": Array<string>;
  "reason": string;
  [k: string]: unknown;
}

/** Event type `branch_create` — stability: stable-v1 (source: crates/edda-core/src/event.rs). */
export interface BranchCreatePayload {
  "name": string;
  "purpose": string;
  "from_branch": string;
  "from_event_id": string;
  [k: string]: unknown;
}

/** Event type `branch_switch` — stability: stable-v1 (source: crates/edda-core/src/event.rs). */
export interface BranchSwitchPayload {
  "from": string;
  "to": string;
  [k: string]: unknown;
}

/** Event type `checkpoint` — stability: stable-v1 (source: crates/edda-core/src/event.rs). */
export interface CheckpointPayload {
  "role": string;
  "tags": Array<string>;
  "hypotheses": Array<string>;
  "rejected": Array<{
    "hypothesis": string;
    "reason": string;
    [k: string]: unknown;
  }>;
  "open": Array<string>;
  "next": string;
  [k: string]: unknown;
}

/** Event type `cmd` — stability: stable-v1 (source: crates/edda-core/src/event.rs). */
export interface CmdPayload {
  "argv": Array<string>;
  "cwd": string;
  "exit_code": number /* integer */;
  "duration_ms": number /* integer */;
  "stdout_blob": string;
  "stderr_blob": string;
  "source"?: string;
  "session_id"?: string;
  [k: string]: unknown;
}

/** Event type `commit` — stability: stable-v1 (source: crates/edda-core/src/event.rs). */
export interface CommitPayload {
  "title": string;
  "purpose": string;
  "prev_summary": string;
  "contribution": string;
  "evidence": Array<unknown>;
  "labels": Array<string>;
  [k: string]: unknown;
}

/** Event type `cycle_telemetry` — stability: unstable (source: crates/edda-core/src/event.rs). */
export type CycleTelemetryPayload = unknown;

/** Event type `decide_snapshot` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface DecideSnapshotPayload {
  "context_hash": string;
  "engine_version": string;
  "schema_version"?: string;
  "redaction_level"?: string;
  "village_id"?: string;
  "cycle_id"?: string;
  "context_blob"?: string;
  "result_blob"?: string;
  "context_inline"?: unknown;
  "result_inline"?: unknown;
  [k: string]: unknown;
}

/** Event type `decision_import` — stability: stable-v1 (source: crates/edda-ledger/src/sync.rs). */
export interface DecisionImportPayload {
  "role": string;
  "text": string;
  "tags": Array<string>;
  "decision": {
    "key": string;
    "value": string;
    "reason"?: string | null;
    "scope"?: "local" | "shared" | "global" | null;
    "authority"?: string | null;
    "affected_paths"?: Array<string> | null;
    "tags"?: Array<string> | null;
    "review_after"?: string | null;
    "reversibility"?: string | null;
    "village_id"?: string | null;
    [k: string]: unknown;
  };
  "source_project_id": string;
  "source_project_name": string;
  "source_event_id": string;
  [k: string]: unknown;
}

/** Event type `decision_ratify` — stability: stable-v1 (source: crates/edda-core/src/event.rs). */
export interface DecisionRatifyPayload {
  "key": string;
  "ratified_by": string;
  "note"?: string;
  [k: string]: unknown;
}

/** Event type `device_pair` — stability: unstable (source: crates/edda-cli/src/cmd_pair.rs). */
export interface DevicePairPayload {
  "device_name": string;
  "paired_from_ip": string;
  "token_hash_prefix": string;
  [k: string]: unknown;
}

/** Event type `device_revoke` — stability: unstable (source: crates/edda-cli/src/cmd_pair.rs). */
export type DeviceRevokePayload = unknown | unknown;

/** Event type `execution_event` — stability: unstable (source: crates/edda-core/src/event.rs). */
export type ExecutionEventPayload = unknown;

/** Event type `ingestion` — stability: unstable (source: crates/edda-ingestion/src/writer.rs). */
export interface IngestionPayload {
  "id": string;
  "triggerType": "auto" | "suggested" | "manual";
  "eventType": string;
  "sourceLayer": "L0" | "L1" | "L2" | "L3" | "L4" | "L5";
  "sourceRefs"?: Array<{
    "layer": string;
    "kind": string;
    "id": string;
    "note"?: string;
    [k: string]: unknown;
  }>;
  "summary": string;
  "detail": unknown;
  "tags"?: Array<string>;
  "createdAt": string;
  [k: string]: unknown;
}

/** Event type `merge` — stability: stable-v1 (source: crates/edda-core/src/event.rs). */
export interface MergePayload {
  "src": string;
  "dst": string;
  "reason": string;
  "adopted_commits": Array<string>;
  [k: string]: unknown;
}

/** Event type `note` — stability: stable-v1 (source: crates/edda-core/src/event.rs). */
export interface NotePayload {
  "role": string;
  "text": string;
  "tags": Array<string>;
  "decision"?: {
    "key": string;
    "value": string;
    "reason"?: string | null;
    "scope"?: "local" | "shared" | "global" | null;
    "authority"?: string | null;
    "affected_paths"?: Array<string> | null;
    "tags"?: Array<string> | null;
    "review_after"?: string | null;
    "reversibility"?: string | null;
    "village_id"?: string | null;
    [k: string]: unknown;
  };
  "source"?: string;
  "session_id"?: string;
  "session_stats"?: {
    "tool_calls"?: number /* integer */;
    "tool_failures"?: number /* integer */;
    "user_prompts"?: number /* integer */;
    "duration_minutes"?: number /* integer */;
    "nudge_count"?: number /* integer */;
    "decide_count"?: number /* integer */;
    "signal_count"?: number /* integer */;
    "input_tokens"?: number /* integer */;
    "output_tokens"?: number /* integer */;
    "cache_read_tokens"?: number /* integer */;
    "cache_creation_tokens"?: number /* integer */;
    "files_modified"?: Array<string>;
    "failed_commands"?: Array<string>;
    "commits_made"?: Array<string>;
    "deps_added"?: Array<string>;
    "notes"?: Array<string>;
    "tasks_snapshot"?: Array<{
      "subject": string;
      "status": string;
      [k: string]: unknown;
    }>;
    "outcome"?: string;
    "activity"?: string;
    "model"?: string;
    "edit_ratio"?: number;
    "search_ratio"?: number;
    "estimated_cost_usd"?: number | null;
    "tool_call_breakdown"?: {
      [k: string]: number /* integer */;
    };
    "file_edit_counts"?: Array<Array<unknown>>;
    [k: string]: unknown;
  };
  "digest_watermark"?: {
    "offset": number /* integer */;
    "prefix_hash": string;
    [k: string]: unknown;
  };
  [k: string]: unknown;
}

/** Event type `pr` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface PrPayload {
  "pr_number": number /* integer */;
  "pr_status": string;
  "review_result": string | null;
  "blocker_count": number /* integer */;
  "time_to_merge_hours": number | null;
  "created_at": string;
  "merged_at": string | null;
  "author": string;
  "title": string;
  [k: string]: unknown;
}

/** Event type `rebuild` — stability: stable-v1 (source: crates/edda-core/src/event.rs). */
export interface RebuildPayload {
  "scope": string;
  "branch": string;
  "reason": string;
  [k: string]: unknown;
}

/** Event type `review_bundle` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface ReviewBundlePayload {
  "bundle_id": string;
  "change_summary": {
    "files": Array<{
      "path": string;
      "added": number /* integer */;
      "deleted": number /* integer */;
      [k: string]: unknown;
    }>;
    "total_added": number /* integer */;
    "total_deleted": number /* integer */;
    "diff_ref": string;
    [k: string]: unknown;
  };
  "test_results": {
    "passed": number /* integer */;
    "failed": number /* integer */;
    "ignored": number /* integer */;
    "total": number /* integer */;
    "failures": Array<string>;
    "command": string;
    [k: string]: unknown;
  };
  "risk_assessment": {
    "level": "low" | "medium" | "high" | "critical";
    "factors": Array<{
      "signal": string;
      "level": "low" | "medium" | "high" | "critical";
      "detail": string;
      [k: string]: unknown;
    }>;
    [k: string]: unknown;
  };
  "suggested_action": "approve" | "review" | "request_changes" | "reject";
  "suggested_reason": string;
  [k: string]: unknown;
}

/** Event type `task.created` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface TaskCreatedPayload {
  "task_id": number /* integer */;
  "title": string;
  "after": Array<number /* integer */>;
  "scope_paths"?: Array<string>;
  "assignee"?: string;
  "agent_kind"?: string;
  "plan_id"?: string;
  "work_unit_ref"?: string;
  "brief_ref"?: string;
  "idempotency_key"?: string;
  [k: string]: unknown;
}

/** Event type `task.done` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface TaskDonePayload {
  "task_id": number /* integer */;
  "receipt": string;
  "evidence_paths": Array<string>;
  [k: string]: unknown;
}

/** Event type `task.failed` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface TaskFailedPayload {
  "task_id": number /* integer */;
  "reason": string;
  [k: string]: unknown;
}

/** Event type `task.requeued` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface TaskRequeuedPayload {
  "task_id": number /* integer */;
  "attempt": number /* integer */;
  [k: string]: unknown;
}

/** Event type `task.session` — stability: unstable (source: crates/edda-core/src/event.rs). */
export type TaskSessionPayload = unknown | unknown;

/** Event type `task.started` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface TaskStartedPayload {
  "task_id": number /* integer */;
  "lease_ttl_s": number /* integer */;
  "attempt": number /* integer */;
  [k: string]: unknown;
}

/** Event type `task_intake` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface TaskIntakePayload {
  "source": string;
  "source_id": string;
  "source_url": string;
  "title": string;
  "intent": string;
  "labels": Array<string>;
  "priority": string;
  "constraints": Array<string>;
  [k: string]: unknown;
}

/** Event type `verdict.recorded` — stability: unstable (source: crates/edda-core/src/event.rs). */
export interface VerdictRecordedPayload {
  "subject": string;
  "decision": "approved" | "rejected";
  "sha": string;
  "comment"?: string;
  "actor": string;
  [k: string]: unknown;
}

// ── Stability-partitioned unions (contract §3) ──

/** Layer 1 stable event payload union (registry stability "stable-v1"). */
export type Layer1Payload = BranchCreatePayload | BranchSwitchPayload | CheckpointPayload | CmdPayload | CommitPayload | DecisionImportPayload | DecisionRatifyPayload | MergePayload | NotePayload | RebuildPayload;

/** Layer 2 experimental payload union (registry stability "unstable") — may change in any release. */
export type Layer2Payload = AgentPhaseChangePayload | ApprovalPayload | ApprovalPolicyMatchPayload | ApprovalRequestPayload | CycleTelemetryPayload | DecideSnapshotPayload | DevicePairPayload | DeviceRevokePayload | ExecutionEventPayload | IngestionPayload | PrPayload | ReviewBundlePayload | TaskCreatedPayload | TaskDonePayload | TaskFailedPayload | TaskRequeuedPayload | TaskSessionPayload | TaskStartedPayload | TaskIntakePayload | VerdictRecordedPayload;
