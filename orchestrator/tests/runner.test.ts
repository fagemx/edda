import { describe, it, expect } from 'vitest';
import {
  findNextPhase,
  isPlanBlocked,
  isPlanComplete,
} from '../src/runner/find-next.js';
import { buildPrompt, buildPlanContext } from '../src/runner/prompt.js';
import { classifyResult } from '../src/runner/classify.js';
import { DEFAULT_TOOLS } from '../src/runner/types.js';
import { initPlanState, transition } from '../src/plan/state.js';
import type { Plan } from '../src/plan/types.js';

function makePlan(phases: Array<{ id: string; depends_on?: string[] }>): Plan {
  return {
    name: 'test-plan',
    phases: phases.map((p) => ({
      id: p.id,
      prompt: `Do ${p.id}`,
      depends_on: p.depends_on ?? [],
      check: [],
      max_attempts: 3,
      timeout_sec: 1800,
      env: {},
      on_fail: 'ask' as const,
      permission_mode: 'bypassPermissions',
    })),
    max_attempts: 3,
    timeout_sec: 1800,
    env: {},
    on_fail: 'ask' as const,
    tags: [],
  };
}

// ── findNextPhase ──

describe('findNextPhase', () => {
  it('returns first pending phase in order', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }, { id: 'c' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    expect(findNextPhase(plan, state, ['a', 'b', 'c'])).toBe('a');
  });

  it('skips passed phases', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed');
    expect(findNextPhase(plan, state, ['a', 'b'])).toBe('b');
  });

  it('respects depends_on', () => {
    const plan = makePlan([
      { id: 'a' },
      { id: 'b', depends_on: ['a'] },
    ]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    // 'b' depends on 'a' which is still pending
    expect(findNextPhase(plan, state, ['a', 'b'])).toBe('a');
  });

  it('blocks when dependency not satisfied', () => {
    const plan = makePlan([
      { id: 'a' },
      { id: 'b', depends_on: ['a'] },
    ]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'failed');
    // 'a' is failed, 'b' depends on 'a' → no runnable phase
    expect(findNextPhase(plan, state, ['a', 'b'])).toBeNull();
  });

  it('allows skipped dependency', () => {
    const plan = makePlan([
      { id: 'a' },
      { id: 'b', depends_on: ['a'] },
    ]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'skipped');
    expect(findNextPhase(plan, state, ['a', 'b'])).toBe('b');
  });

  it('returns null when all done', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed');
    expect(findNextPhase(plan, state, ['a'])).toBeNull();
  });
});

// ── isPlanBlocked / isPlanComplete ──

describe('isPlanBlocked', () => {
  it('returns true when a phase is failed', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'failed');
    expect(isPlanBlocked(state)).toBe(true);
  });

  it('returns false when no failures', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    expect(isPlanBlocked(state)).toBe(false);
  });
});

describe('isPlanComplete', () => {
  it('returns true when all passed', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed');
    transition(state, 'b', 'pending', 'skipped');
    expect(isPlanComplete(state)).toBe(true);
  });

  it('returns false when some pending', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed');
    expect(isPlanComplete(state)).toBe(false);
  });
});

// ── buildPrompt ──

describe('buildPrompt', () => {
  it('returns prompt only when no context', () => {
    expect(buildPrompt({ prompt: 'Do something' })).toBe('Do something');
  });

  it('prepends context before prompt', () => {
    const result = buildPrompt({
      prompt: 'Build the API',
      context: 'Use PostgreSQL',
    });
    expect(result).toBe('Use PostgreSQL\n\nBuild the API');
  });
});

// ── buildPlanContext ──

describe('buildPlanContext', () => {
  it('includes plan name and phase position', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    const ctx = buildPlanContext(plan, state, 'a');
    expect(ctx).toContain('test-plan');
    expect(ctx).toContain('Phase 1/2: a');
  });

  it('shows completed phases', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running', { started_at: '2026-01-01T00:00:00Z' });
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed', { completed_at: '2026-01-01T00:01:00Z' });
    const ctx = buildPlanContext(plan, state, 'b');
    expect(ctx).toContain('Phase 2/2: b');
    expect(ctx).toContain('"a": completed');
  });
});

// ── classifyResult ──

describe('classifyResult', () => {
  it('classifies success', () => {
    const result = classifyResult({ type: 'result', subtype: 'success', total_cost_usd: 0.5 });
    expect(result).toEqual({ type: 'agent_done', cost_usd: 0.5 });
  });

  it('classifies max_turns', () => {
    const result = classifyResult({ type: 'result', subtype: 'error_max_turns', total_cost_usd: 2.0 });
    expect(result).toEqual({ type: 'max_turns', cost_usd: 2.0 });
  });

  it('classifies budget_exceeded', () => {
    const result = classifyResult({ type: 'result', subtype: 'error_max_budget_usd', total_cost_usd: 10.0 });
    expect(result).toEqual({ type: 'budget_exceeded', cost_usd: 10.0 });
  });

  it('classifies execution error', () => {
    const result = classifyResult({
      type: 'result',
      subtype: 'error_during_execution',
      error: 'something broke',
    });
    expect(result).toEqual({ type: 'agent_crash', error: 'something broke' });
  });

  it('handles null/undefined message', () => {
    expect(classifyResult(null).type).toBe('agent_crash');
    expect(classifyResult(undefined).type).toBe('agent_crash');
  });

  it('handles unknown subtype', () => {
    const result = classifyResult({ type: 'result', subtype: 'unknown_thing' });
    expect(result.type).toBe('agent_crash');
  });
});

// ── DEFAULT_TOOLS ──

describe('DEFAULT_TOOLS', () => {
  it('includes standard tools', () => {
    expect(DEFAULT_TOOLS).toContain('Read');
    expect(DEFAULT_TOOLS).toContain('Write');
    expect(DEFAULT_TOOLS).toContain('Edit');
    expect(DEFAULT_TOOLS).toContain('Bash');
  });
});
