import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { startChannel } from './channel.mjs';

const exec = promisify(execFile), cli = fileURLToPath(new URL('./cli.mjs', import.meta.url));
const run = async (args, env) => {
  try { return { status: 0, ...(await exec(process.execPath, [cli, ...args], { encoding: 'utf8', env: { ...process.env, ...env } })) }; }
  catch (error) { return { status: typeof error.code === 'number' ? error.code : 1, stdout: error.stdout || '', stderr: error.stderr || '' }; }
};

test('send, receipt and list address a session in another registry with --registry', async (t) => {
  const registryA = await mkdtemp(join(tmpdir(), 'edda-registry-a-'));
  const registryB = await mkdtemp(join(tmpdir(), 'edda-registry-b-'));
  const messages = [];
  const channel = await startChannel({ root: registryA, sessionId: randomUUID(), cwd: registryA,
    deliver: (text) => messages.push(text) });
  t.after(async () => {
    await channel.close();
    await rm(registryA, { recursive: true, force: true });
    await rm(registryB, { recursive: true, force: true });
  });
  const sid = channel.sessionId, env = { EDDA_PI_CHANNEL_DIR: registryB };

  // The default registry is B; the session lives in A, so the failure must name
  // the registry rather than a bare "no reachable registered owner".
  const miss = await run(['send', sid, '--message', 'nope', '--id', randomUUID()], env);
  assert.equal(miss.status, 1);
  assert.match(miss.stderr, /No session .* in registry/);
  assert.match(miss.stderr, /--registry/);
  assert.equal(messages.length, 0);

  // --registry A addresses the peer registry explicitly.
  const id = randomUUID();
  const hit = await run(['send', sid, '--message', 'cross-registry', '--id', id, '--registry', registryA], env);
  assert.equal(hit.status, 0, hit.stderr);
  assert.equal(messages.length, 1);
  assert.match(messages[0], /cross-registry/);

  const receipt = await run(['receipt', sid, '--id', id, '--registry', registryA], env);
  assert.equal(receipt.status, 0, receipt.stderr);
  assert.match(receipt.stdout, new RegExp(id));
  const list = await run(['list', '--registry', registryA], env);
  assert.equal(list.status, 0, list.stderr);
  assert.match(list.stdout, new RegExp(sid));

  const badRoot = await run(['send', sid, '--message', 'x', '--id', randomUUID(), '--registry', join(registryB, 'missing')], env);
  assert.equal(badRoot.status, 1);
  assert.match(badRoot.stderr, /--registry must be an existing directory/);
});
