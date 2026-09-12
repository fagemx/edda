// Generated-type probe: representative enum fields in the pinned schema
// corpus must generate as real string-literal unions, never degrade to
// unknown/object, and keep their requiredness. The compile-time assertions
// below are enforced by `tsc` in the contract runner's build step: if a
// field degrades to `unknown`, the positive assignments still compile but
// every `@ts-expect-error` directive becomes unused and the build fails.
// (Compiled from test/ into dist/test/ by the standard build.)

import { test } from "node:test";
import assert from "node:assert/strict";
import type {
  ContinuityCapsulePayload,
  ControlIntentPayload,
  ControlManifestPayload,
  ControlReceiptPayload,
  DecisionImportPayload,
  ExecutionBriefPayload,
  IngestionPayload,
  NotePayload,
  ReviewBundlePayload,
  TaskDonePayload,
  TaskSessionPayload,
  VerdictRecordedPayload,
} from "../src/types.gen.js";

test("generated control events preserve authority, provenance, and commitment types", () => {
  type ManifestRecord = ControlManifestPayload["control_manifest"];
  type Adjudication = NonNullable<ManifestRecord["adjudication"]>;
  type IsOptional<T, K extends keyof T> = Record<string, never> extends Pick<T, K>
    ? true
    : false;

  const authorityAction: ManifestRecord["authority"]["permitted_action"] = "control_adjudicate";
  const intentAction: ControlIntentPayload["control_intent"]["action_kind"] = "needs_decision";
  const receiptState: ControlReceiptPayload["control_receipt"]["next_state"] = "completed";
  const evidence: Adjudication["evidence"] = ["receipt:evt_one"];
  const priorStateRequired: IsOptional<Adjudication, "prior_state_version"> = false;
  const priorDigestRequired: IsOptional<Adjudication, "prior_manifest_digest"> = false;
  const intentCommitmentRequired: IsOptional<
    ControlIntentPayload["control_intent"],
    "action_token"
  > = false;
  const receiptCommitmentRequired: IsOptional<
    ControlReceiptPayload["control_receipt"],
    "action_token"
  > = false;

  assert.equal(authorityAction, "control_adjudicate");
  assert.equal(intentAction, "needs_decision");
  assert.equal(receiptState, "completed");
  assert.deepEqual(evidence, ["receipt:evt_one"]);
  assert.equal(priorStateRequired, false);
  assert.equal(priorDigestRequired, false);
  assert.equal(intentCommitmentRequired, false);
  assert.equal(receiptCommitmentRequired, false);
});

// @ts-expect-error control actions remain the schema's literal union
const badControlAction: ControlIntentPayload["control_intent"]["action_kind"] = "dispatch_task";
// @ts-expect-error local S6a receipts cannot claim an external next state
const badControlState: ControlReceiptPayload["control_receipt"]["next_state"] = "verifying";

void badControlAction;
void badControlState;

test("generated enum fields accept their literal members", () => {
  // Bare enum, required: "auto" | "suggested" | "manual".
  const triggerType: IngestionPayload["triggerType"] = "suggested";
  assert.equal(triggerType, "suggested");
  // Bare enum, required: "L0" | … | "L5".
  const sourceLayer: IngestionPayload["sourceLayer"] = "L1";
  assert.equal(sourceLayer, "L1");
  // Bare enum, required: "approved" | "rejected".
  const decision: VerdictRecordedPayload["decision"] = "approved";
  assert.equal(decision, "approved");
  // Bare enum nested in an object property.
  const riskLevel: ReviewBundlePayload["risk_assessment"]["level"] = "critical";
  assert.equal(riskLevel, "critical");
  // Bare enum nested in an array item schema.
  const factorLevel: ReviewBundlePayload["risk_assessment"]["factors"][number]["level"] = "low";
  assert.equal(factorLevel, "low");
  // Bare enum, required: "approve" | "review" | "request_changes" | "reject".
  const suggestedAction: ReviewBundlePayload["suggested_action"] = "approve";
  assert.equal(suggestedAction, "approve");
  // anyOf-wrapped enum: "local" | "shared" | "global" | null.
  const scope: DecisionImportPayload["decision"]["scope"] = "shared";
  assert.equal(scope, "shared");
  const noScope: DecisionImportPayload["decision"]["scope"] = null;
  assert.equal(noScope, null);
  const noteScope: NonNullable<NotePayload["decision"]>["scope"] = "global";
  assert.equal(noteScope, "global");
  // Local $ref + const/enum resolution in the execution-brief schema.
  const runtimeProfile: ExecutionBriefPayload["execution_brief"]["brief"]["runtime_profile"] = "flash";
  assert.equal(runtimeProfile, "flash");
  // JSON Schema const values stay exact literals at every nesting level.
  const authority: ContinuityCapsulePayload["data_authority"] = "data_only";
  const recordVersion: ContinuityCapsulePayload["continuity"]["record_version"] = 1;
  const capsuleVersion: ContinuityCapsulePayload["continuity"]["capsule"]["capsule_version"] = 1;
  assert.deepEqual([authority, recordVersion, capsuleVersion], ["data_only", 1, 1]);
});

// @ts-expect-error JSON Schema const rejects a different string
const badAuthority: ContinuityCapsulePayload["data_authority"] = "instructions";
// @ts-expect-error JSON Schema const rejects a different number
const badRecordVersion: ContinuityCapsulePayload["continuity"]["record_version"] = 2;

void badAuthority;
void badRecordVersion;

// Attempt-bound controlled task fields stay typed when a schema combines a
// common object shape with anyOf/dependentRequired validation constraints.
const briefEventId: TaskSessionPayload["brief_event_id"] = "evt_01";
const sessionAttempt: TaskSessionPayload["attempt"] = 1;
const doneAttempt: NonNullable<TaskDonePayload["controlled_completion"]>["attempt"] = 1;
const hostSession: TaskSessionPayload = {
  task_id: 1,
  agent_kind: "acp:grok",
  session_id: "session-one",
  attempt: 1,
};
const controlledSession: TaskSessionPayload = {
  ...hostSession,
  brief_event_id: "evt_one",
  brief_digest: "a".repeat(64),
  lease_owner: "owner-one",
};
assert.equal(briefEventId, "evt_01");
assert.equal(sessionAttempt, 1);
assert.equal(doneAttempt, 1);
assert.equal(controlledSession.lease_owner, "owner-one");

// @ts-expect-error anyOf requires either ACP id or the host session triple
const missingSessionShape: TaskSessionPayload = { task_id: 1 };
// @ts-expect-error dependentRequired forbids a partial controlled binding
const partialControlledSession: TaskSessionPayload = {
  task_id: 1,
  acp_session_id: "session-one",
  brief_event_id: "evt_one",
};
// @ts-expect-error attempt is numeric, not an unknown generated fallback
const badSessionAttempt: TaskSessionPayload["attempt"] = "first";

type GeneratedBrief = ExecutionBriefPayload["execution_brief"]["brief"];
const briefBase: Omit<GeneratedBrief, "runtime_profile" | "procedure"> = {
  brief_version: 1 as const,
  brief_id: "brief_one",
  brief_event_id: "evt_one",
  content_digest: "a".repeat(64),
  intent: "fix" as const,
  objective: "bounded fix",
  basis: { base_full_sha: "b".repeat(40) },
  scope: { allowed_paths: ["src/**"] },
  outcome_codes: [{ code: "DONE", result_class: "success" }],
  receipt_schema: { receipt_version: 1 as const, required_fields: [] },
};
// @ts-expect-error Flash/controller requires non-empty probe_cards or implementation_steps
const emptyFlashProcedure: GeneratedBrief = {
  ...briefBase,
  runtime_profile: "flash",
  procedure: { kind: "controller_authored", authored_by: "controller" },
};
const flashImplementation: GeneratedBrief = {
  ...briefBase,
  runtime_profile: "flash",
  procedure: {
    kind: "controller_authored",
    authored_by: "controller",
    implementation_steps: [{ step_id: "step_one", instruction: "change one file" }],
  },
};
const strongPrinciples: GeneratedBrief = {
  ...briefBase,
  runtime_profile: "strong",
  procedure: { kind: "controller_authored", authored_by: "controller" },
};
assert.equal(flashImplementation.runtime_profile, "flash");
assert.equal(strongPrinciples.runtime_profile, "strong");
// @ts-expect-error execution profile remains the schema's literal enum
const badRuntimeProfile: ExecutionBriefPayload["execution_brief"]["brief"]["runtime_profile"] = "tiny";

// Non-members are rejected — this is the assertion that fails the build if
// the field degrades to `unknown` (the @ts-expect-error becomes unused) or
// to `object` (the assignment itself errors).
// @ts-expect-error non-member rejected by the literal union
const badTriggerType: IngestionPayload["triggerType"] = "not-a-trigger";
// @ts-expect-error non-member rejected through the anyOf union
const badScope: DecisionImportPayload["decision"]["scope"] = "region";

// Requiredness is preserved: omitting the required enum field must fail to
// compile even though the interface carries an index signature.
// @ts-expect-error triggerType remains required
const missingTriggerType: IngestionPayload = {
  id: "evt_01",
  eventType: "ingestion",
  sourceLayer: "L1",
  summary: "s",
  detail: {},
  createdAt: "2026-01-01T00:00:00Z",
};
