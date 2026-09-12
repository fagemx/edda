import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { startChannel } from './channel.mjs';
import { pageConversation, projectEntry } from './conversation.mjs';
import { enroll, watch, reply, checkpoint } from './supervision.mjs';

test('supervisor reads exact reply, sends once per reviewed cursor, refuses busy/stale replies, checkpoints', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-supervisor-test-'));
  const entries = [projectEntry({ type: 'message', id: 'a', parentId: null,
    message: { role: 'assistant', content: [{ type: 'text', text: 'Please specify the next action' }] } })];
  const calls = [];
  const channel = await startChannel({ root, sessionId: randomUUID(), cwd: root,
    deliver: (text) => calls.push(text), getConversation: (options) => pageConversation(entries, options) });
  t.after(async () => { await channel.close(); await rm(root, { recursive: true, force: true }); });
  const sid = channel.sessionId;
  await enroll(root, sid, 'Continue original task; no new spend or database permissions');
  const [view] = await watch(root);
  assert.equal(view.conversation.entries[0].text, 'Please specify the next action');
  assert.equal(view.conversation.source, 'pi_runtime');
  assert.equal(view.assessment, 'read_reply_before_deciding');
  await assert.rejects(reply(root, sid, { to: 'wrong', message: 'continue' }), /New conversation/);
  const first = await reply(root, sid, { to: 'a', message: 'Inspect the existing test failure; fix within the original scope.' });
  assert.equal(first.status, 'unconfirmed');
  const again = await reply(root, sid, { to: 'a', message: 'Inspect the existing test failure; fix within the original scope.' });
  assert.equal(again.id, first.id);
  assert.equal(calls.length, 1);
  await assert.rejects(reply(root, sid, { to: 'a', message: 'different approval' }), /different reply/);
  entries.push(projectEntry({ type: 'message', id: 'b', parentId: 'a', message: { role: 'assistant', content: 'working' } }));
  channel.event('agent_start');
  await assert.rejects(reply(root, sid, { to: 'b', message: 'continue' }), /idle/);
  channel.settled();
  await checkpoint(root, sid, { cursor: 'b', action: 'waiting_user', note: 'Separate migration approval remains pending.' });
  const [later] = await watch(root);
  assert.equal(later.conversation.entries.length, 0);
  assert.equal(later.supervision.action, 'waiting_user');
  entries.push(projectEntry({ type: 'message', id: 'c', parentId: 'b', message: { role: 'user', content: 'Verification failed; wait before completion.' } }));
  await assert.rejects(checkpoint(root, sid, { cursor: 'b', action: 'complete', note: 'Stale completion decision.' }), /unread activity/);
  assert.equal((await watch(root))[0].conversation.entries[0].id, 'c');
  await checkpoint(root, sid, { cursor: 'c', action: 'complete', note: 'Operator ended this synthetic test after observing the latest instruction.' });
  assert.deepEqual(await watch(root), []);
});

test('explicit pause works after an unpersisted session disappears, preserving its checkpoint', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-pause-test-'));
  const entries = [projectEntry({ type: 'message', id: 'a', parentId: null, message: { role: 'assistant', content: 'Waiting' } })];
  const channel = await startChannel({ root, sessionId: randomUUID(), cwd: root,
    deliver() {}, getConversation: (options) => pageConversation(entries, options) });
  t.after(async () => { await channel.close(); await rm(root, { recursive: true, force: true }); });
  await enroll(root, channel.sessionId, 'Synthetic local-only task');
  await checkpoint(root, channel.sessionId, { cursor: 'a', action: 'observed', note: 'Read waiting response.' });
  await channel.close();
  assert.equal((await watch(root))[0].assessment, 'inspection_failed_do_not_send');
  const paused = await checkpoint(root, channel.sessionId, { action: 'paused', note: 'Operator requested monitoring stop.' });
  assert.equal(paused.enabled, false);
  assert.equal(paused.cursor, 'a');
  assert.deepEqual(await watch(root), []);
});
