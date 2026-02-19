import { Command } from 'commander';
import chalk from 'chalk';
import { withErrorHandler } from '../../lib/command/with-error-handler.js';
import { listPlanStates, loadPlanState, statePath } from '../../plan/state.js';
import { formatDuration } from '../../lib/utils/format.js';
import type { PlanState, PhaseState } from '../../plan/state.js';

function statusIcon(status: string): string {
  switch (status) {
    case 'completed':
    case 'passed': return chalk.green('✅');
    case 'running':
    case 'checking': return chalk.yellow('●');
    case 'blocked':
    case 'failed':
    case 'stale': return chalk.red('✗');
    case 'skipped': return chalk.dim('⏭');
    case 'aborted': return chalk.red('⚠');
    case 'pending': return chalk.dim('○');
    default: return chalk.dim('?');
  }
}

function phaseDuration(p: PhaseState): string {
  if (!p.started_at) return '';
  const end = p.completed_at ?? new Date().toISOString();
  return formatDuration(new Date(end).getTime() - new Date(p.started_at).getTime());
}

function printPlanSummary(states: PlanState[]): void {
  if (states.length === 0) {
    console.log(chalk.dim('No plans found.'));
    console.log(chalk.dim('  Create a plan.yaml and run: orchestrate run plan.yaml'));
    return;
  }

  console.log('Plans:');
  const nameWidth = Math.max(4, ...states.map((s) => s.plan_name.length));
  for (const s of states) {
    const completed = s.phases.filter(
      (p) => p.status === 'passed' || p.status === 'skipped',
    ).length;
    const duration = s.started_at
      ? formatDuration(Date.now() - new Date(s.started_at).getTime())
      : '';
    console.log(
      `  ${s.plan_name.padEnd(nameWidth)}  ${statusIcon(s.status)} ${s.status.padEnd(10)}  ${completed}/${s.phases.length} phases  ${duration}`,
    );
  }
}

function printPlanDetail(state: PlanState): void {
  console.log(`Plan: ${chalk.bold(state.plan_name)} (${state.status})`);
  if (state.started_at) {
    console.log(`Started: ${new Date(state.started_at).toLocaleString()}`);
  }
  console.log('');

  const idWidth = Math.max(4, ...state.phases.map((p) => p.id.length));
  for (const p of state.phases) {
    const dur = phaseDuration(p);
    const attempts = p.attempts > 0 ? `(${p.attempts} attempt${p.attempts > 1 ? 's' : ''})` : '';
    const error = p.error ? chalk.red(`  ${p.error.message}`) : '';
    const skipReason = p.skip_reason ? chalk.dim(`  ${p.skip_reason}`) : '';

    console.log(
      `  ${statusIcon(p.status)} ${p.id.padEnd(idWidth)}  ${p.status.padEnd(10)}  ${dur.padStart(8)}  ${attempts}`,
    );
    if (error) console.log(`    ${error}`);
    if (skipReason) console.log(`    ${skipReason}`);
  }
}

export const statusCommand = new Command('status')
  .description('Show plan execution status')
  .argument('[plan-name]', 'Specific plan name to show details')
  .option('--json', 'Output result as JSON')
  .action(
    withErrorHandler(async (planName: string | undefined, options: { json?: boolean }) => {
      if (planName) {
        // Specific plan
        const state = loadPlanState(statePath(planName));
        if (!state) {
          console.error(chalk.red(`✗ No state found for plan "${planName}"`));
          console.error(chalk.dim('  Run: orchestrate status (to list all plans)'));
          process.exit(1);
        }

        if (options.json) {
          console.log(JSON.stringify({
            plan: state.plan_name,
            status: state.status,
            started_at: state.started_at,
            phases: state.phases.map((p) => ({
              id: p.id,
              status: p.status,
              attempts: p.attempts,
              error: p.error?.message,
            })),
          }));
          return;
        }

        printPlanDetail(state);
      } else {
        // List all plans
        const states = listPlanStates();

        if (options.json) {
          console.log(JSON.stringify(
            states.map((s) => ({
              plan: s.plan_name,
              status: s.status,
              phases_completed: s.phases.filter(
                (p) => p.status === 'passed' || p.status === 'skipped',
              ).length,
              phases_total: s.phases.length,
            })),
          ));
          return;
        }

        printPlanSummary(states);
      }
    }),
  );
