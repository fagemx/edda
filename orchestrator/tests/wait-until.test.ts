import { describe, it, expect, vi } from 'vitest';
import { computeDelay, sleep, checkWaitUntil } from '../src/checks/wait-until.js';

// ── computeDelay ──

describe('computeDelay', () => {
  it('none: returns base + jitter', () => {
    const delay = computeDelay(5, 3, 'none');
    // base=5, jitter 0-1s → [5, 6)
    expect(delay).toBeGreaterThanOrEqual(5);
    expect(delay).toBeLessThan(6);
  });

  it('linear: base * attempt capped at 5x', () => {
    // attempt=3, base=10 → 30, but cap=50, so 30 + jitter
    const delay = computeDelay(10, 3, 'linear');
    expect(delay).toBeGreaterThanOrEqual(30);
    expect(delay).toBeLessThan(31);
  });

  it('linear: caps at 5x base', () => {
    // attempt=100, base=10 → min(1000, 50)=50 + jitter
    const delay = computeDelay(10, 100, 'linear');
    expect(delay).toBeGreaterThanOrEqual(50);
    expect(delay).toBeLessThan(51);
  });

  it('exponential: base * 2^(n-1) capped at 10x', () => {
    // attempt=1, base=5 → 5*2^0=5 + jitter
    expect(computeDelay(5, 1, 'exponential')).toBeGreaterThanOrEqual(5);
    // attempt=3, base=5 → 5*2^2=20 + jitter
    expect(computeDelay(5, 3, 'exponential')).toBeGreaterThanOrEqual(20);
  });

  it('exponential: caps at 10x base', () => {
    // attempt=100, base=5 → min(huge, 50)=50 + jitter
    const delay = computeDelay(5, 100, 'exponential');
    expect(delay).toBeGreaterThanOrEqual(50);
    expect(delay).toBeLessThan(51);
  });

  it('unknown backoff defaults to base', () => {
    const delay = computeDelay(7, 5, 'unknown_strategy');
    expect(delay).toBeGreaterThanOrEqual(7);
    expect(delay).toBeLessThan(8);
  });
});

// ── sleep ──

describe('sleep', () => {
  it('resolves after duration', async () => {
    const start = Date.now();
    await sleep(50);
    expect(Date.now() - start).toBeGreaterThanOrEqual(40);
  });

  it('resolves early on abort', async () => {
    const controller = new AbortController();
    const start = Date.now();
    setTimeout(() => controller.abort(), 20);
    await sleep(5000, controller.signal);
    const elapsed = Date.now() - start;
    expect(elapsed).toBeLessThan(1000);
  });

  it('resolves immediately if already aborted', async () => {
    const controller = new AbortController();
    controller.abort();
    const start = Date.now();
    await sleep(5000, controller.signal);
    expect(Date.now() - start).toBeLessThan(50);
  });
});

// ── checkWaitUntil ──

describe('checkWaitUntil', () => {
  it('passes when inner check passes immediately', async () => {
    const result = await checkWaitUntil(
      {
        type: 'wait_until',
        check: { type: 'cmd_succeeds', cmd: 'echo ok' },
        interval_sec: 1,
        timeout_sec: 10,
        max_attempts: 5,
      },
      process.cwd(),
    );
    expect(result.status).toBe('passed');
    expect(result.detail).toContain('1 attempts');
  });

  it('respects max_attempts', async () => {
    // Use a command that always fails
    const result = await checkWaitUntil(
      {
        type: 'wait_until',
        check: { type: 'file_exists', path: 'nonexistent_file_wait_until_test.xyz' },
        interval_sec: 0.1,
        timeout_sec: 60,
        max_attempts: 3,
        backoff: 'none',
      },
      process.cwd(),
    );
    expect(result.status).toBe('failed');
    expect(result.detail).toContain('max attempts');
    expect(result.detail).toContain('3 attempts');
  });

  it('respects timeout', async () => {
    const result = await checkWaitUntil(
      {
        type: 'wait_until',
        check: { type: 'file_exists', path: 'nonexistent_file_wait_until_test.xyz' },
        interval_sec: 0.05,
        timeout_sec: 0.3,
        max_attempts: 1000,
        backoff: 'none',
      },
      process.cwd(),
    );
    expect(result.status).toBe('failed');
    expect(result.detail).toContain('timed out');
    expect(result.duration_ms).toBeLessThan(5000);
  });

  it('aborts early on signal', async () => {
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 100);

    const result = await checkWaitUntil(
      {
        type: 'wait_until',
        check: { type: 'file_exists', path: 'nonexistent_file_wait_until_test.xyz' },
        interval_sec: 10,
        timeout_sec: 60,
        max_attempts: 100,
      },
      process.cwd(),
      controller.signal,
    );
    expect(result.status).toBe('failed');
    expect(result.detail).toContain('aborted');
    expect(result.duration_ms).toBeLessThan(5000);
  });
});
