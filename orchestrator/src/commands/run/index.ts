import { Command } from 'commander';
import { resolve } from 'node:path';
import chalk from 'chalk';
import { withErrorHandler } from '../../lib/command/with-error-handler.js';
import { loadPlanFile, topoSort } from '../../plan/parser.js';
import { loadPlanState, statePath } from '../../plan/state.js';
import { runPlan } from '../../runner/runner.js';

export const runCommand = new Command('run')
  .description('Execute a plan (or resume an existing one)')
  .argument('<plan-file>', 'Path to plan YAML file')
  .option('--dry-run', 'Parse and validate only, do not execute')
  .option('--json', 'Output result as JSON')
  .option('--non-interactive', 'Exit on failure instead of prompting')
  .option('--verbose', 'Show agent output')
  .action(
    withErrorHandler(async (planFile: string, options: {
      dryRun?: boolean;
      json?: boolean;
      nonInteractive?: boolean;
      verbose?: boolean;
    }) => {
      const resolvedPath = resolve(planFile);
      const plan = loadPlanFile(resolvedPath);
      const order = topoSort(plan.phases);

      // --dry-run: validate only
      if (options.dryRun) {
        if (options.json) {
          console.log(JSON.stringify({
            plan: plan.name,
            phases: plan.phases.map((p) => ({
              id: p.id,
              depends_on: p.depends_on,
              checks: p.check.length,
            })),
            order,
            valid: true,
          }));
        } else {
          console.log(chalk.green(`✓ Plan "${plan.name}" is valid`));
          console.log(`  ${plan.phases.length} phases`);
          console.log(`  Execution order: ${order.join(' → ')}`);
        }
        return;
      }

      // Print overview
      if (!options.json) {
        console.log(`Plan: ${chalk.bold(plan.name)}`);
        console.log(`  ${plan.phases.length} phases`);
        console.log('');
        for (const phase of plan.phases) {
          const deps = phase.depends_on.length > 0
            ? chalk.dim(` (depends on: ${phase.depends_on.join(', ')})`)
            : '';
          console.log(`  ○ ${phase.id.padEnd(16)} ${phase.prompt.slice(0, 60)}${deps}`);
        }
        console.log('');
      }

      // Execute plan
      await runPlan({ planFile: resolvedPath });

      // Read final state for exit code + JSON output
      const state = loadPlanState(statePath(plan.name));
      if (!state) return;

      if (options.json) {
        const totalMs = state.started_at && state.completed_at
          ? new Date(state.completed_at).getTime() - new Date(state.started_at).getTime()
          : undefined;
        console.log(JSON.stringify({
          plan: plan.name,
          status: state.status,
          phases: state.phases.map((p) => ({
            id: p.id,
            status: p.status,
            attempts: p.attempts,
          })),
          duration_ms: totalMs,
        }));
      }

      // Exit codes per spec
      switch (state.status) {
        case 'completed':
          process.exit(0);
          break;
        case 'blocked':
          process.exit(1);
          break;
        case 'aborted':
          process.exit(2);
          break;
      }
    }),
  );
