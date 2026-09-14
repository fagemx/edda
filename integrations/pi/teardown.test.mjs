import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { removeTempTree } from './fixtures/temp-teardown.mjs';

// The residual-handle window is a Windows property: a child process holds its
// working directory until it exits, and a just-exited child can hold it a short
// while longer. Start a child whose cwd is `dir` and return only once that child
// has signalled it is running, so the removals below meet the real lock instead
// of racing the child's start-up.
async function holdDirectory(dir) {
  const child = spawn(process.execPath,
    ['-e', 'process.send && process.send("ready"); setTimeout(() => {}, 8000)'],
    { cwd: dir, stdio: ['ignore', 'ignore', 'ignore', 'ipc'] });
  await new Promise((resolve, reject) => {
    child.once('message', resolve);
    child.once('error', reject);
    child.once('exit', () => reject(new Error('the child exited before signalling readiness')));
  });
  return child;
}

test('a temp tree held by a child is torn down once the child releases it', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'edda-teardown-retry-'));
  const child = await holdDirectory(dir);
  const release = setTimeout(() => child.kill(), 250);
  t.after(async () => { clearTimeout(release); child.kill(); await removeTempTree(dir).catch(() => {}); });

  await removeTempTree(dir);
  assert.equal(existsSync(dir), false);
});

test('a still-held temp tree is not masked by the bounded retry', {
  // On POSIX a live process's cwd does not lock the directory, so the retained
  // handle can only reproduce on Windows; the injected test below covers the
  // exhausted window on every platform.
  skip: process.platform !== 'win32' && 'a live process cwd does not lock a directory on this platform',
}, async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'edda-teardown-locked-'));
  const child = await holdDirectory(dir);
  t.after(async () => { child.kill(); await removeTempTree(dir).catch(() => {}); });

  await assert.rejects(
    () => removeTempTree(dir, { delays: [0, 10, 10] }),
    (error) => error.code === 'EBUSY',
  );
});

test('an exhausted retry window rethrows after using the whole window', async () => {
  let calls = 0;
  const remove = async () => {
    calls += 1;
    const error = new Error('EBUSY: resource busy or locked, rmdir');
    error.code = 'EBUSY';
    throw error;
  };
  await assert.rejects(
    () => removeTempTree('C:/held/tree', { remove, sleep: async () => {}, delays: [0, 10, 10] }),
    (error) => error.code === 'EBUSY',
  );
  assert.equal(calls, 3, 'the bounded window was fully used before giving up');
});
