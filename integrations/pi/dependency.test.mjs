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
import { digest, writeJson, readJson } from './store.mjs';
import { ownerSubscriptionDir, DELIVERY_WAIT_LIMIT } from './dependency-observer.mjs';

const baseTask = () => ({ task_id: 17, title: 'Upstream review', created_event_id: 'evt-17',
  status: 'running', after: [], scope_paths: [], attempts: 1, receipt: null, evidence_paths: [], failure_reason: null });
const returnFixture = () => ({ file: process.execPath,
  args: [fileURLToPath(new URL('./fixtures/edda-return.mjs', import.meta.url))] });
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

test('follow names the session identity it expects when handed a run id', async (t) => {
  const project = await mkdtemp(join(tmpdir(), 'edda-follow-identity-'));
  const root = join(project, 'private');
  t.after(() => rm(project, { recursive: true, force: true }));
  await assert.rejects(
    followDependencies(root, randomUUID(), { project, taskIds: ['17'], notify: false, scope: 'Observe only.' }),
    /Pi channel session id[\s\S]*run-status[\s\S]*not the managed run id/,
  );
});

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

test('owner-bound subscription survives assistant replacement and supersedes the old holder', async (t) => {
  const project = await mkdtemp(join(tmpdir(), 'edda-owner-dependency-'));
  const root = join(project, 'private');
  const ownerRef = 'assistant/owner-subscription';
  let current = baseTask();
  const change = async (patch) => { current = { ...current, ...patch }; await writeFile(join(project, 'task.json'), JSON.stringify(current)); };
  await change({});
  const dependencyCommand = { file: process.execPath, args: [fileURLToPath(new URL('./fixtures/edda-task-reader.mjs', import.meta.url))] };
  const messagesA = [], messagesB = [];
  let a, b;
  t.after(async () => { await b?.close(); await a?.close(); await rm(project, { recursive: true, force: true }); });
  a = await startChannel({ root, sessionId: randomUUID(), cwd: project, ownerRef, ownerCommand: returnFixture(), dependencyCommand,
    deliver: (text) => { messagesA.push(text); a.messageStarted(text); a.settled(); } });
  await enroll(root, a.sessionId, 'Observe this synthetic fixture; no real task work or spending.');
  await a.dependencies.configure({ project, taskIds: ['17'], notify: true, maxNotifications: 10 });
  await a.dependencies.check();
  assert.equal(messagesA.length, 1);
  const dir = ownerSubscriptionDir(root, ownerRef);
  const persisted = readJson(join(dir, 'dependencies.json'));
  assert.equal(persisted.ownerRef, ownerRef);
  assert.equal(persisted.holderSession, a.sessionId);
  assert.equal(persisted.enabled, true);
  assert.equal(typeof persisted.scope, 'string');

  // A replacement session with the same stable owner reference adopts the
  // subscription and observes without any follow/refollow.
  b = await startChannel({ root, sessionId: randomUUID(), cwd: project, ownerRef, ownerCommand: returnFixture(), dependencyCommand,
    deliver: (text) => { messagesB.push(text); b.messageStarted(text); b.settled(); } });
  assert.equal(b.dependencies.status().configured, true);
  assert.notEqual(b.dependencies.status().phase, 'needs_refollow');
  const adopted = readJson(join(dir, 'dependencies.json'));
  assert.equal(adopted.ownerRef, ownerRef);
  assert.equal(adopted.sessionId, b.sessionId);
  assert.equal(adopted.holderSession, b.sessionId);
  await change({ status: 'done', receipt: 'B observed this' });
  await b.dependencies.check();
  assert.equal(messagesB.length, 1);
  assert.ok(messagesB[0].includes('B observed this'));

  // The replaced holder is explicitly superseded and stops notifying.
  await a.dependencies.check();
  assert.equal(a.dependencies.status().phase, 'superseded');
  const before = messagesA.length;
  await change({ receipt: 'A must not send' });
  await a.dependencies.check();
  assert.equal(messagesA.length, before);
});

test('a never-configured replaced holder cannot pause the current subscription', async (t) => {
  const project = await mkdtemp(join(tmpdir(), 'edda-owner-pause-'));
  const root = join(project, 'private');
  const ownerRef = 'assistant/pause-guard';
  await writeFile(join(project, 'task.json'), JSON.stringify(baseTask()));
  const dependencyCommand = { file: process.execPath, args: [fileURLToPath(new URL('./fixtures/edda-task-reader.mjs', import.meta.url))] };
  let a, b;
  t.after(async () => { await b?.close(); await a?.close(); await rm(project, { recursive: true, force: true }); });
  a = await startChannel({ root, sessionId: randomUUID(), cwd: project, ownerRef, ownerCommand: returnFixture(), dependencyCommand, deliver() {} });
  b = await startChannel({ root, sessionId: randomUUID(), cwd: project, ownerRef, ownerCommand: returnFixture(), dependencyCommand, deliver() {} });
  await enroll(root, b.sessionId, 'Observe this synthetic fixture; no real task work or spending.');
  await b.dependencies.configure({ project, taskIds: ['17'], notify: false, maxNotifications: 10 });
  assert.notEqual(b.dependencies.status().phase, 'paused');
  // A never configured, but it must not be able to pause B's subscription.
  await a.dependencies.pause();
  assert.notEqual(b.dependencies.status().phase, 'paused');
});

test('an unconfirmed delivery is abandoned after a bounded wait so a later revision still arrives', async (t) => {
  const messages = [];
  let deliveries = 0;
  let channel;
  const f = await fixture(t, (text) => {
    messages.push(text);
    // The first delivery never confirms, like a holder replaced mid-delivery.
    if (deliveries++ === 0) return;
    channel.messageStarted(text); channel.settled();
  });
  channel = f.channel;
  await f.follow();
  assert.equal(messages.length, 1);
  assert.equal(channel.dependencies.status().lastAlert.receiptStatus, 'unconfirmed');
  await f.change({ status: 'done', receipt: 'v2' });
  await channel.dependencies.check();
  assert.equal(channel.dependencies.status().phase, 'awaiting_delivery');
  for (let i = 0; i < DELIVERY_WAIT_LIMIT + 1; i++) {
    await channel.dependencies.check();
    if (channel.dependencies.status().phase !== 'awaiting_delivery') break;
  }
  assert.equal(channel.dependencies.status().phase, 'notification_sent');
  assert.ok(channel.dependencies.status().abandonedAttempts >= 1);
  assert.equal(messages.length, 2);
  assert.ok(messages[1].includes('"receipt":"v2"'));
});

test('an abandoned attempt is never re-sent and a delivered revision is not duplicated', async (t) => {
  const messages = [];
  let deliveries = 0;
  let channel;
  const f = await fixture(t, (text) => {
    messages.push(text);
    if (deliveries++ === 0) return;
    channel.messageStarted(text); channel.settled();
  });
  channel = f.channel;
  await f.follow();
  await f.change({ status: 'done', receipt: 'v2' });
  for (let i = 0; i < DELIVERY_WAIT_LIMIT + 2; i++) await channel.dependencies.check();
  assert.equal(messages.filter((m) => m.includes('"receipt":"v2"')).length, 1);
  // A genuinely later revision still arrives; the abandoned initial is not re-sent.
  await f.change({ receipt: 'v3' });
  await channel.dependencies.check();
  assert.equal(messages.filter((m) => m.includes('"receipt":"v3"')).length, 1);
  assert.equal(messages.filter((m) => m.includes('"receipt":""')).length, 1);
});

test('an unknown delivery outcome is not blindly retried, and a later revision still arrives', async (t) => {
  const messages = [];
  let deliveries = 0;
  let channel;
  const f = await fixture(t, (text) => {
    messages.push(text);
    if (deliveries++ === 0) throw new Error('transport unknown');
    channel.messageStarted(text); channel.settled();
  });
  channel = f.channel;
  await f.follow();
  assert.equal(channel.dependencies.status().phase, 'delivery_unknown');
  await f.change({ status: 'done', receipt: 'v2' });
  for (let i = 0; i < DELIVERY_WAIT_LIMIT + 1; i++) await channel.dependencies.check();
  // The unknown initial is not retried, and the later revision is delivered.
  assert.equal(messages.filter((m) => m.includes('"status":"running"')).length, 1);
  assert.equal(messages.filter((m) => m.includes('"receipt":"v2"')).length, 1);
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
