import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, appendFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { ExperimentPi, checkpoint, forkCheckpoint, sessionSDK } from './fork-smoke-runtime.mjs';
const entry = process.argv[2];
if (!entry) throw new Error('Supply installed Pi entry');
const { SessionManager } = await sessionSDK(entry);
const root = mkdtempSync(join(tmpdir(), 'edda-native-fork-'));
const profile = { entry, provider: 'edda-fork-fixture', model: 'echo', thinking: 'off',
  extensions: [fileURLToPath(new URL('./fixtures/fork-provider.mjs', import.meta.url))] };
const owned = [];
const make = (label, sessionFile) => {
  const cwd = join(root, label); mkdirSync(cwd);
  const pi = new ExperimentPi({ ...profile, cwd, dir: join(root, `${label}-session`), sessionFile }); owned.push(pi); return pi;
};
try {
  const parent = make('parent'); await parent.ready(); await parent.prompt('SEED_86cb', 15000);
  const source = await checkpoint(parent, SessionManager, join(root, 'checkpoint.jsonl'));
  const sourceBytes = readFileSync(source.path);
  const children = ['one', 'two'].map((label) => {
    const cwd = join(root, label); mkdirSync(cwd);
    const dir = join(root, `${label}-session`);
    const fork = forkCheckpoint(SessionManager, source, cwd, dir);
    const pi = new ExperimentPi({ ...profile, cwd, dir, sessionFile: fork.sessionFile }); owned.push(pi); return pi;
  });
  const states = await Promise.all(children.map((pi) => pi.ready()));
  assert.equal(new Set([parent.identity.sessionId, ...states.map((s) => s.sessionId)]).size, 3);
  assert.equal(new Set(owned.map((pi) => pi.child.pid)).size, 3);
  await Promise.all(children.map((pi) => pi.prompt('CHILD', 15000)));
  for (const pi of children) {
    const response = await pi.rpc('get_last_assistant_text');
    assert.deepEqual(JSON.parse(response.text), { seed: true, child: true, parentLater: false });
  }
  assert.deepEqual(readFileSync(parent.identity.sessionFile), sourceBytes);
  await parent.prompt('PARENT_LATER', 15000);
  assert.deepEqual(JSON.parse((await parent.rpc('get_last_assistant_text')).text), { seed: true, child: false, parentLater: true });
  assert.deepEqual(readFileSync(source.path), sourceBytes);
  assert.throws(() => forkCheckpoint(SessionManager, source, children[0].cwd, children[0].dir), /EEXIST/);
  appendFileSync(source.path, '\n');
  assert.throws(() => forkCheckpoint(SessionManager, source, children[1].cwd, join(root, 'tampered')), /Checkpoint changed/);
  await assert.rejects(() => checkpoint({ rpc: async () => ({ isStreaming: true }) }, SessionManager, join(root, 'busy')), /not idle/);
  const incomplete = SessionManager.create(root, join(root, 'incomplete'));
  incomplete.appendMessage({ role: 'user', content: 'fixture', timestamp: Date.now() });
  incomplete.appendMessage({ role: 'assistant', content: [{ type: 'toolCall', id: 'pending', name: 'read', arguments: { path: 'x' } }], stopReason: 'toolUse', timestamp: Date.now() });
  await assert.rejects(() => checkpoint({ rpc: async () => ({ sessionId: incomplete.getSessionId(), sessionFile: incomplete.getSessionFile() }) }, SessionManager, join(root, 'incomplete-copy')), /Incomplete tool/);
  await assert.rejects(() => parent.prompt('HANG', 200), /Worker timeout/);
  await parent.stop();
  assert.ok(parent.child.exitCode !== null || parent.child.signalCode !== null);
  console.log(JSON.stringify({ passed: true, actualPi: true, nativeFork: true, children: 2, distinctProcesses: true,
    inheritedContextObservedByProvider: true, parentUnchangedByChildren: true, parentContinuesIndependently: true,
    duplicateIntentRejected: true, changedCheckpointRejected: true, busyParentRejected: true, incompleteToolRejected: true,
    timeoutObservedAndOwnedProcessStopped: true, root }));
} finally { for (const pi of owned) await pi.stop(); }
