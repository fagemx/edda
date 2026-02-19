import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { randomUUID } from 'node:crypto';
import { afterEach } from 'vitest';

/**
 * Create a test context with auto-cleanup.
 * Provides temp dirs, abort signal, and unique IDs.
 * All resources are cleaned up automatically in afterEach.
 */
export function testContext() {
  const abortController = new AbortController();
  const tempDirs: string[] = [];

  afterEach(() => {
    abortController.abort();
    for (const dir of tempDirs) {
      try {
        rmSync(dir, { recursive: true, force: true });
      } catch {
        // EBUSY on Windows — child process may still hold locks. Ignore.
      }
    }
  });

  return {
    signal: abortController.signal,
    createTempDir(): string {
      const dir = mkdtempSync(join(tmpdir(), 'orchestrate-test-'));
      tempDirs.push(dir);
      return dir;
    },
    uniqueId(prefix = 'test'): string {
      return `${prefix}-${randomUUID().slice(0, 8)}`;
    },
  };
}
