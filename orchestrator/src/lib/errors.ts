/** Error categories for the orchestrator */
export const ErrorCode = {
  // Plan errors
  PLAN_NOT_FOUND: 'PLAN_NOT_FOUND',
  PLAN_PARSE_ERROR: 'PLAN_PARSE_ERROR',
  PLAN_VALIDATION_ERROR: 'PLAN_VALIDATION_ERROR',
  PLAN_CYCLE_DETECTED: 'PLAN_CYCLE_DETECTED',

  // State errors
  STATE_CORRUPT: 'STATE_CORRUPT',
  STATE_TRANSITION_INVALID: 'STATE_TRANSITION_INVALID',
  STATE_STALE: 'STATE_STALE',

  // Check errors
  CHECK_FAILED: 'CHECK_FAILED',
  CHECK_TIMEOUT: 'CHECK_TIMEOUT',
  CHECK_UNKNOWN_TYPE: 'CHECK_UNKNOWN_TYPE',

  // Runner errors
  PHASE_MAX_ATTEMPTS: 'PHASE_MAX_ATTEMPTS',
  PHASE_TIMEOUT: 'PHASE_TIMEOUT',
  AGENT_SDK_ERROR: 'AGENT_SDK_ERROR',

  // CLI errors
  MISSING_ARGUMENT: 'MISSING_ARGUMENT',
  INTERACTIVE_REQUIRED: 'INTERACTIVE_REQUIRED',
} as const;

export type ErrorCode = (typeof ErrorCode)[keyof typeof ErrorCode];

/** Orchestrator error with code and optional remediation hint */
export class OrchestratorError extends Error {
  constructor(
    public readonly code: ErrorCode,
    message: string,
    public readonly hint?: string,
  ) {
    super(message);
    this.name = 'OrchestratorError';
  }
}
