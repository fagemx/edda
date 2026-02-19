import { describe, it, expect, beforeEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { testContext } from './helpers/test-context.js';
import {
  recordEvent,
  resetEventSequence,
  formatEventMessage,
  formatEventTags,
  type OrchestratorEvent,
  type EventWriterOptions,
} from '../src/integration/ledger.js';

const ctx = testContext();

function makeOptions(dir: string): EventWriterOptions {
  return {
    eddaEnabled: false, // Don't call real edda in tests
    jsonLogPath: join(dir, 'events.jsonl'),
  };
}

function readEvents(path: string): OrchestratorEvent[] {
  return readFileSync(path, 'utf-8')
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

beforeEach(() => {
  resetEventSequence();
});

// ── recordEvent ──

describe('recordEvent', () => {
  it('writes JSONL with seq and ts', () => {
    const dir = ctx.createTempDir();
    const opts = makeOptions(dir);

    recordEvent(opts, {
      type: 'plan:start',
      plan_name: 'test-plan',
      phase_count: 3,
      plan_file: '/tmp/plan.yaml',
    });

    const events = readEvents(opts.jsonLogPath);
    expect(events).toHaveLength(1);
    expect(events[0]!.type).toBe('plan:start');
    expect(events[0]!.seq).toBe(0);
    expect(events[0]!.ts).toBeTruthy();
  });

  it('increments sequence number', () => {
    const dir = ctx.createTempDir();
    const opts = makeOptions(dir);

    recordEvent(opts, {
      type: 'plan:start',
      plan_name: 'test',
      phase_count: 1,
      plan_file: '/tmp/plan.yaml',
    });
    recordEvent(opts, {
      type: 'phase:passed',
      plan_name: 'test',
      phase_id: 'step1',
      duration_ms: 5000,
      attempts: 1,
    });

    const events = readEvents(opts.jsonLogPath);
    expect(events[0]!.seq).toBe(0);
    expect(events[1]!.seq).toBe(1);
  });

  it('creates parent directories for JSONL', () => {
    const dir = ctx.createTempDir();
    const opts = {
      eddaEnabled: false,
      jsonLogPath: join(dir, 'nested', 'deep', 'events.jsonl'),
    };

    recordEvent(opts, {
      type: 'plan:start',
      plan_name: 'test',
      phase_count: 1,
      plan_file: '/tmp/plan.yaml',
    });

    const events = readEvents(opts.jsonLogPath);
    expect(events).toHaveLength(1);
  });
});

// ── formatEventMessage ──

describe('formatEventMessage', () => {
  it('formats plan:start', () => {
    const msg = formatEventMessage({
      type: 'plan:start',
      plan_name: 'add-auth',
      phase_count: 3,
      plan_file: '/tmp/plan.yaml',
      seq: 0,
      ts: '',
    });
    expect(msg).toBe('plan:start add-auth (3 phases)');
  });

  it('formats phase:passed with duration', () => {
    const msg = formatEventMessage({
      type: 'phase:passed',
      plan_name: 'add-auth',
      phase_id: 'schema',
      duration_ms: 105_000,
      attempts: 1,
      seq: 1,
      ts: '',
    });
    expect(msg).toBe('phase:passed add-auth/schema (1m 45s, attempt 1)');
  });

  it('formats phase:failed', () => {
    const msg = formatEventMessage({
      type: 'phase:failed',
      plan_name: 'add-auth',
      phase_id: 'api',
      duration_ms: 252_000,
      attempts: 2,
      error_type: 'check_failed',
      seq: 2,
      ts: '',
    });
    expect(msg).toBe('phase:failed add-auth/api — check_failed');
  });

  it('formats phase:skipped', () => {
    const msg = formatEventMessage({
      type: 'phase:skipped',
      plan_name: 'add-auth',
      phase_id: 'schema',
      skip_reason: 'Schema already exists',
      seq: 3,
      ts: '',
    });
    expect(msg).toBe('phase:skipped add-auth/schema — Schema already exists');
  });

  it('formats plan:completed', () => {
    const msg = formatEventMessage({
      type: 'plan:completed',
      plan_name: 'add-auth',
      duration_ms: 521_000,
      phases_passed: 3,
      phases_skipped: 0,
      total_attempts: 4,
      seq: 4,
      ts: '',
    });
    expect(msg).toBe('plan:completed add-auth (8m 41s, 3 passed)');
  });

  it('formats plan:completed with skipped phases', () => {
    const msg = formatEventMessage({
      type: 'plan:completed',
      plan_name: 'test',
      duration_ms: 60_000,
      phases_passed: 2,
      phases_skipped: 1,
      total_attempts: 3,
      seq: 5,
      ts: '',
    });
    expect(msg).toContain('2 passed');
    expect(msg).toContain('1 skipped');
  });

  it('formats plan:aborted', () => {
    const msg = formatEventMessage({
      type: 'plan:aborted',
      plan_name: 'add-auth',
      phases_passed: 1,
      phases_pending: 2,
      seq: 6,
      ts: '',
    });
    expect(msg).toBe('plan:aborted add-auth (1 passed, 2 pending)');
  });
});

// ── formatEventTags ──

describe('formatEventTags', () => {
  it('includes orchestrator and plan tags', () => {
    const tags = formatEventTags({
      type: 'plan:start',
      plan_name: 'add-auth',
      phase_count: 3,
      plan_file: '/tmp/plan.yaml',
      seq: 0,
      ts: '',
    });
    expect(tags).toContain('orchestrator');
    expect(tags).toContain('plan:add-auth');
  });

  it('includes phase tag for phase events', () => {
    const tags = formatEventTags({
      type: 'phase:passed',
      plan_name: 'add-auth',
      phase_id: 'schema',
      duration_ms: 5000,
      attempts: 1,
      seq: 1,
      ts: '',
    });
    expect(tags).toContain('phase:schema');
  });

  it('does not include phase tag for plan events', () => {
    const tags = formatEventTags({
      type: 'plan:start',
      plan_name: 'test',
      phase_count: 1,
      plan_file: '/tmp/plan.yaml',
      seq: 0,
      ts: '',
    });
    expect(tags).not.toContain(expect.stringContaining('phase:'));
  });
});
