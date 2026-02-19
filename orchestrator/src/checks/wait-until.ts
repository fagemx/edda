import { runCheck, type CheckOutput } from './engine.js';

// ── Backoff strategies ──

/**
 * Compute delay in seconds for the current attempt.
 * Includes small jitter (0-1s) to avoid synchronized polling.
 */
export function computeDelay(
  base: number,
  attempt: number,
  backoff: string,
): number {
  let delay: number;

  switch (backoff) {
    case 'none':
      delay = base;
      break;
    case 'linear':
      delay = Math.min(base * attempt, base * 5);
      break;
    case 'exponential':
      delay = Math.min(base * Math.pow(2, attempt - 1), base * 10);
      break;
    default:
      delay = base;
  }

  // Jitter: add 0-1s to avoid thundering herd
  const jitterMs = Date.now() % 1000;
  return delay + jitterMs / 1000;
}

// ── Interruptable sleep ──

/**
 * Sleep for the given duration in ms.
 * Resolves early if the signal is aborted.
 */
export function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal?.aborted) {
      resolve();
      return;
    }

    const timer = setTimeout(resolve, ms);

    signal?.addEventListener('abort', () => {
      clearTimeout(timer);
      resolve();
    }, { once: true });
  });
}

// ── wait_until executor ──

export interface WaitUntilSpec {
  type: 'wait_until';
  check: Record<string, unknown>;
  interval_sec?: number;
  timeout_sec?: number;
  backoff?: string;
  max_attempts?: number;
}

/**
 * Poll an inner check until it passes or timeout/max_attempts is reached.
 */
export async function checkWaitUntil(
  spec: WaitUntilSpec,
  cwd: string,
  signal?: AbortSignal,
): Promise<CheckOutput> {
  const interval = spec.interval_sec ?? 30;
  const timeout = spec.timeout_sec ?? 600;
  const backoff = spec.backoff ?? 'linear';
  const maxAttempts = spec.max_attempts ?? Infinity;
  const innerCheck = spec.check;
  const deadline = Date.now() + timeout * 1000;
  const start = Date.now();
  let attempt = 0;

  while (Date.now() < deadline && attempt < maxAttempts) {
    if (signal?.aborted) {
      return {
        status: 'failed',
        detail: `aborted after ${attempt} attempts`,
        duration_ms: Date.now() - start,
      };
    }

    attempt++;
    const result = await runCheck(innerCheck, cwd);

    if (result.status === 'passed') {
      return {
        status: 'passed',
        detail: `passed after ${attempt} attempts`,
        duration_ms: Date.now() - start,
      };
    }

    // Check deadline before sleeping
    const remaining = deadline - Date.now();
    if (remaining <= 0) break;

    const delaySec = computeDelay(interval, attempt, backoff);
    const sleepMs = Math.min(delaySec * 1000, remaining);
    await sleep(sleepMs, signal);
  }

  // Determine failure reason
  if (attempt >= maxAttempts) {
    return {
      status: 'failed',
      detail: `max attempts reached (${maxAttempts} attempts in ${Math.round((Date.now() - start) / 1000)}s)`,
      duration_ms: Date.now() - start,
    };
  }

  return {
    status: 'failed',
    detail: `timed out after ${timeout}s (${attempt} attempts)`,
    duration_ms: Date.now() - start,
  };
}
