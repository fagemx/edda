import { Command } from 'commander';
import chalk from 'chalk';
import { withErrorHandler } from '../../lib/command/with-error-handler.js';
import {
  listPlanStates,
  loadPlanState,
  savePlanState,
  statePath,
  buildRunnerStatus,
  saveRunnerStatus,
} from '../../plan/state.js';
import { OrchestratorError, ErrorCode } from '../../lib/errors.js';
import { recordEvent, type EventWriterOptions } from '../../integration/ledger.js';
import { checkEddaAvailable } from '../../agent/hooks.js';
import { resolve } from 'node:path';

export const abortCommand = new Command('abort')
  .description('Abort a running plan')
  .argument('[plan-name]', 'Plan name (defaults to the most recent running plan)')
  .option('--json', 'Output result as JSON')
  .action(
    withErrorHandler(async (planName: string | undefined, options: { json?: boolean }) => {
      let state;

      if (planName) {
        state = loadPlanState(statePath(planName));
      } else {
        // Find the most recent running/pending plan
        const states = listPlanStates();
        state = states.find((s) => s.status === 'running' || s.status === 'pending')
          ?? states[0];
      }

      if (!state) {
        throw new OrchestratorError(
          ErrorCode.MISSING_ARGUMENT,
          planName
            ? `No state found for plan "${planName}"`
            : 'No plans found to abort',
          'Run: orchestrate status (to see all plans)',
        );
      }

      if (state.status === 'completed' || state.status === 'aborted') {
        throw new OrchestratorError(
          ErrorCode.STATE_TRANSITION_INVALID,
          `Plan "${state.plan_name}" is already ${state.status}`,
        );
      }

      // Abort the plan
      state.status = 'aborted';
      state.aborted_at = new Date().toISOString();
      savePlanState(state);
      saveRunnerStatus(state.plan_name, buildRunnerStatus(state));

      // Record event
      const eddaEnabled = checkEddaAvailable();
      const eventOpts: EventWriterOptions = {
        eddaEnabled,
        jsonLogPath: resolve('.edda', 'orchestrator', state.plan_name, 'events.jsonl'),
      };
      recordEvent(eventOpts, {
        type: 'plan:aborted',
        plan_name: state.plan_name,
        phases_passed: state.phases.filter((p) => p.status === 'passed').length,
        phases_pending: state.phases.filter(
          (p) => p.status === 'pending' || p.status === 'running',
        ).length,
      });

      if (options.json) {
        console.log(JSON.stringify({
          plan: state.plan_name,
          action: 'abort',
          status: 'ok',
          phases: state.phases.map((p) => ({
            id: p.id,
            status: p.status,
          })),
        }));
      } else {
        console.log(`⚠ Plan "${state.plan_name}" aborted`);
        console.log('');
        for (const p of state.phases) {
          const icon = p.status === 'passed' ? chalk.green('✅')
            : p.status === 'skipped' ? chalk.dim('⏭')
            : p.status === 'running' ? chalk.red('✗')
            : chalk.dim('○');
          const note = p.status === 'running' ? ' (was running)' : '';
          console.log(`  ${icon} ${p.id.padEnd(16)} ${p.status}${note}`);
        }
      }

      process.exit(2);
    }),
  );
