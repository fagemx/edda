import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { startChannel } from './channel.mjs';
import { prepareHandoff, requestSession } from './client.mjs';
import { enroll } from './supervision.mjs';
import { digest } from './store.mjs';
import { inboxStore, wakeCapability } from './inbox-store.mjs';
import { publicExcerpt } from './inbox-producer.mjs';
import { listInbox, readInbox, acknowledgeInbox, recordAuthorization, revokeAuthorization, respondInbox } from './inbox-manager.mjs';

const manifest = { version: 1, runId: 'fixture-run', role: 'controller', goal: 'Offline fixture',
  planRef: { uri: 'fixture://plan', revision: 'v1' }, doneWhen: ['Synthetic check'],
  scope: { allowed: ['Synthetic task'], excluded: ['Real work'], reserved: ['Spending'],
    authorityRefs: [{ uri: 'fixture://operator', revision: 'v1' }] } };
async function fixture(t, delivery) {
  const root = await mkdtemp(join(tmpdir(), 'edda-inbox-test-'));
  let channel;
  const messages = [];
  const deliver = delivery || ((text) => {
    messages.push(text); channel.event('agent_start'); channel.messageStarted(text);
    channel.event('assistant_end', { stopReason: 'stop', text: 'Synthetic response completed' }); channel.settled();
  });
  channel = await startChannel({ root, sessionId: randomUUID(), cwd: root, deliver });
  await enroll(root, channel.sessionId, 'Synthetic fixture; no new spend.');
  await prepareHandoff(root, channel.sessionId, manifest);
  t.after(async () => { await channel.close(); await rm(root, { recursive: true, force: true }); });
  const report = () => {
    channel.event('agent_start');
    const id = randomUUID();
    const value = { manifestRevision: channel.handoffContext().manifestRevision, reportedState: 'waiting_decision',
      stage: 'fixture', summary: 'Need fixture continuation', nextStep: 'Read existing operator reference', evidence: [], dependencies: [],
      decision: { question: 'May fixture continue?', requestedAction: 'fixture_continue', resource: 'offline-only', recommendation: 'Use existing authorization' } };
    channel.reportHandoff(id, value); channel.settled();
    return listInbox(root, { limit: 50 }).events.filter((e) => e.kind === 'decision_request').at(-1).eventId;
  };
  const authorization = { requestedAction: 'fixture_continue', resource: 'offline-only',
    source: { uri: 'fixture://operator-approved', revision: 'v1' }, note: 'Operator already authorized this synthetic operation.' };
  return { root, channel, messages, report, authorization };
}

test('decision events are durable/deduplicated; read acknowledgement never clears unanswered decisions', async (t) => {
  const f = await fixture(t), id = f.report();
  f.channel.settled();
  assert.equal(listInbox(f.root).events.length, 1);
  acknowledgeInbox(f.root, id, 'manager-one');
  assert.equal(listInbox(f.root, { consumer: 'manager-one' }).events[0].read, true);
  assert.equal(listInbox(f.root, { consumer: 'manager-two' }).events[0].read, false);
  const detail = await readInbox(f.root, id);
  assert.equal(detail.event.kind, 'decision_request');
  assert.equal(detail.delivery.notified, false);
  assert.equal(detail.delivery.response, null);
  assert.equal(wakeCapability().status, 'unsupported');
});

test('same scoped authorization is a reuse candidate; explicit response executes once', async (t) => {
  const f = await fixture(t), first = f.report();
  const grant = await recordAuthorization(f.root, first, f.authorization);
  const response = await respondInbox(f.root, first, { message: 'Continue the synthetic operation.', authorizationId: grant.id });
  assert.equal(response.status, 'settled');
  assert.equal((await respondInbox(f.root, first, { message: 'Continue the synthetic operation.', authorizationId: grant.id })).status, 'settled');
  assert.equal(f.messages.length, 1);
  const second = f.report();
  assert.deepEqual((await readInbox(f.root, second)).authorizations.map((r) => r.id), [grant.id]);
  assert.equal(f.messages.length, 1, 'candidate matching never auto-sends');
  await assert.rejects(respondInbox(f.root, first, { message: 'Different instruction' }), /different manager response/);
});

test('authorization records are optional; revoked or mismatched evidence warns but does not block explicit response', async (t) => {
  const f = await fixture(t), first = f.report();
  const grant = await recordAuthorization(f.root, first, f.authorization);
  revokeAuthorization(f.root, grant.id);
  assert.equal((await readInbox(f.root, first)).authorizations.length, 0);
  const result = await respondInbox(f.root, first, { message: 'Proceed within the existing fixture scope.', authorizationId: grant.id });
  assert.equal(result.status, 'settled');
  assert.ok(result.authorizationWarning);
  assert.ok(!f.messages[0].includes('Existing manager-declared'));
});

test('new work, scope change and receiver replacement reject stale automatic replies', async (t) => {
  const f = await fixture(t), first = f.report();
  f.channel.event('agent_start'); f.channel.settled();
  await assert.rejects(respondInbox(f.root, first, { message: 'Stale' }), /no longer current/);
  const second = f.report();
  await enroll(f.root, f.channel.sessionId, 'Changed scope');
  await assert.rejects(respondInbox(f.root, second, { message: 'Stale scope' }), /scope changed/);
  await f.channel.close();
  const replacement = await startChannel({ root: f.root, sessionId: f.channel.sessionId, cwd: f.root, deliver() {} });
  try {
    assert.ok(listInbox(f.root, { limit: 50 }).events.some((r) => r.eventId === second));
    await assert.rejects(respondInbox(f.root, second, { message: 'Wrong instance' }), /instance changed/);
  } finally { await replacement.close(); }
});

test('unknown response is not retried and remains inspectable after read acknowledgement', async (t) => {
  let calls = 0;
  const f = await fixture(t, () => { calls++; throw new Error('uncertain runtime acceptance'); });
  const id = f.report();
  assert.equal((await respondInbox(f.root, id, { message: 'Fixture' })).status, 'unknown');
  acknowledgeInbox(f.root, id);
  assert.equal((await respondInbox(f.root, id, { message: 'Fixture' })).status, 'unknown');
  assert.equal(calls, 1);
  assert.equal((await readInbox(f.root, id)).delivery.response.status, 'unknown');
  assert.equal(listInbox(f.root).events.length, 1);
});

test('reportless settlement produces bounded public activity without inferring approval', async (t) => {
  const f = await fixture(t);
  f.channel.event('agent_start');
  f.channel.event('assistant_end', { stopReason: 'stop', text: '請批准'.repeat(1000) });
  f.channel.settled();
  const id = listInbox(f.root).events[0].eventId;
  const event = (await readInbox(f.root, id)).event;
  assert.equal(event.kind, 'unread_activity');
  assert.equal(event.report, undefined);
  assert.ok(event.excerpt.truncated);
  assert.ok(Buffer.byteLength(event.excerpt.text) <= 2400);
  assert.ok(!publicExcerpt('你', 2).text.includes('\uFFFD'));
  assert.equal((await respondInbox(f.root, id, { message: 'Already authorized; continue.' })).status, 'settled');
});

test('pending publication recovers and malformed inbox setup does not block original channel', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-inbox-error-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = inboxStore(root, true), id = digest('pending-fixture');
  store.put('pending', id, { sessionId: 'fixture', recipient: 'local-manager', kind: 'unread_activity' });
  store.recover();
  assert.ok(store.read('events', id));
  assert.equal(store.list('pending').length, 0);
  const badRoot = join(root, 'bad');
  await mkdir(join(badRoot, 'inbox'), { recursive: true });
  await writeFile(join(badRoot, 'inbox', 'events'), 'cannot create event directory');
  let calls = 0;
  const channel = await startChannel({ root: badRoot, sessionId: randomUUID(), cwd: root, deliver: () => { calls++; } });
  try {
    assert.equal(channel.snapshot().inbox.status, 'storage_error');
    const result = await requestSession(badRoot, channel.sessionId, '/messages', { id: randomUUID(), message: 'Continue', sender: 'manager', mode: 'followUp' });
    assert.equal(result.status, 'unconfirmed');
    assert.equal(calls, 1);
    await prepareHandoff(badRoot, channel.sessionId, manifest);
    assert.equal(channel.handoffContext().status, 'ready');
    channel.event('agent_start');
    assert.equal(channel.reportHandoff(randomUUID(), { manifestRevision: channel.handoffContext().manifestRevision,
      reportedState: 'paused', stage: 'fixture', summary: 'Valid report despite broken telemetry', nextStep: 'Original work can continue',
      evidence: [], dependencies: [] }).status, 'recorded');
    channel.settled();
  } finally { await channel.close(); }
});

test('reportless session can receive an explicit inbox response without enrollment or a manifest', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-inbox-unmanaged-'));
  const messages = [];
  const channel = await startChannel({ root, sessionId: randomUUID(), cwd: root, deliver: (text) => messages.push(text) });
  t.after(async () => { await channel.close(); await rm(root, { recursive: true, force: true }); });
  channel.event('agent_start');
  channel.event('assistant_end', { stopReason: 'stop', text: 'Waiting for a concrete instruction' });
  channel.settled();
  const id = listInbox(root).events[0].eventId;
  assert.equal((await respondInbox(root, id, { message: 'Continue the already-authorized task.' })).status, 'unconfirmed');
  assert.equal(messages.length, 1);
  assert.equal(channel.handoffContext().status, 'not_prepared');
});
