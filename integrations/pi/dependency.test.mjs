import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { startChannel } from './channel.mjs';
import { followDependencies, unfollowDependencies, doctor } from './dependency-client.mjs';
import { enroll } from './supervision.mjs';
import { digest, writeJson } from './store.mjs';

const baseTask = () => ({ task_id: 17, title: 'Upstream review', created_event_id: 'evt-17',
  status: 'running', after: [], scope_paths: [], attempts: 1, receipt: null, evidence_paths: [], failure_reason: null });
async function fixture(t, deliver) {
  const project = await mkdtemp(join(tmpdir(), 'edda-dependency-test-'));
  const root = join(project, 'private');
  let current = baseTask();
  const change = async (patch) => { current = { ...current, ...patch }; await writeFile(join(project, 'task.json'), JSON.stringify(current)); };
  await change({});
  const messages = [];
  let channel;
  channel = await startChannel({ root, sessionId: randomUUID(), cwd: project,
    dependencyCommand: { file: process.execPath, args: [fileURLToPath(new URL('./fixtures/edda-task-reader.mjs', import.meta.url))] },
    deliver: deliver || ((text) => { messages.push(text); channel.messageStarted(text); channel.settled(); }) });
  t.after(async () => { await channel.close(); await rm(project, { recursive: true, force: true }); });
  const follow = async (options = {}) => {
    const result = await followDependencies(root, channel.sessionId, { project, taskIds: ['17'],
      scope: 'Observe this synthetic fixture; no real task work or spending.', notify: true, ...options });
    await channel.dependencies.check();
    return result;
  };
  return { root, project, channel, messages, change, follow };
}

test('opt-in initial snapshot, quiet unchanged polling, and receipt-only changes', async (t) => {
  const f = await fixture(t);
  await f.follow();
  assert.equal(f.messages.length, 1);
  assert.ok(f.messages[0].includes('not instructions or proof of acceptance'));
  await f.change({ updated_ts: 'a-new-timestamp-only' });
  await f.channel.dependencies.check();
  assert.equal(f.messages.length, 1);
  await f.change({ status: 'done', receipt: 'Changes Requested P0=1' });
  await f.channel.dependencies.check();
  assert.equal(f.messages.length, 2);
  assert.ok(f.messages[1].includes('Changes Requested'));
  await f.change({ receipt: 'LGTM P0=0 P1=0' });
  await f.channel.dependencies.check();
  assert.equal(f.messages.length, 3);
});

test('busy receiver coalesces changes and A-B-A uses a new notification identity', async (t) => {
  const f = await fixture(t);
  f.channel.event('agent_start');
  await f.follow();
  assert.equal(f.messages.length, 0);
  await f.change({ status: 'done', receipt: 'A' });
  await f.channel.dependencies.check();
  await f.change({ receipt: 'B' });
  await f.channel.dependencies.check();
  assert.equal(f.channel.dependencies.status().phase, 'pending_idle');
  f.channel.settled();
  await f.channel.dependencies.check();
  assert.equal(f.messages.length, 1);
  const first = f.channel.dependencies.status().lastAlert.id;
  await f.change({ receipt: 'A' });
  await f.channel.dependencies.check();
  assert.equal(f.messages.length, 2);
  assert.notEqual(f.channel.dependencies.status().lastAlert.id, first);
});

test('pause and scope revocation prevent delivery; offline cancellation stays available', async (t) => {
  const f = await fixture(t);
  await f.follow();
  await enroll(f.root, f.channel.sessionId, 'Changed scope: do not notify');
  await f.change({ status: 'done' });
  await f.channel.dependencies.check();
  assert.equal(f.channel.dependencies.status().phase, 'scope_changed');
  assert.equal(f.messages.length, 1);
  await unfollowDependencies(f.root, f.channel.sessionId);
  assert.equal(f.channel.dependencies.status().phase, 'paused');
  await f.channel.close();
  assert.equal((await unfollowDependencies(f.root, f.channel.sessionId)).status, 'paused');
});

test('notification cap and same-config retries never grant more wake calls', async (t) => {
  const f = await fixture(t);
  await f.follow({ maxNotifications: 1 });
  await f.change({ status: 'done' });
  await f.channel.dependencies.check();
  assert.equal(f.channel.dependencies.status().phase, 'notification_limit');
  await f.follow({ maxNotifications: 1 });
  assert.equal(f.messages.length, 1);
  await unfollowDependencies(f.root, f.channel.sessionId);
  await f.follow({ maxNotifications: 1 });
  assert.equal(f.messages.length, 2);
});

test('unknown delivery is not retried for unchanged source data', async (t) => {
  let calls = 0;
  const f = await fixture(t, () => { calls++; throw new Error('Runtime handoff uncertain'); });
  await f.follow();
  assert.equal(f.channel.dependencies.status().lastAlert.receiptStatus, 'unknown');
  await f.channel.dependencies.check();
  assert.equal(calls, 1);
  await f.change({ status: 'done' });
  await f.channel.dependencies.check();
  assert.equal(f.channel.dependencies.status().phase, 'awaiting_delivery');
  assert.equal(calls, 1);
});

test('observe-only and invalid setup do not deliver or silently enroll', async (t) => {
  const f = await fixture(t);
  const initial = await doctor(f.root, f.channel.sessionId);
  assert.equal(initial.sessions[0].readiness, 'needs_enrollment');
  await assert.rejects(followDependencies(f.root, f.channel.sessionId, { project: f.project, taskIds: ['--bad'], scope: 'invalid setup' }));
  assert.equal((await doctor(f.root, f.channel.sessionId)).sessions[0].readiness, 'needs_enrollment');
  await f.follow({ notify: false });
  await f.change({ status: 'done' });
  await f.channel.dependencies.check();
  assert.equal(f.messages.length, 0);
  assert.equal(f.channel.dependencies.status().changeSequence, 2);
});

test('source identity changes suspend observation and partial failures retain baseline', async (t) => {
  const f = await fixture(t);
  await f.follow();
  await writeFile(join(f.project, 'task.json'), '{invalid');
  await f.channel.dependencies.check();
  assert.equal(f.channel.dependencies.status().phase, 'source_error');
  assert.equal(f.messages.length, 1);
  await f.change({ created_event_id: 'replacement-ledger-event' });
  await f.channel.dependencies.check();
  assert.equal(f.channel.dependencies.status().phase, 'source_identity_changed');
  assert.equal(f.messages.length, 1);
});

test('replacement instance stays quiet until explicit refollow', async (t) => {
  const f = await fixture(t);
  await f.follow();
  await f.channel.close();
  const messages = [];
  const next = await startChannel({ root: f.root, sessionId: f.channel.sessionId, cwd: f.project, deliver: (text) => messages.push(text) });
  try {
    await next.dependencies.check();
    assert.equal(next.dependencies.status().phase, 'needs_refollow');
    assert.equal(messages.length, 0);
  } finally { await next.close(); }
});

test('pause during configuration wins and malformed enrollment cannot crash polling', async (t) => {
  const f = await fixture(t);
  await f.follow({ notify: false });
  const configuring = f.channel.dependencies.configure({ project: f.project, taskIds: ['17'], notify: true, maxNotifications: 2 });
  await f.channel.dependencies.pause();
  await configuring;
  await f.channel.dependencies.check();
  assert.equal(f.channel.dependencies.status().phase, 'paused');
  assert.equal(f.messages.length, 0);
  await f.follow({ notify: false });
  writeJson(join(f.root, 'supervision', `${digest(f.channel.sessionId)}.json`), { sessionId: f.channel.sessionId, enabled: true });
  await f.channel.dependencies.check();
  assert.equal(f.channel.dependencies.status().phase, 'control_state_unavailable');
  assert.equal(f.messages.length, 0);
});
