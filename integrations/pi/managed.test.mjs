import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile, readFile, copyFile, symlink } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { installRuntime, verifyRelease, rpcFrames, managedDir, alive } from './managed-store.mjs';
import { launchManaged, managedStatus, stopManaged, resumeManaged, managedConversation } from './managed-client.mjs';
import { requestSession } from './client.mjs';
import { readJson, writeJson } from './store.mjs';
import { listInbox } from './inbox-manager.mjs';
import { removeTempTree } from './fixtures/temp-teardown.mjs';

const exec = promisify(execFile);
async function until(fn) {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) { if (await fn()) return; await delay(100); }
  throw new Error('Timed out waiting for owned fixture');
}
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'edda-managed-test-'));
  const project = join(root, 'project'), registry = join(root, 'registry'), pkg = join(root, 'pi');
  await mkdir(project); await mkdir(join(pkg, 'dist/bundle'), { recursive: true });
  await writeFile(join(pkg, 'package.json'), JSON.stringify({ type: 'module', name: '@earendil-works/pi-coding-agent', version: 'fixture' }));
  const entry = join(pkg, 'dist/bundle/cli.js');
  await copyFile(new URL('./fixtures/managed-pi.mjs', import.meta.url), entry);
  const runId = randomUUID();
  t.after(async () => {
    if (readJson(join(managedDir(registry, runId), 'config.json'))) {
      const state = await managedStatus(registry, runId);
      if (state.live) await stopManaged(registry, runId, { abort: true });
      await until(async () => { const current = await managedStatus(registry, runId); return !alive(current.runnerPid) && !alive(current.childPid); });
    }
    await removeTempTree(root);
  });
  return { root, project, registry, entry, runId };
}

test('runtime install is repeatable and verifies actual file bytes', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-release-test-'));
  t.after(async () => { await removeTempTree(root); });
  const first = installRuntime(root), second = installRuntime(root);
  assert.deepEqual(first, second);
  assert.equal(verifyRelease(first).id, first.id);
  await writeFile(join(first.path, 'extension.mjs'), 'changed');
  assert.throws(() => verifyRelease(first), /mismatch/);
});

test('RPC parser honors LF only across UTF-8 chunks and refuses malformed/oversized frames', () => {
  const messages = [], errors = [];
  const parse = rpcFrames((m) => messages.push(m), (e) => errors.push(e.message));
  for (const byte of Buffer.from(JSON.stringify({ text: '你\u2028我\u2029' }) + '\r\n')) parse(Buffer.from([byte]));
  assert.equal(messages[0].text, '你\u2028我\u2029');
  assert.deepEqual(errors, []);
  parse(Buffer.from('bad\n'));
  assert.equal(errors.length, 1);
  const large = rpcFrames(() => {}, (e) => errors.push(e.message), 4);
  large(Buffer.from('12345'));
  assert.match(errors.at(-1), /limit/);
});

test('CLI launch survives client exit; duplicate identity, authenticated stop and same-session resume', async (t) => {
  const f = await fixture(t);
  const promptFile = join(f.root, 'prompt.txt'); await writeFile(promptFile, 'HELLO');
  const cli = fileURLToPath(new URL('./cli.mjs', import.meta.url));
  const launched = await exec(process.execPath, [cli, 'launch', '--project', f.project, '--pi-entry', f.entry,
    '--run-id', f.runId, '--prompt-file', promptFile, '--provider', 'fixture', '--model', 'echo'],
  { env: { ...process.env, EDDA_PI_CHANNEL_DIR: f.registry }, windowsHide: true, timeout: 45000 });
  const state = JSON.parse(launched.stdout);
  assert.equal(state.live, true);
  await until(async () => (await managedStatus(f.registry, f.runId)).initialReceipt?.status === 'settled');
  const duplicate = await launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry, prompt: 'HELLO', provider: 'fixture', model: 'echo' });
  assert.equal(duplicate.serviceId, state.serviceId);
  await assert.rejects(launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry, prompt: 'DIFFERENT' }), /different launch inputs/);
  const owner = readJson(join(managedDir(f.registry, f.runId), 'owner.json'));
  const rejected = await fetch(`http://127.0.0.1:${owner.port}/stop`, { method: 'POST', headers: { authorization: `Bearer ${owner.token}`, 'x-edda-instance': randomUUID() } });
  assert.equal(rejected.status, 409);
  assert.equal((await managedStatus(f.registry, f.runId)).live, true);
  const before = await requestSession(f.registry, state.sessionId, '/conversation?limit=20');
  assert.equal((await stopManaged(f.registry, f.runId)).status, 'stopped');
  // Real Pi often returns unconfirmed at launch, then settles asynchronously.
  // The fixture settles synchronously, so preserve that real launch-copy shape.
  const stateFile = join(managedDir(f.registry, f.runId), 'state.json');
  const saved = readJson(stateFile);
  writeJson(stateFile, { ...saved, initialReceipt: { id: saved.initialReceipt.id, status: 'unconfirmed' } });
  const stopped = await managedStatus(f.registry, f.runId);
  assert.equal(stopped.live, false);
  assert.equal(stopped.initialReceipt.status, 'settled');
  assert.equal(stopped.initialReceipt.live, false);
  const offline = await managedConversation(f.registry, f.runId);
  assert.equal(offline.evidenceSource, 'persisted_session');
  assert.equal(offline.conversation.headCursor, before.headCursor);
  assert.deepEqual(offline.conversation.entries, before.entries);
  assert.equal((await managedStatus(f.registry, f.runId)).live, false);
  await writeFile(join(f.root, 'pi/package.json'), JSON.stringify({ type: 'module', name: '@earendil-works/pi-coding-agent', version: 'fixture-updated' }));
  const resumed = await resumeManaged(f.registry, f.runId);
  assert.equal(resumed.sessionId, state.sessionId);
  assert.equal(resumed.piVersion, 'fixture-updated');
  assert.equal(resumed.expectedPiVersion, 'fixture');
  assert.notEqual(resumed.instanceId, state.instanceId);
  assert.equal((await requestSession(f.registry, state.sessionId, '/conversation?limit=20')).headCursor, before.headCursor);
  assert.ok(listInbox(f.registry).events.length);
});

test('busy stop refuses; explicit abort only stops the owned child', async (t) => {
  const f = await fixture(t);
  await launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry, prompt: 'BUSY' });
  await until(async () => (await managedStatus(f.registry, f.runId)).pi?.state === 'running');
  await assert.rejects(stopManaged(f.registry, f.runId), /working/);
  const result = await stopManaged(f.registry, f.runId, { abort: true });
  assert.equal(result.status, 'stopped'); assert.equal(result.interrupted, true);
});

test('malformed RPC fails without an initial prompt being sent', async (t) => {
  const f = await fixture(t);
  const result = await launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry, prompt: 'MUST_NOT_SEND', provider: 'bad-rpc' });
  assert.equal(result.lastRecordedPhase, 'failed');
  assert.match(result.error, /RPC frame/);
  assert.equal(result.initialAttemptedAt, undefined);
});

test('missing session file prevents recovery without spawning a replacement', async (t) => {
  const f = await fixture(t);
  const launched = await launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry });
  await stopManaged(f.registry, f.runId);
  const source = await readFile(launched.sessionFile, 'utf8');
  await rm(launched.sessionFile);
  await assert.rejects(resumeManaged(f.registry, f.runId), /ENOENT/);
  await assert.rejects(managedConversation(f.registry, f.runId), /ENOENT/);
  const entries = source.split('\n');
  entries[0] = JSON.stringify({ ...JSON.parse(entries[0]), id: randomUUID() });
  await writeFile(launched.sessionFile, entries.join('\n'));
  await assert.rejects(managedConversation(f.registry, f.runId), /identity/);
  await writeFile(launched.sessionFile, source);
});

test('a NUL state.json degrades run-status and recovers run-conversation from the owned session', async (t) => {
  const f = await fixture(t);
  await launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry, prompt: 'HELLO' });
  await until(async () => (await managedStatus(f.registry, f.runId)).initialReceipt?.status === 'settled');
  await stopManaged(f.registry, f.runId);
  const expected = await managedConversation(f.registry, f.runId);
  const stateFile = join(managedDir(f.registry, f.runId), 'state.json');
  const config = readJson(join(managedDir(f.registry, f.runId), 'config.json'));
  await writeFile(stateFile, Buffer.alloc(1887, 0));
  const before = await readFile(stateFile);
  const status = await managedStatus(f.registry, f.runId);
  assert.equal(status.status, 'record_unavailable');
  assert.equal(status.live, false);
  assert.equal(status.runId, f.runId);
  assert.equal(status.project, config.project);
  assert.deepEqual(status.release, config.release);
  assert.deepEqual(status.error, { code: 'record_unavailable', record: 'state.json',
    message: 'Record unavailable: state.json; it was not repaired' });
  const recovered = await managedConversation(f.registry, f.runId);
  assert.equal(recovered.evidenceSource, 'persisted_session');
  assert.equal(recovered.sessionId, expected.sessionId);
  assert.equal(recovered.conversation.headCursor, expected.conversation.headCursor);
  assert.deepEqual(recovered.conversation.entries, expected.conversation.entries);
  assert.deepEqual(await readFile(stateFile), before);
});

test('system-style ancestor aliases do not reject an owned session during resume', async (t) => {
  const f = await fixture(t);
  const alias = join(f.root, 'ancestor-alias');
  const target = join(f.root, 'real-ancestor');
  await mkdir(target);
  await symlink(target, alias, process.platform === 'win32' ? 'junction' : 'dir');
  const registry = join(alias, 'registry');
  let launched;
  try {
    launched = await launchManaged(registry, { project: f.project, piEntry: f.entry, prompt: 'HELLO' });
    await until(async () => (await managedStatus(registry, launched.runId)).initialReceipt?.status === 'settled');
    await stopManaged(registry, launched.runId);
    const resumed = await resumeManaged(registry, launched.runId);
    assert.equal(resumed.sessionId, launched.sessionId);
    assert.equal(resumed.live, true);
  } finally {
    if (launched) {
      const status = await managedStatus(registry, launched.runId);
      if (status.live) await stopManaged(registry, launched.runId, { abort: true });
    }
  }
});

test('managed launch records the stable owner reference and exposes it even when state is unreadable', async (t) => {
  const f = await fixture(t);
  const launched = await launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry, prompt: 'HELLO',
    provider: 'fixture', model: 'echo', owner: 'assistant/project', returnOwner: 'assistant/return' });
  assert.equal(launched.owner, 'assistant/project');
  assert.equal(launched.returnOwner, 'assistant/return');
  assert.match(launched.ownerRoot, /owner-mailbox$/);
  await assert.rejects(launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry, prompt: 'HELLO',
    provider: 'fixture', model: 'echo', owner: 'assistant/other', returnOwner: 'assistant/return' }), /different launch inputs/);
  await assert.rejects(launchManaged(f.registry, { runId: randomUUID(), project: f.project, piEntry: f.entry, owner: 'bad\u0000owner' }), /owner/);
  // An unset shell variable expands to the empty string; it must be treated as
  // absent (here: a different inputs digest), not rejected as an invalid label.
  await assert.rejects(launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry, prompt: 'HELLO',
    provider: 'fixture', model: 'echo', owner: 'assistant/project', returnOwner: '' }), /different launch inputs/);
  await until(async () => (await managedStatus(f.registry, f.runId)).initialReceipt?.status === 'settled');
  await stopManaged(f.registry, f.runId);
  const stopped = await managedStatus(f.registry, f.runId);
  assert.equal(stopped.owner, 'assistant/project');
  assert.equal(stopped.returnOwner, 'assistant/return');
  await writeFile(join(managedDir(f.registry, f.runId), 'state.json'), Buffer.alloc(1887, 0));
  const degraded = await managedStatus(f.registry, f.runId);
  assert.equal(degraded.status, 'record_unavailable');
  assert.equal(degraded.owner, 'assistant/project');
  assert.equal(degraded.returnOwner, 'assistant/return');
});

test('managed runner strips an inherited owner identity and passes only the configured contract', async (t) => {
  const f = await fixture(t);
  const capture = join(f.root, 'owner-env.json');
  const priorOwner = process.env.EDDA_OWNER_REF, priorReturn = process.env.EDDA_RETURN_OWNER;
  process.env.EDDA_OWNER_REF = 'assistant/leaked-parent';
  process.env.EDDA_FIXTURE_ENV_CAPTURE = capture;
  t.after(() => {
    if (priorOwner === undefined) delete process.env.EDDA_OWNER_REF; else process.env.EDDA_OWNER_REF = priorOwner;
    if (priorReturn === undefined) delete process.env.EDDA_RETURN_OWNER; else process.env.EDDA_RETURN_OWNER = priorReturn;
    delete process.env.EDDA_FIXTURE_ENV_CAPTURE;
  });
  await launchManaged(f.registry, { runId: f.runId, project: f.project, piEntry: f.entry, returnOwner: 'assistant/real-return' });
  await until(async () => existsSync(capture));
  const captured = JSON.parse(await readFile(capture, 'utf8'));
  assert.equal(captured.owner, null);
  assert.equal(captured.returnOwner, 'assistant/real-return');
  assert.equal(captured.returnRoot, join(f.registry, 'owner-mailbox'));
});
