import chalk from 'chalk';

const DEBUG = process.env.ORCHESTRATE_DEBUG ?? '';

/** Debug logger gated by ORCHESTRATE_DEBUG env var */
export function debug(namespace: string, ...args: unknown[]): void {
  if (!DEBUG) return;
  if (DEBUG === '*' || namespace.startsWith(DEBUG.replace('*', ''))) {
    console.error(chalk.dim(`[DEBUG] [${namespace}]`), ...args);
  }
}
