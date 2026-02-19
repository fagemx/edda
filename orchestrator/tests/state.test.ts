import { describe, it, expect } from 'vitest';
import { join } from 'node:path';
import { readFileSync, existsSync } from 'node:fs';
import { testContext } from './helpers/test-context.js';
import {
  initPlanState,
  savePlanState,
  loadPlanState,
  transition,
  derivePlanStatus,
  detectStalePhases,
  getPhase,
  nextPendingPhase,
  buildRunnerStatus,
  type PlanState,
  type PhaseState,
} from '../src/plan/state.js';
import type { Plan } from '../src/plan/types.js';

const ctx = testContext();

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

// ── initPlanState ──

describe('initPlanState', () => {
  it('creates state with all phases pending', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    expect(state.plan_name).toBe('test-plan');
    expect(state.plan_file).toBe('/tmp/plan.yaml');
    expect(state.status).toBe('pending');
    expect(state.phases).toHaveLength(2);
    expect(state.phases[0]!.status).toBe('pending');
    expect(state.phases[0]!.attempts).toBe(0);
    expect(state.version).toBe(1);
  });
});

// ── derivePlanStatus ──

describe('derivePlanStatus', () => {
  function phase(status: PhaseState['status']): PhaseState {
    return { id: 'x', status, started_at: null, completed_at: null, attempts: 0, checks: [], error: null };
  }

  it('returns pending when all pending', () => {
    expect(derivePlanStatus([phase('pending'), phase('pending')])).toBe('pending');
  });

  it('returns running when any running', () => {
    expect(derivePlanStatus([phase('passed'), phase('running')])).toBe('running');
  });

  it('returns running when any checking', () => {
    expect(derivePlanStatus([phase('passed'), phase('checking')])).toBe('running');
  });

  it('returns blocked when any failed', () => {
    expect(derivePlanStatus([phase('passed'), phase('failed')])).toBe('blocked');
  });

  it('returns blocked when any stale', () => {
    expect(derivePlanStatus([phase('passed'), phase('stale')])).toBe('blocked');
  });

  it('returns completed when all passed or skipped', () => {
    expect(derivePlanStatus([phase('passed'), phase('skipped')])).toBe('completed');
  });
});

// ── transition (CAS guard) ──

describe('transition', () => {
  it('applies valid transition', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    const ok = transition(state, 'a', 'pending', 'running', {
      started_at: '2026-01-01T00:00:00Z',
      attempts: 1,
    });
    expect(ok).toBe(true);
    expect(getPhase(state, 'a').status).toBe('running');
    expect(getPhase(state, 'a').attempts).toBe(1);
    expect(state.status).toBe('running');
  });

  it('rejects when current state does not match from', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    const ok = transition(state, 'a', 'running', 'checking');
    expect(ok).toBe(false);
    expect(getPhase(state, 'a').status).toBe('pending');
  });

  it('throws on invalid transition', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    // pending → checking is not allowed (must go through running)
    expect(() => transition(state, 'a', 'pending', 'checking')).toThrow('invalid transition');
  });

  it('throws on unknown phase', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    expect(() => transition(state, 'nonexistent', 'pending', 'running')).toThrow('not found');
  });

  it('failed → pending (retry)', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'failed');
    const ok = transition(state, 'a', 'failed', 'pending');
    expect(ok).toBe(true);
    expect(getPhase(state, 'a').status).toBe('pending');
  });

  it('preserves aborted status', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    state.status = 'aborted';
    transition(state, 'a', 'pending', 'skipped');
    expect(state.status).toBe('aborted');
  });
});

// ── persistence ──

describe('savePlanState / loadPlanState', () => {
  it('round-trips state through JSON', () => {
    const dir = ctx.createTempDir();
    const path = join(dir, 'plan-state.json');

    const plan = makePlan([{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running', { started_at: '2026-01-01T00:00:00Z', attempts: 1 });

    savePlanState(state, path);
    const loaded = loadPlanState(path);

    expect(loaded).not.toBeNull();
    expect(loaded!.plan_name).toBe('test-plan');
    expect(loaded!.phases[0]!.status).toBe('running');
    expect(loaded!.phases[0]!.attempts).toBe(1);
  });

  it('returns null for missing file', () => {
    expect(loadPlanState('/nonexistent/path.json')).toBeNull();
  });

  it('returns null for corrupt file', () => {
    const dir = ctx.createTempDir();
    const path = join(dir, 'bad.json');
    const { writeFileSync } = require('node:fs');
    writeFileSync(path, '{ corrupt json !!!');
    expect(loadPlanState(path)).toBeNull();
  });

  it('atomic write creates .tmp then renames', () => {
    const dir = ctx.createTempDir();
    const path = join(dir, 'state.json');
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');

    savePlanState(state, path);
    expect(existsSync(path)).toBe(true);
    // .tmp should not remain
    expect(existsSync(path + '.tmp')).toBe(false);
  });
});

// ── stale detection ──

describe('detectStalePhases', () => {
  it('marks running phase as stale when past timeout', () => {
    const plan = makePlan([{ id: 'a' }]);
    plan.phases[0]!.timeout_sec = 60;
    const state = initPlanState(plan, '/tmp/plan.yaml');

    // Simulate: phase started 2 hours ago
    state.phases[0]!.status = 'running';
    state.phases[0]!.started_at = new Date(Date.now() - 7200_000).toISOString();

    detectStalePhases(state, plan);
    expect(state.phases[0]!.status).toBe('stale');
    expect(state.phases[0]!.error?.type).toBe('timeout');
    expect(state.phases[0]!.error?.retryable).toBe(true);
    expect(state.status).toBe('blocked');
  });

  it('does not mark running phase within timeout', () => {
    const plan = makePlan([{ id: 'a' }]);
    plan.phases[0]!.timeout_sec = 1800;
    const state = initPlanState(plan, '/tmp/plan.yaml');

    state.phases[0]!.status = 'running';
    state.phases[0]!.started_at = new Date().toISOString();

    detectStalePhases(state, plan);
    expect(state.phases[0]!.status).toBe('running');
  });
});

// ── buildRunnerStatus ──

describe('buildRunnerStatus', () => {
  it('returns running status with current phase', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    const rs = buildRunnerStatus(state);
    expect(rs.mode).toBe('running');
    expect(rs.current_phase).toBe('a');
    expect(rs.phases_completed).toBe(0);
    expect(rs.phases_total).toBe(2);
  });

  it('returns completed when all done', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed');
    const rs = buildRunnerStatus(state);
    expect(rs.mode).toBe('completed');
    expect(rs.phases_completed).toBe(1);
    expect(rs.current_phase).toBeNull();
  });
});

// ── nextPendingPhase ──

describe('nextPendingPhase', () => {
  it('returns first pending in topo order', () => {
    const plan = makePlan([{ id: 'a' }, { id: 'b' }, { id: 'c' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'running');
    transition(state, 'a', 'running', 'checking');
    transition(state, 'a', 'checking', 'passed');
    expect(nextPendingPhase(state, ['a', 'b', 'c'])).toBe('b');
  });

  it('returns null when none pending', () => {
    const plan = makePlan([{ id: 'a' }]);
    const state = initPlanState(plan, '/tmp/plan.yaml');
    transition(state, 'a', 'pending', 'skipped');
    expect(nextPendingPhase(state, ['a'])).toBeNull();
  });
});
