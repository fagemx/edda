import { describe, it, expect, beforeEach } from 'vitest';
import { join, resolve } from 'node:path';
import { mkdirSync, writeFileSync, readFileSync, existsSync } from 'node:fs';
import { testContext } from './helpers/test-context.js';
import {
  initPlanState,
  savePlanState,
  loadPlanState,
  transition,
  getPhase,
  listPlanStates,
  findPlanForPhase,
  statePath,
} from '../src/plan/state.js';
import type { Plan } from '../src/plan/types.js';

const ctx = testContext();

function makePlan(name: string, phases: Array<{ id: string; depends_on?: string[] }>): Plan {
  return {
    name,
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

// ── listPlanStates ──

describe('listPlanStates', () => {
  it('returns empty array when no state dir exists', () => {
    const dir = ctx.createTempDir();
    const states = listPlanStates(dir);
    expect(states).toEqual([]);
  });

  it('finds all plan states in orchestrator dir', () => {
    const dir = ctx.createTempDir();
    const orchestratorDir = join(dir, '.edda', 'orchestrator');

    // Create two plan states
    const plan1 = makePlan('plan-a', [{ id: 'a1' }]);
    const state1 = initPlanState(plan1, '/tmp/plan-a.yaml');
    const dir1 = join(orchestratorDir, 'plan-a');
    mkdirSync(dir1, { recursive: true });
    savePlanState(state1, join(dir1, 'plan-state.json'));

    const plan2 = makePlan('plan-b', [{ id: 'b1' }, { id: 'b2' }]);
    const state2 = initPlanState(plan2, '/tmp/plan-b.yaml');
    const dir2 = join(orchestratorDir, 'plan-b');
    mkdirSync(dir2, { recursive: true });
    savePlanState(state2, join(dir2, 'plan-state.json'));

    const states = listPlanStates(dir);
    expect(states).toHaveLength(2);
    const names = states.map((s) => s.plan_name).sort();
    expect(names).toEqual(['plan-a', 'plan-b']);
  });

  it('ignores corrupted state files', () => {
    const dir = ctx.createTempDir();
    const orchestratorDir = join(dir, '.edda', 'orchestrator');

    // Valid plan
    const plan = makePlan('good-plan', [{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/good.yaml');
    const goodDir = join(orchestratorDir, 'good-plan');
    mkdirSync(goodDir, { recursive: true });
    savePlanState(state, join(goodDir, 'plan-state.json'));

    // Corrupted plan
    const badDir = join(orchestratorDir, 'bad-plan');
    mkdirSync(badDir, { recursive: true });
    writeFileSync(join(badDir, 'plan-state.json'), 'not-json{{{');

    const states = listPlanStates(dir);
    expect(states).toHaveLength(1);
    expect(states[0]!.plan_name).toBe('good-plan');
  });
});

// ── findPlanForPhase ──

describe('findPlanForPhase', () => {
  it('finds plan containing the phase', () => {
    const dir = ctx.createTempDir();
    const orchestratorDir = join(dir, '.edda', 'orchestrator');

    const plan = makePlan('my-plan', [{ id: 'setup' }, { id: 'deploy' }]);
    const state = initPlanState(plan, '/tmp/my-plan.yaml');
    const planDir = join(orchestratorDir, 'my-plan');
    mkdirSync(planDir, { recursive: true });
    savePlanState(state, join(planDir, 'plan-state.json'));

    const found = findPlanForPhase('deploy', undefined, dir);
    expect(found).not.toBeNull();
    expect(found!.plan_name).toBe('my-plan');
  });

  it('returns null when phase not found', () => {
    const dir = ctx.createTempDir();
    const orchestratorDir = join(dir, '.edda', 'orchestrator');

    const plan = makePlan('my-plan', [{ id: 'setup' }]);
    const state = initPlanState(plan, '/tmp/my-plan.yaml');
    const planDir = join(orchestratorDir, 'my-plan');
    mkdirSync(planDir, { recursive: true });
    savePlanState(state, join(planDir, 'plan-state.json'));

    const found = findPlanForPhase('nonexistent', undefined, dir);
    expect(found).toBeNull();
  });

  it('filters by plan name when specified', () => {
    const dir = ctx.createTempDir();
    const orchestratorDir = join(dir, '.edda', 'orchestrator');

    // Two plans both have a phase "setup"
    for (const name of ['plan-a', 'plan-b']) {
      const plan = makePlan(name, [{ id: 'setup' }]);
      const state = initPlanState(plan, `/tmp/${name}.yaml`);
      const planDir = join(orchestratorDir, name);
      mkdirSync(planDir, { recursive: true });
      savePlanState(state, join(planDir, 'plan-state.json'));
    }

    const found = findPlanForPhase('setup', 'plan-b', dir);
    expect(found).not.toBeNull();
    expect(found!.plan_name).toBe('plan-b');
  });
});

// ── Retry logic (state transitions) ──

describe('retry logic', () => {
  it('resets failed phase to pending', () => {
    const plan = makePlan('test', [{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'failed');

    const phase = getPhase(state, 'a');
    expect(phase.status).toBe('failed');

    transition(state, 'a', 'failed', 'pending');
    expect(phase.status).toBe('pending');
  });

  it('resets stale phase to pending', () => {
    const plan = makePlan('test', [{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'stale');

    transition(state, 'a', 'stale', 'pending');
    expect(getPhase(state, 'a').status).toBe('pending');
  });

  it('cannot retry a passed phase', () => {
    const plan = makePlan('test', [{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed');

    // 'passed' has no valid transitions
    expect(() => transition(state, 'a', 'passed', 'pending')).toThrow('invalid transition');
  });
});

// ── Skip logic ──

describe('skip logic', () => {
  it('marks pending phase as skipped', () => {
    const plan = makePlan('test', [{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');

    transition(state, 'a', 'pending', 'skipped');
    const phase = getPhase(state, 'a');
    expect(phase.status).toBe('skipped');
  });

  it('allows skip of failed phase (via pending reset)', () => {
    const plan = makePlan('test', [{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'failed');

    // Reset to pending first, then skip
    transition(state, 'a', 'failed', 'pending');
    transition(state, 'a', 'pending', 'skipped');
    expect(getPhase(state, 'a').status).toBe('skipped');
  });

  it('cannot skip a passed phase', () => {
    const plan = makePlan('test', [{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed');

    expect(() => transition(state, 'a', 'passed', 'skipped')).toThrow('invalid transition');
  });
});

// ── Abort logic ──

describe('abort logic', () => {
  it('marks plan as aborted', () => {
    const plan = makePlan('test', [{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');

    state.status = 'aborted';
    state.aborted_at = new Date().toISOString();

    expect(state.status).toBe('aborted');
    expect(state.aborted_at).toBeTruthy();
  });

  it('preserves phase states on abort', () => {
    const plan = makePlan('test', [{ id: 'a' }, { id: 'b' }, { id: 'c' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed');
    transition(state, 'b', 'pending', 'running');

    state.status = 'aborted';

    expect(getPhase(state, 'a').status).toBe('passed');
    expect(getPhase(state, 'b').status).toBe('running');
    expect(getPhase(state, 'c').status).toBe('pending');
  });
});

// ── Debug utility ──

describe('debug utility', () => {
  it('exports debug function', async () => {
    const { debug } = await import('../src/lib/utils/debug.js');
    expect(typeof debug).toBe('function');
    // Should not throw when called
    debug('test', 'hello');
  });
});

// ── withErrorHandler ──

describe('withErrorHandler', () => {
  it('exports withErrorHandler function', async () => {
    const { withErrorHandler } = await import('../src/lib/command/with-error-handler.js');
    expect(typeof withErrorHandler).toBe('function');
  });
});
