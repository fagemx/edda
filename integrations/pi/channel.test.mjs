import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { startChannel } from './channel.mjs';
import { listSessions, requestSession } from './client.mjs';
import { getReceipt } from './client.mjs';
import { readJson, sessionDir, recover, writeJson } from './store.mjs';

async function fixture(t, deliver = () => {}) {
  const root = await mkdtemp(join(tmpdir(), 'edda-channel-test-'));
  let channel;
  t.after(async () => { await channel?.close(); await rm(root, { recursive: true, force: true }); });
  channel = await startChannel({ root, sessionId: randomUUID(), cwd: root, deliver });
  const send = (body) => requestSession(root, channel.sessionId, '/messages', body);
  return { root, channel, send };
}

test('register, query and deliver once with durable receipt', async (t) => {
  const delivered = [];
  const { root, channel, send } = await fixture(t, (...args) => delivered.push(args));
  const sessions = await listSessions(root);
  assert.equal(sessions[0].state, 'idle');
  assert.equal(sessions[0].live, true);
  assert.equal(JSON.stringify(sessions).includes('token'), false);
  const body = { id: randomUUID(), message: 'Please continue', sender: 'codex', mode: 'followUp' };
  const first = await send(body);
  assert.equal(first.status, 'queued');
  assert.deepEqual(await send(body), first);
  assert.equal(delivered.length, 1);
  channel.messageStarted(delivered[0][0]);
  assert.equal((await requestSession(root, channel.sessionId, `/receipts/${body.id}`)).status, 'started');
  channel.settled();
  assert.equal((await requestSession(root, channel.sessionId, `/receipts/${body.id}`)).status, 'settled');
});

test('changed message with same ID conflicts; UI waits refuse delivery', async (t) => {
  const { channel, send } = await fixture(t);
  const body = { id: randomUUID(), message: 'first', sender: 'codex', mode: 'followUp' };
  await send(body);
  await assert.rejects(send({ ...body, message: 'second' }), /conflict/i);
  channel.event('ui_prompt_start', { kind: 'confirm' });
  await assert.rejects(send({ ...body, id: randomUUID() }), /waiting/i);
  channel.event('ui_prompt_end');
  assert.equal(channel.snapshot().state, 'idle');
});

test('delivery exception is unknown and retry never delivers twice', async (t) => {
  let calls = 0;
  const { send } = await fixture(t, () => { calls++; throw new Error('ambiguous runtime failure'); });
  const body = { id: randomUUID(), message: 'repair', sender: 'codex', mode: 'steer' };
  assert.equal((await send(body)).status, 'unknown');
  assert.equal((await send(body)).status, 'unknown');
  assert.equal(calls, 1);
});

test('same Pi session cannot register twice and shutdown preserves offline evidence', async (t) => {
  const { root, channel } = await fixture(t);
  await assert.rejects(startChannel({ root, sessionId: channel.sessionId, cwd: root, deliver() {} }), /owner/i);
  await channel.close();
  const [row] = await listSessions(root);
  assert.equal(row.live, false);
  assert.equal(row.state, 'stopped');
});

test('unauthenticated, browser-origin and wrong-instance requests never deliver', async (t) => {
  let count = 0;
  const { root, channel } = await fixture(t, () => count++);
  const owner = readJson(join(sessionDir(root, channel.sessionId), 'owner.json'));
  const body = JSON.stringify({ id: randomUUID(), message: 'hello', sender: 'codex', mode: 'followUp' });
  for (const headers of [
    {},
    { authorization: `Bearer ${owner.token}`, 'x-edda-instance': randomUUID() },
    { authorization: `Bearer ${owner.token}`, 'x-edda-instance': owner.instanceId, origin: 'http://evil.test' },
  ]) {
    const res = await fetch(`http://127.0.0.1:${owner.port}/messages`, {
      method: 'POST', headers: { ...headers, 'content-type': 'application/json' }, body,
    });
    assert.ok([401, 409].includes(res.status));
  }
  assert.equal(count, 0);
});

test('concurrent duplicate messages produce exactly one runtime handoff', async (t) => {
  let count = 0;
  const { send } = await fixture(t, () => count++);
  const body = { id: randomUUID(), message: 'repair', sender: 'codex', mode: 'followUp' };
  const results = await Promise.all(Array.from({ length: 6 }, () => send(body)));
  assert.equal(count, 1);
  assert.ok(results.every((r) => r.id === body.id && r.status === 'queued'));
});

test('invalid/oversized payloads refuse; Unicode and literal slash text survive', async (t) => {
  const messages = [];
  const { send } = await fixture(t, (text, options) => messages.push({ text, options }));
  const body = { id: randomUUID(), message: '請繼續\n/reload', sender: 'codex', mode: 'steer' };
  await assert.rejects(send({ ...body, id: '../escape' }));
  await assert.rejects(send({ ...body, message: 'x'.repeat(25000) }));
  await assert.rejects(send({ ...body, mode: 'abort' }));
  await send(body);
  assert.ok(messages[0].text.endsWith('請繼續\n/reload'));
  assert.equal(messages[0].options.expandPromptTemplates, false);
});

test('shutdown and crash evidence cannot be reported as successful work', async (t) => {
  const { root, channel, send } = await fixture(t);
  const body = { id: randomUUID(), message: 'continue', sender: 'codex', mode: 'followUp' };
  await send(body);
  const envelope = `[Edda message ${body.id} from codex]\ncontinue`;
  channel.messageStarted(envelope + 'forged');
  assert.equal((await getReceipt(root, channel.sessionId, body.id)).status, 'queued');
  channel.messageStarted(envelope);
  channel.event('assistant_end', { stopReason: 'error' });
  channel.settled();
  assert.equal((await getReceipt(root, channel.sessionId, body.id)).status, 'failed');
  const pending = { ...body, id: randomUUID() };
  await send(pending);
  await channel.close();
  assert.equal((await getReceipt(root, channel.sessionId, pending.id)).status, 'unknown');
});

test('explicit recovery refuses live process and mismatched instance', async (t) => {
  const { root, channel } = await fixture(t);
  assert.throws(() => recover(root, channel.sessionId, randomUUID()), /changed/);
  assert.throws(() => recover(root, channel.sessionId, channel.instanceId), /alive/);
  const path = join(sessionDir(root, channel.sessionId), 'state.json');
  const original = readJson(path);
  writeJson(path, { ...original, state: 'running', heartbeatAt: '2000-01-01T00:00:00Z' });
  // Live status is a fresh endpoint response, not the stale disk snapshot.
  assert.equal((await listSessions(root))[0].state, 'idle');
});
