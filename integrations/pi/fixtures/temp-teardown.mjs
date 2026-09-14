import { rm } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';

// A Windows child that has exited can still hold its old working directory for a
// short window, and a scanner can hold a freshly written file; a recursive
// rmdir then fails with EBUSY/EPERM even though nothing leaked. Retry a bounded
// number of times, then rethrow so a genuine leak still fails the test.
const RETRYABLE_RM = new Set(['EBUSY', 'EPERM', 'ENOTEMPTY', 'EMFILE', 'ENFILE']);
export const RM_RETRY_MS = [0, 50, 100, 200, 400, 800, 1000, 1000, 1000];

export async function removeTempTree(path, { remove = rm, sleep = delay, delays = RM_RETRY_MS } = {}) {
  for (let attempt = 0; ; attempt += 1) {
    try { await remove(path, { recursive: true, force: true }); return; }
    catch (error) {
      if (!RETRYABLE_RM.has(error.code) || attempt >= delays.length - 1) throw error;
      await sleep(delays[attempt + 1]);
    }
  }
}
