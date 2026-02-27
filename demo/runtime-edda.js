/**
 * runtime-edda.js — Reference adapter for karvi task-engine → edda conductor.
 *
 * Usage:
 *   const { dispatch, capabilities } = require('./runtime-edda');
 *   const result = await dispatch(plan, { briefUrl, onEvent });
 *
 * Requires: `edda` CLI in PATH, Node.js >= 18
 */

const { spawn } = require("child_process");
const readline = require("readline");
const fs = require("fs");
const path = require("path");
const os = require("os");

/**
 * Dispatch a plan to edda conductor.
 * @param {Object} plan - DispatchPlan from karvi
 * @param {Object} opts
 * @param {string} [opts.briefUrl] - PATCH endpoint for brief updates
 * @param {function} [opts.onEvent] - callback(event) for each event
 * @param {string} [opts.cwd] - working directory override
 * @returns {Promise<{ code: number, events: object[], error?: string }>}
 */
async function dispatch(plan, opts = {}) {
  const planYaml = buildPlanYaml(plan);
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "edda-dispatch-"));
  const planFile = path.join(tmpDir, "plan.yaml");
  fs.writeFileSync(planFile, planYaml);

  const args = ["conduct", "run", planFile, "--json"];
  if (opts.cwd) args.push("--cwd", opts.cwd);

  return new Promise((resolve) => {
    const child = spawn("edda", args, {
      stdio: ["ignore", "pipe", "pipe"],
      cwd: opts.cwd || tmpDir,
    });

    const events = [];
    let stderr = "";

    const rl = readline.createInterface({ input: child.stdout });
    rl.on("line", (line) => {
      try {
        const event = JSON.parse(line);
        events.push(event);
        if (opts.onEvent) opts.onEvent(event);
        if (opts.briefUrl) {
          const patch = translateEvent(event, plan.taskId);
          if (patch) briefPatch(opts.briefUrl, plan.taskId, patch);
        }
      } catch {
        // Non-JSON line (shouldn't happen with --json, ignore)
      }
    });

    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });

    child.on("close", (code) => {
      // Clean up temp plan file
      try {
        fs.unlinkSync(planFile);
        fs.rmdirSync(tmpDir);
      } catch {
        // best-effort cleanup
      }
      resolve({
        code: code ?? 1,
        events,
        error: code !== 0 ? stderr.trim() || `exit code ${code}` : undefined,
      });
    });
  });
}

/**
 * Convert a DispatchPlan to edda plan.yaml content.
 */
function buildPlanYaml(plan) {
  const name = `dispatch-${plan.taskId || "task"}`;
  const maxAttempts = plan.controlsSnapshot?.max_review_attempts ?? 3;
  const timeoutSec = plan.timeoutSec ?? 600;

  // Build context from artifacts
  const context = (plan.artifacts || [])
    .map((a) => `- ${a.title} (${a.status}): ${a.summary}`)
    .join("\n");

  const prompt = context
    ? `${plan.message}\n\nContext from previous tasks:\n${context}`
    : plan.message;

  const tags = [];
  if (plan.taskId) tags.push(`karvi:${plan.taskId}`);
  if (plan.planId) tags.push(`dispatch:${plan.planId}`);

  return [
    `name: "${name}"`,
    `max_attempts: ${maxAttempts}`,
    `timeout_sec: ${timeoutSec}`,
    tags.length ? `tags: [${tags.map((t) => `"${t}"`).join(", ")}]` : null,
    `phases:`,
    `  - id: main`,
    `    prompt: |`,
    ...prompt.split("\n").map((l) => `      ${l}`),
    plan.mode === "redispatch" ? `    env:` : null,
    plan.mode === "redispatch" ? `      REDISPATCH: '1'` : null,
  ]
    .filter(Boolean)
    .join("\n");
}

/**
 * Translate an edda conductor event to a brief PATCH payload.
 * Returns null if no patch needed.
 */
function translateEvent(event, taskId) {
  switch (event.type) {
    case "plan_start":
      return {
        plan: { name: event.plan_name, totalPhases: event.phase_count },
        completedPhases: 0,
      };

    case "phase_start":
      return {
        phases: {
          [event.phase_id]: {
            status: "running",
            attempts: event.attempt,
            startedAt: event.ts,
          },
        },
        currentPhase: event.phase_id,
      };

    case "phase_passed":
      return {
        phases: {
          [event.phase_id]: {
            status: "passed",
            duration_ms: event.duration_ms,
            cost_usd: event.cost_usd,
            completedAt: event.ts,
          },
        },
        cost: event.cost_usd != null ? { phase_usd: event.cost_usd } : undefined,
      };

    case "phase_failed":
      return {
        phases: {
          [event.phase_id]: {
            status: "failed",
            attempts: event.attempt,
            error: event.error,
          },
        },
      };

    case "phase_skipped":
      return {
        phases: {
          [event.phase_id]: {
            status: "skipped",
            reason: event.reason,
          },
        },
      };

    case "plan_completed":
      return {
        cost: { total_usd: event.total_cost_usd },
      };

    case "plan_aborted":
      return {
        aborted: true,
        phases_passed: event.phases_passed,
        phases_pending: event.phases_pending,
      };

    default:
      return null;
  }
}

/**
 * Extract reply text from collected events.
 */
function extractReplyText(events) {
  const completed = events.find((e) => e.type === "plan_completed");
  if (completed) {
    return `Plan completed: ${completed.phases_passed} phases, $${completed.total_cost_usd.toFixed(2)}`;
  }
  const aborted = events.find((e) => e.type === "plan_aborted");
  if (aborted) {
    return `Plan aborted: ${aborted.phases_passed} passed, ${aborted.phases_pending} pending`;
  }
  return "Plan finished (no terminal event)";
}

/**
 * Extract session ID from events (first phase_start).
 */
function extractSessionId(events) {
  const start = events.find((e) => e.type === "phase_start");
  return start ? `edda-${start.phase_id}-${start.attempt}` : null;
}

/**
 * Runtime capabilities declaration.
 */
function capabilities() {
  return { name: "edda", streaming: true, phases: true };
}

/**
 * Send a PATCH to the brief API. Best-effort, no retries.
 */
async function briefPatch(baseUrl, taskId, payload) {
  try {
    await fetch(`${baseUrl}/${taskId}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
  } catch {
    // best-effort
  }
}

module.exports = {
  dispatch,
  buildPlanYaml,
  translateEvent,
  extractReplyText,
  extractSessionId,
  capabilities,
};
