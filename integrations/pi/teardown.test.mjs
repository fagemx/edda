import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { removeTempTree } from './fixtures/temp-teardown.mjs';

// The residual-handle window is a Windows property: a child process keeps a
// lock on its working directory after it is spawned, and a just-exited child
// can hold that lock for a short while. Both tests below spawn a real child
// with the temp tree as its cwd so the teardown helper is exercised against
// the actual mechanism, not a stub.

test('a temp tree held briefly by a child is torn down by the bounded retry', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'edda-teardown-retry-'));
  const child = spawn(process.execPath, ['-e', 'setTimeout(() => {}, 8000)'], { cwd: dir, stdio: 'ignore' });
  // Release the child's directory lock a short way into the retry window, so
  // teardown must tolerate several failures before it succeeds.
  const killer = setTimeout(() => child.kill(), 300);
  t.after(async () => {
    clearTimeout(killer);
    child.kill();
    // The child is gone; this is cleanup, never the assertion under test.
    await removeTempTree(dir).catch(() => {});
  });

  await removeTempTree(dir);
  assert.equal(existsSync(dir), false);
});

test('a still-held temp tree is not masked by the bounded retry', {
  // On POSIX, removing a directory that is a live process's cwd succeeds, so
  // the retained-handle reproduction is Windows-specific.
  skip: process.platform !== 'win32' && 'a live process cwd does not lock a directory on this platform',
}, async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'edda-teardown-locked-'));
  const child = spawn(process.execPath, ['-e', 'setTimeout(() => {}, 8000)'], { cwd: dir, stdio: 'ignore' });
  t.after(async () => {
    child.kill();
    await removeTempTree(dir).catch(() => {});
  });

  await assert.rejects(
    () => removeTempTree(dir, { delays: [0, 10, 10] }),
    (error) => error.code === 'EBUSY',
  );
});
