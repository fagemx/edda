import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile, readFile } from 'node:fs/promises';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import extension, { managedOwnerRefs } from './extension.mjs';
import { listSessions, requestSession, getReceipt } from './client.mjs';
import { listInbox, readInbox } from './inbox-manager.mjs';
import { sessionDir, readJson } from './store.mjs';
import { doctor } from './dependency-client.mjs';

test('Pi lifecycle, tool spans, queued messages, UI waits, settlement and session replacement', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-extension-test-'));
  const old = process.env.EDDA_PI_CHANNEL_DIR;
  process.env.EDDA_PI_CHANNEL_DIR = root;
  const events = new Map();
  const delivered = [];
  const notices = [];
  let sid = randomUUID();
  const ctx = { cwd: root, sessionManager: { getSessionId: () => sid }, isIdle: () => true,
    ui: { setStatus() {}, notify: (...args) => notices.push(args) } };
  const pi = { registerFlag() {}, getFlag: () => 'test-worker',
    registerTool() {},
    registerCommand() {}, on: (name, fn) => events.set(name, fn),
    sendUserMessage: (text, options) => delivered.push({ text, options }) };
  extension(pi);
  t.after(async () => {
    await events.get('session_shutdown')();
    if (old === undefined) delete process.env.EDDA_PI_CHANNEL_DIR; else process.env.EDDA_PI_CHANNEL_DIR = old;
    await rm(root, { recursive: true, force: true });
  });
  const emit = (name, value = {}) => events.get(name)(value, ctx);
  await emit('session_start');
  assert.equal(notices.length, 0);
  const state = () => requestSession(root, sid, '/status');
  assert.equal((await state()).state, 'idle');
  await emit('agent_start');
  await emit('tool_execution_start', { toolCallId: '1', toolName: 'bash' });
  await emit('tool_execution_start', { toolCallId: '2', toolName: 'read' });
  await emit('tool_execution_end', { toolCallId: '1' });
  assert.deepEqual((await state()).toolNames, ['read']);
  await emit('ui_prompt_start', { kind: 'select' });
  assert.equal((await state()).state, 'waiting_user');
  await emit('ui_prompt_end');
  await emit('tool_execution_end', { toolCallId: '2' });
  assert.equal((await state()).state, 'running');
  const id = randomUUID();
  await requestSession(root, sid, '/messages', { id, message: 'continue', sender: 'codex', mode: 'followUp' });
  assert.equal(delivered[0].options.deliverAs, 'followUp');
  await emit('agent_settled');
  // A local run settling cannot complete a message Pi has not started.
  assert.equal((await getReceipt(root, sid, id)).status, 'unconfirmed');
  await emit('agent_start');
  await emit('message_start', { message: { role: 'user', content: [{ type: 'text', text: delivered[0].text }] } });
  await emit('message_end', { message: { role: 'assistant', stopReason: 'stop', content: [
    { type: 'thinking', thinking: 'PRIVATE_REASONING_MUST_NOT_LEAK' }, { type: 'text', text: 'Public stopping reason' },
  ] } });
  await emit('agent_settled');
  const inbox = listInbox(root);
  const detail = await readInbox(root, inbox.events.at(-1).eventId);
  assert.equal(detail.event.excerpt.text, 'Public stopping reason');
  assert.ok(!JSON.stringify(detail).includes('PRIVATE_REASONING_MUST_NOT_LEAK'));
  assert.equal((await getReceipt(root, sid, id)).status, 'settled');
  const prior = sid;
  await emit('session_shutdown');
  sid = randomUUID();
  await emit('session_start');
  const rows = await listSessions(root);
  assert.equal(rows.find((r) => r.sessionId === prior).state, 'stopped');
  assert.equal(rows.find((r) => r.sessionId === sid).live, true);
});

test('resumed original Pi session preserves NUL state, refuses live owner, then reconnects without replay', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-reconnect-test-'));
  const oldRoot = process.env.EDDA_PI_CHANNEL_DIR;
  process.env.EDDA_PI_CHANNEL_DIR = root;
  const sid = randomUUID(), entry = fileURLToPath(new URL('./fixtures/channel-owner.mjs', import.meta.url));
  const child = spawn(process.execPath, [entry, root, sid], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  const events = new Map(), commands = new Map(), notices = [], sent = [];
  const ctx = { cwd: root, sessionManager: { getSessionId: () => sid }, isIdle: () => true,
    ui: { setStatus() {}, notify: (...args) => notices.push(args) } };
  const pi = { registerFlag() {}, getFlag: () => '', registerTool() {},
    registerCommand: (name, value) => commands.set(name, value), on: (name, fn) => events.set(name, fn),
    sendUserMessage: (...args) => sent.push(args) };
  extension(pi);
  t.after(async () => {
    await events.get('session_shutdown')();
    if (child.exitCode === null && child.signalCode === null) { const exit = once(child, 'exit'); child.kill(); await exit; }
    if (oldRoot === undefined) delete process.env.EDDA_PI_CHANNEL_DIR; else process.env.EDDA_PI_CHANNEL_DIR = oldRoot;
    await rm(root, { recursive: true, force: true });
  });
  const [output] = await once(child.stdout, 'data', { signal: AbortSignal.timeout(15000) });
  const owner = JSON.parse(output.toString()), statePath = join(sessionDir(root, sid), 'state.json');
  await writeFile(statePath, Buffer.alloc(472));
  await events.get('session_start')({}, ctx);
  await commands.get('edda-session-recover').handler('', ctx);
  assert.match(notices.at(-1)[0], /still live; recovery refused/);
  assert.equal(readJson(join(sessionDir(root, sid), 'owner.json')).instanceId, owner.instanceId);
  const exit = once(child, 'exit'); child.kill(); await exit;
  await commands.get('edda-session-recover').handler('', ctx);
  assert.match(notices.at(-1)[0], /reconnected for original session/);
  assert.equal((await requestSession(root, sid, '/status')).live, true);
  assert.deepEqual(await readFile(`${statePath}.${owner.instanceId}.damaged`), Buffer.alloc(472));
  assert.deepEqual(sent, []);
  await commands.get('edda-session-recover').handler('', ctx);
  assert.match(notices.at(-1)[0], /already live/);
});

test('legacy CLI recover preserves damaged state and receipts before reclaiming a dead owner', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-legacy-recovery-test-'));
  const sid = randomUUID(), entry = fileURLToPath(new URL('./fixtures/channel-owner.mjs', import.meta.url));
  const child = spawn(process.execPath, [entry, root, sid], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) { const exit = once(child, 'exit'); child.kill(); await exit; }
    await rm(root, { recursive: true, force: true });
  });
  const [output] = await once(child.stdout, 'data', { signal: AbortSignal.timeout(15000) });
  const owner = JSON.parse(output.toString()), dir = sessionDir(root, sid);
  const receiptPath = join(dir, 'receipts', `${randomUUID()}.json`);
  await writeFile(receiptPath, '{"status":"unknown"}');
  await writeFile(join(dir, 'state.json'), Buffer.alloc(445));
  const exit = once(child, 'exit'); child.kill(); await exit;
  const cli = fileURLToPath(new URL('./cli.mjs', import.meta.url));
  const { stdout } = await promisify(execFile)(process.execPath, [cli, 'recover', sid, '--instance', owner.instanceId],
    { env: { ...process.env, EDDA_PI_CHANNEL_DIR: root }, timeout: 15000 });
  assert.equal(JSON.parse(stdout).recovered, true);
  assert.deepEqual(await readFile(join(dir, 'state.json')), Buffer.alloc(445));
  assert.deepEqual(await readFile(join(dir, `state.json.${owner.instanceId}.damaged`)), Buffer.alloc(445));
  assert.equal(await readFile(receiptPath, 'utf8'), '{"status":"unknown"}');
});

test('doctor explicitly names a reachable stale loaded extension without upgrading it', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-stale-extension-test-'));
  const sid = randomUUID(), instanceId = randomUUID(), token = 'a'.repeat(64);
  const server = createServer((req, res) => {
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify({ sessionId: sid, instanceId, live: true, state: 'idle',
      integration: { modulePath: join(root, 'stale-source', 'channel.mjs'), version: '0.7.0' } }));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => { await new Promise((resolve) => server.close(resolve)); await rm(root, { recursive: true, force: true }); });
  await mkdir(sessionDir(root, sid), { recursive: true });
  await writeFile(join(sessionDir(root, sid), 'owner.json'), JSON.stringify({ sessionId: sid, instanceId,
    token, port: server.address().port, pid: process.pid }));
  const result = await doctor(root, sid);
  assert.equal(result.sessions[0].readiness, 'stale_extension');
  assert.match(result.sessions[0].nextAction, /SAME Pi session/);
});

test('managedOwnerRefs reads an adopted owner from a managed run state', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-owner-refs-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const runId = randomUUID(), ownerRoot = join(root, 'owner-mailbox');
  await mkdir(join(root, 'managed', runId), { recursive: true });
  await writeFile(join(root, 'managed', runId, 'state.json'), JSON.stringify({ runId,
    owner: 'assistant/adopted', returnOwner: 'assistant/return', ownerRoot }));
  assert.deepEqual(managedOwnerRefs(root, runId), { owner: 'assistant/adopted', returnOwner: 'assistant/return', ownerRoot });
  assert.equal(managedOwnerRefs(root, randomUUID()), null); // no state record
  assert.equal(managedOwnerRefs(root, 'not-a-run'), null);  // not a managed run id
});
