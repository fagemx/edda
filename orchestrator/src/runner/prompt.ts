import type { Plan } from '../plan/types.js';
import type { PlanState } from '../plan/state.js';
import { formatDuration } from '../lib/utils/format.js';

/** Build the full prompt for a phase (context + phase.context + phase.prompt) */
export function buildPrompt(
  phase: { prompt: string; context?: string },
): string {
  const parts: string[] = [];

  if (phase.context) {
    parts.push(phase.context);
  }

  parts.push(phase.prompt);

  // edda context is injected automatically via SessionStart hook (O1),
  // no need to add it here.
  return parts.join('\n\n');
}

/** Build plan-level context injected as systemPrompt */
export function buildPlanContext(
  plan: Plan,
  state: PlanState,
  currentPhaseId: string,
): string {
  const lines: string[] = [];
  const completedCount = state.phases.filter(
    (p) => p.status === 'passed' || p.status === 'skipped',
  ).length;

  lines.push(`## Orchestrator: Plan "${plan.name}"`);
  if (plan.description) {
    lines.push(plan.description);
  }
  lines.push(`Phase ${completedCount + 1}/${plan.phases.length}: ${currentPhaseId}`);
  lines.push('');

  // Summary of completed phases
  for (const phase of state.phases) {
    if (phase.status === 'passed') {
      const duration = phase.started_at && phase.completed_at
        ? formatDuration(new Date(phase.completed_at).getTime() - new Date(phase.started_at).getTime())
        : '?';
      lines.push(`- Phase "${phase.id}": completed (${duration})`);
    } else if (phase.status === 'skipped') {
      lines.push(`- Phase "${phase.id}": skipped`);
    }
  }

  return lines.join('\n');
}
