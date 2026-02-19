import { Command } from 'commander';
import chalk from 'chalk';
import { withErrorHandler } from '../../lib/command/with-error-handler.js';
import {
  findPlanForPhase,
  loadPlanState,
  savePlanState,
  statePath,
  getPhase,
} from '../../plan/state.js';
import { OrchestratorError, ErrorCode } from '../../lib/errors.js';
import { isInteractive } from '../../lib/utils/prompt-utils.js';
import { runPlan } from '../../runner/runner.js';

export const skipCommand = new Command('skip')
  .description('Skip a phase')
  .argument('<phase-id>', 'Phase ID to skip')
  .option('--plan <name>', 'Plan name (if multiple plans have same phase ID)')
  .option('--reason <reason>', 'Skip reason')
  .option('--json', 'Output result as JSON')
  .option('--continue', 'Continue running the plan after skipping')
  .action(
    withErrorHandler(async (phaseId: string, options: {
      plan?: string;
      reason?: string;
      json?: boolean;
      continue?: boolean;
    }) => {
      // Find the plan state
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

      // Validate: only pending, failed, stale can be skipped
      if (phase.status !== 'pending' && phase.status !== 'failed' && phase.status !== 'stale') {
        throw new OrchestratorError(
          ErrorCode.STATE_TRANSITION_INVALID,
          `Cannot skip phase "${phaseId}": status is "${phase.status}" (only pending/failed/stale can be skipped)`,
        );
      }

      // Reason is required
      const reason = options.reason ?? 'skipped by user';
      if (!options.reason && !isInteractive() && !options.json) {
        console.error(chalk.yellow('  Note: use --reason to provide a skip reason'));
      }

      // Skip the phase
      phase.status = 'skipped';
      phase.skip_reason = reason;
      phase.error = null;
      savePlanState(state);

      if (options.json) {
        console.log(JSON.stringify({ phase: phaseId, action: 'skip', status: 'ok', reason }));
      } else {
        console.log(`⏭ Skipping phase "${phaseId}"`);
        console.log(`  Reason: ${reason}`);
      }

      // Optionally continue running
      if (options.continue) {
        if (!options.json) {
          console.log('\nContinuing with next phase...');
        }
        await runPlan({ planFile: state.plan_file });
      }
    }),
  );
