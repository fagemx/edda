import chalk from 'chalk';
import { OrchestratorError } from '../errors.js';

/**
 * Wrap a CLI command handler with centralized error handling.
 * Catches all errors and prints user-friendly output.
 */
export function withErrorHandler<T extends unknown[]>(
  fn: (...args: T) => Promise<void>,
): (...args: T) => Promise<void> {
  return async (...args: T) => {
    try {
      await fn(...args);
    } catch (err) {
      if (err instanceof OrchestratorError) {
        console.error(chalk.red(`✗ ${err.message}`));
        if (err.hint) {
          console.error(chalk.dim(`  ${err.hint}`));
        }
        // Validation errors get exit code 3
        if (
          err.code === 'PLAN_VALIDATION_ERROR' ||
          err.code === 'PLAN_PARSE_ERROR' ||
          err.code === 'PLAN_NOT_FOUND'
        ) {
          process.exit(3);
        }
        process.exit(1);
      } else if (err instanceof Error) {
        console.error(chalk.red(`✗ ${err.message}`));
      } else {
        console.error(chalk.red('✗ An unexpected error occurred'));
      }
      process.exit(1);
    }
  };
}
