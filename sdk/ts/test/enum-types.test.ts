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
  DecisionImportPayload,
  IngestionPayload,
  NotePayload,
  ReviewBundlePayload,
  VerdictRecordedPayload,
} from "../src/types.gen.js";

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
});

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
