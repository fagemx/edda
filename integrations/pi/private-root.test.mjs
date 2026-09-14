import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { privateRoot, runPrivateDirectoryAcl, PrivateRootError } from './store.mjs';

// The exact error Node raises when `spawnSync` kills a PowerShell that overshot
// its timeout — the failure GH #1204 reproduced on base `main` (run 34799835049,
// job `manager (windows-latest)`).
function aclTimeout() {
  const error = new Error('spawnSync powershell.exe ETIMEDOUT');
  error.code = 'ETIMEDOUT';
  return error;
}

test('a transient ACL timeout is retried and the secure step is applied', () => {
  let calls = 0;
  const sleeps = [];
  const seen = [];
  const io = {
    run: (script, root, timeout) => { calls += 1; seen.push({ script, root, timeout }); if (calls <= 2) throw aclTimeout(); },
    sleep: (ms) => sleeps.push(ms),
  };
  runPrivateDirectoryAcl('C:/registry', io);
  assert.equal(calls, 3, 'two timeouts then a success');
  assert.deepEqual(sleeps, [500, 1500], 'the bounded backoff pauses were used');
  assert.deepEqual(seen.map(({ timeout }) => timeout), [12000, 6000, 6000],
    'the per-attempt timeout shrinks so the whole window is ~24 s, not three full 15 s attempts');
  assert.ok(seen.every(({ script, root }) => /private-directory\.ps1$/.test(script) && root === 'C:/registry'),
    'every attempt ran the real ACL script against the requested root');
});

test('an exhausted window throws a typed failure and never reports success', () => {
  let calls = 0;
  const sleeps = [];
  assert.throws(
    () => runPrivateDirectoryAcl('C:/registry', { run: () => { calls += 1; throw aclTimeout(); }, sleep: (ms) => sleeps.push(ms) }),
    (error) => error instanceof PrivateRootError
      && error.code === 'private_root_failed'
      && error.errno === 'ETIMEDOUT'
      && error.root === 'C:/registry'
      && error.cause?.code === 'ETIMEDOUT',
  );
  assert.equal(calls, 3, 'the bounded window was fully used before giving up');
  assert.deepEqual(sleeps, [500, 1500]);
});

test('a real ACL refusal is thrown unchanged and not retried', () => {
  const refusal = new Error('Channel ACL permits another principal');
  refusal.status = 1;
  refusal.stderr = Buffer.from('Channel ACL permits another principal\n');
  let calls = 0;
  let slept = false;
  assert.throws(
    () => runPrivateDirectoryAcl('C:/registry', { run: () => { calls += 1; throw refusal; }, sleep: () => { slept = true; } }),
    (error) => error === refusal,
  );
  assert.equal(calls, 1, 'a non-timeout failure is not a transient condition');
  assert.equal(slept, false);
});

test('privateRoot retries the Windows ACL through the injected step', {
  // `privateRoot` only takes the ACL branch on Windows; `runPrivateDirectoryAcl`
  // above covers the retry policy on every platform.
  skip: process.platform !== 'win32' && 'the ACL step runs only on win32',
}, () => {
  const dir = mkdtempSync(join(tmpdir(), 'pi-private-root-'));
  let calls = 0;
  try {
    assert.equal(privateRoot(dir, { run: () => { calls += 1; if (calls <= 2) throw aclTimeout(); }, sleep: () => {} }), resolve(dir));
    assert.equal(calls, 3);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
