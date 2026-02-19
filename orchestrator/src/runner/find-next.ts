import type { Plan } from '../plan/types.js';
import type { PlanState } from '../plan/state.js';

/**
 * Find the next phase to run, respecting topological order and dependencies.
 * Returns the phase ID or null if no phase is runnable.
 */
export function findNextPhase(plan: Plan, state: PlanState, order: string[]): string | null {
  for (const id of order) {
    const phaseState = state.phases.find((p) => p.id === id);
    if (!phaseState || phaseState.status !== 'pending') continue;

    const planPhase = plan.phases.find((p) => p.id === id);
    if (!planPhase) continue;

    // Check all dependencies are satisfied (passed or skipped)
    const depsOk = planPhase.depends_on.every((dep) => {
      const depState = state.phases.find((p) => p.id === dep);
      return depState?.status === 'passed' || depState?.status === 'skipped';
    });

    if (depsOk) return id;
  }

  return null;
}

/**
 * Check if the plan is blocked (has failed/stale phases preventing progress).
 */
export function isPlanBlocked(state: PlanState): boolean {
  return state.phases.some(
    (p) => p.status === 'failed' || p.status === 'stale',
  );
}

/**
 * Check if the plan is complete (all phases passed or skipped).
 */
export function isPlanComplete(state: PlanState): boolean {
  return state.phases.every(
    (p) => p.status === 'passed' || p.status === 'skipped',
  );
}
