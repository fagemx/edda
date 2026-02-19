import { resolve, dirname } from 'node:path';
import type { Plan } from '../plan/types.js';
import { topoSort } from '../plan/parser.js';
import {
  type PlanState,
  transition,
  savePlanState,
  loadOrInitState,
  buildRunnerStatus,
  saveRunnerStatus,
  getPhase,
} from '../plan/state.js';
import { runChecks } from '../checks/engine.js';
import { recordEvent, type EventWriterOptions } from '../integration/ledger.js';
import { checkEddaAvailable } from '../agent/hooks.js';
import { findNextPhase, isPlanBlocked, isPlanComplete } from './find-next.js';
import { isShuttingDown, setCurrentAbort, installShutdownHandlers } from './shutdown.js';
import { formatDuration } from '../lib/utils/format.js';
import type { PhaseResult } from './types.js';

export interface RunPlanOptions {
  planFile: string;
  cwd?: string;
}

/**
 * Main runner loop. Executes phases sequentially in topological order.
 *
 * Note: Agent SDK is not yet installed (Wave 2 dependency).
 * runAgentPhase() is a placeholder that will be replaced with actual
 * query() call when the SDK is available.
 */
export async function runPlan(options: RunPlanOptions): Promise<void> {
  const { loadPlanFile } = await import('../plan/parser.js');

  installShutdownHandlers();

  // 1. Load and validate plan
  const plan = loadPlanFile(options.planFile);
  const planFile = resolve(options.planFile);
  const cwd = options.cwd ?? plan.cwd ?? dirname(planFile);
  const order = topoSort(plan.phases);

  // 2. Load or initialize state
  const state = loadOrInitState(plan, planFile);
  const eddaEnabled = checkEddaAvailable();

  const eventOpts: EventWriterOptions = {
    eddaEnabled,
    jsonLogPath: resolve('.edda', 'orchestrator', plan.name, 'events.jsonl'),
    cwd,
  };

  // 3. Record plan:start if fresh
  if (!state.started_at) {
    state.started_at = new Date().toISOString();
    savePlanState(state);
    recordEvent(eventOpts, {
      type: 'plan:start',
      plan_name: plan.name,
      phase_count: plan.phases.length,
      plan_file: planFile,
    });
  }

  const planStartMs = Date.now();

  // 4. Runner loop
  while (true) {
    if (isShuttingDown()) {
      console.log('Shutdown complete. Run `orchestrate run` to resume.');
      break;
    }

    if (isPlanBlocked(state)) {
      const failed = state.phases.find((p) => p.status === 'failed' || p.status === 'stale');
      console.log(`\n✗ Plan blocked: phase "${failed?.id}" is ${failed?.status}`);
      if (failed?.error) {
        console.log(`  ${failed.error.message}`);
      }
      console.log('  Use `orchestrate retry` or `orchestrate skip` to continue.');
      break;
    }

    const nextId = findNextPhase(plan, state, order);
    if (!nextId) {
      // All done or no runnable phase
      break;
    }

    const planPhase = plan.phases.find((p) => p.id === nextId)!;
    const phaseState = getPhase(state, nextId);
    const phaseCwd = planPhase.cwd ?? cwd;

    // Transition: pending → running
    transition(state, nextId, 'pending', 'running', {
      started_at: new Date().toISOString(),
      attempts: phaseState.attempts + 1,
      checks: [],
      error: null,
    });
    savePlanState(state);
    saveRunnerStatus(plan.name, buildRunnerStatus(state));

    console.log(`\n▶ Phase "${nextId}" (attempt ${phaseState.attempts + 1})`);

    // Execute phase
    const abortController = new AbortController();
    setCurrentAbort(abortController);

    const result = await runAgentPhase(planPhase, plan, state, phaseCwd, abortController);

    setCurrentAbort(null);

    // Handle result
    if (result.type === 'agent_done') {
      // Transition: running → checking
      transition(state, nextId, 'running', 'checking');
      savePlanState(state);

      // Run checks
      const checks = planPhase.check as Record<string, unknown>[];
      if (checks.length > 0) {
        console.log(`  ⏳ Running ${checks.length} checks...`);
        const checkResult = await runChecks(checks, phaseCwd);

        if (checkResult.allPassed) {
          transition(state, nextId, 'checking', 'passed', {
            completed_at: new Date().toISOString(),
            checks: checkResult.results,
          });
          const elapsed = Date.now() - new Date(phaseState.started_at!).getTime();
          console.log(`  ✓ Phase "${nextId}" passed (${formatDuration(elapsed)})`);
          recordEvent(eventOpts, {
            type: 'phase:passed',
            plan_name: plan.name,
            phase_id: nextId,
            duration_ms: elapsed,
            attempts: getPhase(state, nextId).attempts,
            cost_usd: result.cost_usd,
          });
        } else {
          transition(state, nextId, 'checking', 'failed', {
            checks: checkResult.results,
            error: checkResult.error,
          });
          console.log(`  ✗ Phase "${nextId}" failed: ${checkResult.error?.message}`);
          recordEvent(eventOpts, {
            type: 'phase:failed',
            plan_name: plan.name,
            phase_id: nextId,
            duration_ms: Date.now() - new Date(phaseState.started_at!).getTime(),
            attempts: getPhase(state, nextId).attempts,
            error_type: 'check_failed',
          });
          handleOnFail(planPhase.on_fail, state, nextId);
        }
      } else {
        // No checks → auto-pass
        transition(state, nextId, 'checking', 'passed', {
          completed_at: new Date().toISOString(),
        });
        const elapsed = Date.now() - new Date(phaseState.started_at!).getTime();
        console.log(`  ✓ Phase "${nextId}" passed — no checks (${formatDuration(elapsed)})`);
        recordEvent(eventOpts, {
          type: 'phase:passed',
          plan_name: plan.name,
          phase_id: nextId,
          duration_ms: elapsed,
          attempts: getPhase(state, nextId).attempts,
          cost_usd: result.cost_usd,
        });
      }
    } else if (result.type === 'timeout') {
      transition(state, nextId, 'running', 'stale', {
        error: { type: 'timeout', message: 'phase timed out', retryable: true, timestamp: new Date().toISOString() },
      });
      console.log(`  ⏰ Phase "${nextId}" timed out`);
    } else if (result.type === 'agent_crash') {
      transition(state, nextId, 'running', 'failed', {
        error: { type: 'agent_crash', message: result.error, retryable: true, timestamp: new Date().toISOString() },
      });
      console.log(`  ✗ Phase "${nextId}" crashed: ${result.error}`);
      recordEvent(eventOpts, {
        type: 'phase:failed',
        plan_name: plan.name,
        phase_id: nextId,
        duration_ms: Date.now() - new Date(phaseState.started_at!).getTime(),
        attempts: getPhase(state, nextId).attempts,
        error_type: 'agent_crash',
      });
    } else if (result.type === 'max_turns') {
      transition(state, nextId, 'running', 'failed', {
        error: {
          type: 'agent_crash',
          message: `Agent exceeded maxTurns (cost: $${result.cost_usd?.toFixed(2) ?? '?'})`,
          retryable: true,
          timestamp: new Date().toISOString(),
        },
      });
      console.log(`  ✗ Phase "${nextId}" exceeded maxTurns`);
    } else if (result.type === 'budget_exceeded') {
      transition(state, nextId, 'running', 'failed', {
        error: {
          type: 'agent_crash',
          message: `Budget exceeded: $${result.cost_usd?.toFixed(2) ?? '?'}`,
          retryable: false,
          timestamp: new Date().toISOString(),
        },
      });
      console.log(`  ✗ Phase "${nextId}" budget exceeded`);
    }

    savePlanState(state);
    saveRunnerStatus(plan.name, buildRunnerStatus(state));
  }

  // 5. Plan-level final event
  if (isPlanComplete(state)) {
    state.completed_at = new Date().toISOString();
    state.status = 'completed';
    savePlanState(state);

    const totalMs = Date.now() - planStartMs;
    const passed = state.phases.filter((p) => p.status === 'passed').length;
    const skipped = state.phases.filter((p) => p.status === 'skipped').length;
    const totalAttempts = state.phases.reduce((sum, p) => sum + p.attempts, 0);

    console.log(`\n✓ Plan "${plan.name}" completed (${formatDuration(totalMs)})`);
    recordEvent(eventOpts, {
      type: 'plan:completed',
      plan_name: plan.name,
      duration_ms: totalMs,
      phases_passed: passed,
      phases_skipped: skipped,
      total_attempts: totalAttempts,
    });
  }

  saveRunnerStatus(plan.name, buildRunnerStatus(state));
}

// ── on_fail handling ──

function handleOnFail(onFail: string, state: PlanState, phaseId: string): void {
  if (onFail === 'skip') {
    transition(state, phaseId, 'failed', 'pending');
    const phase = getPhase(state, phaseId);
    phase.status = 'skipped';
    phase.skip_reason = 'auto-skipped by on_fail policy';
    console.log(`  → Auto-skipped (on_fail: skip)`);
  } else if (onFail === 'abort') {
    state.status = 'aborted';
    state.aborted_at = new Date().toISOString();
    console.log(`  → Plan aborted (on_fail: abort)`);
  }
  // 'ask' — runner loop will detect blocked state and stop
}

// ── Agent phase execution (placeholder for Agent SDK) ──

/**
 * Execute a phase using the Agent SDK.
 *
 * TODO: Replace with actual Agent SDK query() call when
 * @anthropic-ai/claude-agent-sdk is installed (Wave 2).
 *
 * Currently returns agent_done immediately for testing the runner loop.
 */
async function runAgentPhase(
  _phase: Plan['phases'][number],
  _plan: Plan,
  _state: PlanState,
  _cwd: string,
  _abortController: AbortController,
): Promise<PhaseResult> {
  // Placeholder — will be replaced with actual Agent SDK integration
  // For now, simulate immediate completion
  return { type: 'agent_done' };
}
