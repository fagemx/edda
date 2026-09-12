import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { startChannel } from './channel.mjs';
import { requestSession } from './client.mjs';
import { enroll, watch, managementBrief } from './supervision.mjs';

export const manifest = () => ({
  version: 1, runId: 'demo-run-46', role: 'controller', goal: 'Deliver the approved offline schema change',
  planRef: { uri: 'docs/plan.md', revision: 'plan-v3' }, doneWhen: ['Focused tests pass', 'Independent review accepts'],
  scope: { allowed: ['Work in the test fixture only'], excluded: ['No production DB'], reserved: ['New spend'],
    authorityRefs: [{ uri: 'operator:2026-09-12', revision: 'instruction-1' }] },
});
export const report = (revision, reportedState = 'working') => ({
  manifestRevision: revision, reportedState, stage: 'schema', summary: 'Columns and constraints prepared',
  nextStep: 'Run focused tests', evidence: [], dependencies: [],
});
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'edda-handoff-test-'));
  let channel;
  t.after(async () => { await channel?.close(); await rm(root, { recursive: true, force: true }); });
  channel = await startChannel({ root, sessionId: randomUUID(), cwd: root, deliver() {},
    getConversation() { throw new Error('Handoff must not read conversations'); } });
  const prepare = (value, expectedRevision = null) => requestSession(root, channel.sessionId, '/handoff/manifest', { manifest: value, expectedRevision });
  const context = (budget = 16384) => requestSession(root, channel.sessionId, `/handoff?budget=${budget}`);
  return { root, channel, prepare, context };
}

test('prepared manifest yields bounded supervisor context without a transcript', async (t) => {
  const { prepare, context } = await fixture(t);
  const first = await prepare(manifest());
  assert.equal(first.status, 'ready');
  assert.equal(first.attention, 'not_started');
  assert.equal(first.manifest.goal, manifest().goal);
  assert.equal(first.authority, 'declared_context_only');
  assert.equal(first.serializedBytes, Buffer.byteLength(JSON.stringify(first)));
  assert.equal((await context(512)).status, 'needs_context');
  assert.equal((await prepare(manifest())).manifestRevision, first.manifestRevision);
});

test('new work invalidates old report; silence after settlement needs attention', async (t) => {
  const { channel, prepare, context } = await fixture(t);
  const card = await prepare(manifest());
  channel.event('agent_start');
  channel.settled();
  assert.equal((await context()).attention, 'missing_report');
  channel.event('agent_start');
  const value = { ...report(card.manifestRevision, 'waiting_decision'), decision: {
    question: 'May the test fixture migration proceed?', requestedAction: 'test_db_migration',
    resource: 'test-db', recommendation: 'Approve under the existing test-only scope',
  } };
  const id = randomUUID();
  const ack = channel.reportHandoff(id, value);
  assert.deepEqual(channel.reportHandoff(id, value), ack);
  channel.settled();
  assert.equal((await context()).attention, 'decision_required');
  assert.equal((await context()).report.decision.question, value.decision.question);
  channel.event('agent_start');
  channel.settled();
  assert.equal((await context()).attention, 'missing_report');
});

test('report cannot mutate authority, claim acceptance or attach to wrong manifest', async (t) => {
  const { channel, prepare, context } = await fixture(t);
  const card = await prepare(manifest());
  channel.event('agent_start');
  assert.throws(() => channel.reportHandoff(randomUUID(), { ...report(card.manifestRevision), acceptance: 'accepted' }), /Unknown/);
  assert.throws(() => channel.reportHandoff(randomUUID(), report('old-revision')), /revision/);
  assert.throws(() => channel.reportHandoff(randomUUID(), report(card.manifestRevision, 'completed')), /evidence/);
  assert.throws(() => channel.reportHandoff(randomUUID(), report(card.manifestRevision, 'waiting_decision')), /decision/);
  channel.reportHandoff(randomUUID(), { ...report(card.manifestRevision, 'completed'),
    evidence: [{ uri: 'tests/result.json', revision: 'sha256:example' }] });
  channel.settled();
  const view = await context();
  assert.equal(view.attention, 'completion_pending');
  assert.equal(view.acceptance, 'unverified');
});

test('manifest update requires idle instance and exact expected revision', async (t) => {
  const { channel, prepare } = await fixture(t);
  const card = await prepare(manifest());
  const changed = { ...manifest(), goal: 'New bounded goal' };
  await assert.rejects(prepare(changed), /revision/);
  channel.event('agent_start');
  await assert.rejects(prepare(changed, card.manifestRevision), /idle/);
  channel.settled();
  assert.notEqual((await prepare(changed, card.manifestRevision)).manifestRevision, card.manifestRevision);
});

test('watch returns only an index; on-demand brief keeps manager scope and never reads transcript', async (t) => {
  const { root, channel, prepare } = await fixture(t);
  const card = await prepare(manifest());
  await enroll(root, channel.sessionId, 'Operator delegated this fixture; no live work is allowed.');
  channel.event('agent_start');
  channel.reportHandoff(randomUUID(), { ...report(card.manifestRevision, 'failed'), summary: 'Synthetic gate failed' });
  channel.settled();
  const [row] = await watch(root);
  assert.equal(row.assessment, 'failure_reported');
  assert.equal(row.handoff.runId, 'demo-run-46');
  assert.equal('conversation' in row, false);
  assert.equal('manifest' in row.handoff, false);
  assert.equal('scope' in row.supervision, false);
  const full = await managementBrief(root, channel.sessionId);
  assert.equal(full.report.summary, 'Synthetic gate failed');
  assert.ok(full.supervisorScope.includes('no live work'));
  assert.equal(full.serializedBytes, Buffer.byteLength(JSON.stringify(full)));
  assert.equal((await managementBrief(root, channel.sessionId, 512)).status, 'needs_context');
});

test('reload requires explicit rebind and rejects stale report IDs', async (t) => {
  const { root, channel, prepare } = await fixture(t);
  const card = await prepare(manifest());
  channel.event('agent_start');
  const id = randomUUID();
  const value = { ...report(card.manifestRevision, 'completed'), evidence: [{ uri: 'fixture://done', revision: 'v1' }] };
  channel.reportHandoff(id, value);
  channel.settled();
  await channel.close();
  const next = await startChannel({ root, sessionId: channel.sessionId, cwd: root, deliver() {} });
  try {
    assert.equal(next.handoffContext().attention, 'needs_rebind');
    assert.throws(() => next.reportHandoff(randomUUID(), value), /bound to this instance/);
    await requestSession(root, next.sessionId, '/handoff/manifest', { manifest: manifest(), expectedRevision: card.manifestRevision });
    assert.equal(next.handoffContext().report, null);
    next.event('agent_start');
    assert.throws(() => next.reportHandoff(id, value), /previous instance/);
    next.settled();
    assert.equal(next.handoffContext().attention, 'missing_report');
  } finally { await next.close(); }
});

test('CLI prepares and reads context; insufficient budget is nonzero with no truncated authority', async (t) => {
  const { root, channel } = await fixture(t);
  const cli = fileURLToPath(new URL('./cli.mjs', import.meta.url));
  const input = fileURLToPath(new URL('./fixtures/management-manifest.json', import.meta.url));
  const exec = promisify(execFile);
  const run = (...args) => exec(process.execPath, [cli, ...args], {
    windowsHide: true, env: { ...process.env, EDDA_PI_CHANNEL_DIR: root },
  });
  const prepared = JSON.parse((await run('prepare', channel.sessionId, '--manifest', input)).stdout);
  assert.equal(prepared.manifest.runId, 'offline-handoff-demo');
  const card = JSON.parse((await run('brief', channel.sessionId)).stdout);
  assert.equal(card.manifestRevision, prepared.manifestRevision);
  await assert.rejects(run('brief', channel.sessionId, '--budget-bytes', '512'), (error) => {
    const output = JSON.parse(error.stdout);
    assert.equal(error.code, 2);
    assert.equal(output.status, 'needs_context');
    assert.equal('manifest' in output, false);
    return true;
  });
});
