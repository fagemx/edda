/** Result of running a single phase's Agent session */
export type PhaseResult =
  | { type: 'agent_done'; cost_usd?: number }
  | { type: 'agent_crash'; error: string }
  | { type: 'timeout' }
  | { type: 'max_turns'; cost_usd?: number }
  | { type: 'budget_exceeded'; cost_usd?: number };

/** Default tools allowed for Agent sessions */
export const DEFAULT_TOOLS = ['Read', 'Write', 'Edit', 'Bash', 'Glob', 'Grep'];

/** Liveness idle timeout before aborting (5 minutes) */
export const LIVENESS_IDLE_MS = 5 * 60 * 1000;

/** Liveness check interval (30 seconds) */
export const LIVENESS_CHECK_INTERVAL_MS = 30_000;
