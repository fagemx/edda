import { Command } from 'commander';
import { withErrorHandler } from '../../lib/command/with-error-handler.js';
import {
  findPlanForPhase,
  loadPlanState,
  savePlanState,
  statePath,
  transition,
  getPhase,
} from '../../plan/state.js';
import { OrchestratorError, ErrorCode } from '../../lib/errors.js';
import { runPlan } from '../../runner/runner.js';

export const retryCommand = new Command('retry')
  .description('Retry a failed or stale phase')
  .argument('<phase-id>', 'Phase ID to retry')
  .option('--plan <name>', 'Plan name (if multiple plans have same phase ID)')
  .option('--json', 'Output result as JSON')
  .action(
    withErrorHandler(async (phaseId: string, options: { plan?: string; json?: boolean }) => {
      // Find the plan state containing this phase
      const state = options.plan
        ? loadPlanState(statePath(options.plan))
        : findPlanForPhase(phaseId);

      if (!state) {
        throw new OrchestratorError(
          ErrorCode.MISSING_ARGUMENT,
          `No plan found containing phase "${phaseId}"`,
          options.plan
            ? `Check plan name: orchestrate status`
            : `Use --plan <name> if multiple plans exist`,
        );
      }

      const phase = getPhase(state, phaseId);

      // Validate: only failed or stale can be retried
      if (phase.status !== 'failed' && phase.status !== 'stale') {
        throw new OrchestratorError(
          ErrorCode.STATE_TRANSITION_INVALID,
          `Cannot retry phase "${phaseId}": status is "${phase.status}" (only failed/stale can be retried)`,
        );
      }

      // Reset phase to pending
      const fromStatus = phase.status;
      transition(state, phaseId, fromStatus, 'pending');
      state.status = 'running';
      savePlanState(state);

      if (options.json) {
        console.log(JSON.stringify({ phase: phaseId, action: 'retry', status: 'ok' }));
      } else {
        console.log(`⟳ Retrying phase "${phaseId}" (attempt ${phase.attempts + 1})`);
      }

      // Re-run the plan (will pick up from the pending phase)
      await runPlan({ planFile: state.plan_file });
    }),
  );
