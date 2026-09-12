import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import extension from './extension.mjs';
import { listSessions, requestSession, getReceipt } from './client.mjs';

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
  await emit('message_end', { message: { role: 'assistant', stopReason: 'stop' } });
  await emit('agent_settled');
  assert.equal((await getReceipt(root, sid, id)).status, 'settled');
  const prior = sid;
  await emit('session_shutdown');
  sid = randomUUID();
  await emit('session_start');
  const rows = await listSessions(root);
  assert.equal(rows.find((r) => r.sessionId === prior).state, 'stopped');
  assert.equal(rows.find((r) => r.sessionId === sid).live, true);
});
