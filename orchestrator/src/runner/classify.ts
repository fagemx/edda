import type { PhaseResult } from './types.js';

/**
 * Minimal SDKMessage type for result classification.
 * Full type comes from @anthropic-ai/claude-agent-sdk — not yet installed.
 */
export interface SDKResultMessage {
  type: 'result';
  subtype: string;
  total_cost_usd?: number;
  error?: string;
}

/**
 * Classify an Agent SDK result message into a PhaseResult.
 *
 * SDKResultMessage.subtype mapping:
 *   - "success"                → agent_done
 *   - "error_max_turns"        → max_turns
 *   - "error_max_budget_usd"   → budget_exceeded
 *   - "error_during_execution" → agent_crash
 */
export function classifyResult(msg: unknown): PhaseResult {
  if (!msg || typeof msg !== 'object') {
    return { type: 'agent_crash', error: 'no result message from Agent SDK' };
  }

  const result = msg as SDKResultMessage;
  if (result.type !== 'result') {
    return { type: 'agent_crash', error: `unexpected message type: ${result.type}` };
  }

  const cost = result.total_cost_usd;

  switch (result.subtype) {
    case 'success':
      return { type: 'agent_done', cost_usd: cost };
    case 'error_max_turns':
      return { type: 'max_turns', cost_usd: cost };
    case 'error_max_budget_usd':
      return { type: 'budget_exceeded', cost_usd: cost };
    case 'error_during_execution':
      return { type: 'agent_crash', error: result.error ?? 'unknown execution error' };
    default:
      return { type: 'agent_crash', error: `unknown subtype: ${result.subtype}` };
  }
}
