/**
 * Tests for runtime-edda.js adapter.
 * Run: node demo/tests/test_runtime_edda.js
 */

const assert = require("assert");
const {
  buildPlanYaml,
  translateEvent,
  extractReplyText,
  extractSessionId,
  capabilities,
} = require("../runtime-edda");

// ── buildPlanYaml ──

function testBuildPlanYamlBasic() {
  const plan = {
    taskId: "T5",
    planId: "disp_abc",
    message: "Add user auth with JWT",
    timeoutSec: 600,
    controlsSnapshot: { max_review_attempts: 3 },
  };
  const yaml = buildPlanYaml(plan);
  assert(yaml.includes('name: "dispatch-T5"'), "should include plan name");
  assert(yaml.includes("max_attempts: 3"), "should include max_attempts");
  assert(yaml.includes("timeout_sec: 600"), "should include timeout_sec");
  assert(yaml.includes("Add user auth with JWT"), "should include prompt");
  assert(yaml.includes('"karvi:T5"'), "should include karvi tag");
  assert(yaml.includes('"dispatch:disp_abc"'), "should include dispatch tag");
  console.log("  PASS: buildPlanYaml basic");
}

function testBuildPlanYamlWithArtifacts() {
  const plan = {
    taskId: "T5",
    message: "Build API",
    artifacts: [
      { id: "T3", title: "Setup DB", status: "approved", summary: "PostgreSQL schema" },
    ],
  };
  const yaml = buildPlanYaml(plan);
  assert(yaml.includes("Context from previous tasks"), "should include context header");
  assert(yaml.includes("Setup DB (approved): PostgreSQL schema"), "should include artifact");
  console.log("  PASS: buildPlanYaml with artifacts");
}

function testBuildPlanYamlRedispatch() {
  const plan = {
    taskId: "T5",
    message: "Retry auth",
    mode: "redispatch",
  };
  const yaml = buildPlanYaml(plan);
  assert(yaml.includes("REDISPATCH"), "should include REDISPATCH env");
  console.log("  PASS: buildPlanYaml redispatch");
}

function testBuildPlanYamlDefaults() {
  const plan = { message: "Do something" };
  const yaml = buildPlanYaml(plan);
  assert(yaml.includes('name: "dispatch-task"'), "should use default name");
  assert(yaml.includes("max_attempts: 3"), "should default to 3 attempts");
  assert(yaml.includes("timeout_sec: 600"), "should default to 600s timeout");
  console.log("  PASS: buildPlanYaml defaults");
}

// ── translateEvent ──

function testTranslatePlanStart() {
  const event = { type: "plan_start", plan_name: "test", phase_count: 2, seq: 0, ts: "2026-01-01T00:00:00Z" };
  const patch = translateEvent(event, "T5");
  assert.deepStrictEqual(patch.plan, { name: "test", totalPhases: 2 });
  assert.strictEqual(patch.completedPhases, 0);
  console.log("  PASS: translateEvent plan_start");
}

function testTranslatePhaseStart() {
  const event = { type: "phase_start", phase_id: "build", attempt: 1, seq: 1, ts: "2026-01-01T00:00:01Z" };
  const patch = translateEvent(event, "T5");
  assert.strictEqual(patch.phases.build.status, "running");
  assert.strictEqual(patch.currentPhase, "build");
  console.log("  PASS: translateEvent phase_start");
}

function testTranslatePhasePassed() {
  const event = {
    type: "phase_passed", phase_id: "build", attempt: 1,
    duration_ms: 5000, cost_usd: 0.42, seq: 2, ts: "2026-01-01T00:00:06Z",
  };
  const patch = translateEvent(event, "T5");
  assert.strictEqual(patch.phases.build.status, "passed");
  assert.strictEqual(patch.phases.build.duration_ms, 5000);
  assert.strictEqual(patch.cost.phase_usd, 0.42);
  console.log("  PASS: translateEvent phase_passed");
}

function testTranslatePhaseFailed() {
  const event = {
    type: "phase_failed", phase_id: "test", attempt: 2,
    duration_ms: 3000, error: "check failed", seq: 3, ts: "2026-01-01T00:00:09Z",
  };
  const patch = translateEvent(event, "T5");
  assert.strictEqual(patch.phases.test.status, "failed");
  assert.strictEqual(patch.phases.test.error, "check failed");
  console.log("  PASS: translateEvent phase_failed");
}

function testTranslatePhaseSkipped() {
  const event = { type: "phase_skipped", phase_id: "docs", reason: "manually skipped", seq: 4, ts: "2026-01-01T00:00:10Z" };
  const patch = translateEvent(event, "T5");
  assert.strictEqual(patch.phases.docs.status, "skipped");
  assert.strictEqual(patch.phases.docs.reason, "manually skipped");
  console.log("  PASS: translateEvent phase_skipped");
}

function testTranslatePlanCompleted() {
  const event = { type: "plan_completed", phases_passed: 3, total_cost_usd: 1.50, seq: 5, ts: "2026-01-01T00:01:00Z" };
  const patch = translateEvent(event, "T5");
  assert.strictEqual(patch.cost.total_usd, 1.50);
  console.log("  PASS: translateEvent plan_completed");
}

function testTranslatePlanAborted() {
  const event = { type: "plan_aborted", phases_passed: 1, phases_pending: 2, seq: 5, ts: "2026-01-01T00:01:00Z" };
  const patch = translateEvent(event, "T5");
  assert.strictEqual(patch.aborted, true);
  assert.strictEqual(patch.phases_passed, 1);
  console.log("  PASS: translateEvent plan_aborted");
}

function testTranslateUnknownEvent() {
  const event = { type: "unknown_event", seq: 99 };
  const patch = translateEvent(event, "T5");
  assert.strictEqual(patch, null);
  console.log("  PASS: translateEvent unknown returns null");
}

// ── extractReplyText ──

function testExtractReplyTextCompleted() {
  const events = [
    { type: "plan_start", plan_name: "test", phase_count: 2 },
    { type: "plan_completed", phases_passed: 2, total_cost_usd: 1.50 },
  ];
  const text = extractReplyText(events);
  assert(text.includes("2 phases"), "should mention phases");
  assert(text.includes("$1.50"), "should mention cost");
  console.log("  PASS: extractReplyText completed");
}

function testExtractReplyTextAborted() {
  const events = [
    { type: "plan_start", plan_name: "test", phase_count: 3 },
    { type: "plan_aborted", phases_passed: 1, phases_pending: 2 },
  ];
  const text = extractReplyText(events);
  assert(text.includes("aborted"), "should mention aborted");
  console.log("  PASS: extractReplyText aborted");
}

// ── extractSessionId ──

function testExtractSessionId() {
  const events = [
    { type: "plan_start", plan_name: "test", phase_count: 1 },
    { type: "phase_start", phase_id: "build", attempt: 1 },
  ];
  const sid = extractSessionId(events);
  assert.strictEqual(sid, "edda-build-1");
  console.log("  PASS: extractSessionId");
}

function testExtractSessionIdEmpty() {
  const sid = extractSessionId([]);
  assert.strictEqual(sid, null);
  console.log("  PASS: extractSessionId empty");
}

// ── capabilities ──

function testCapabilities() {
  const caps = capabilities();
  assert.strictEqual(caps.name, "edda");
  assert.strictEqual(caps.streaming, true);
  assert.strictEqual(caps.phases, true);
  console.log("  PASS: capabilities");
}

// ── Run all ──

console.log("\nruntime-edda.js tests\n");

console.log("buildPlanYaml:");
testBuildPlanYamlBasic();
testBuildPlanYamlWithArtifacts();
testBuildPlanYamlRedispatch();
testBuildPlanYamlDefaults();

console.log("\ntranslateEvent:");
testTranslatePlanStart();
testTranslatePhaseStart();
testTranslatePhasePassed();
testTranslatePhaseFailed();
testTranslatePhaseSkipped();
testTranslatePlanCompleted();
testTranslatePlanAborted();
testTranslateUnknownEvent();

console.log("\nextractReplyText:");
testExtractReplyTextCompleted();
testExtractReplyTextAborted();

console.log("\nextractSessionId:");
testExtractSessionId();
testExtractSessionIdEmpty();

console.log("\ncapabilities:");
testCapabilities();

console.log("\nAll tests passed!\n");
