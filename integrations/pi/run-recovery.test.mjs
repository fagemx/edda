import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { digest, sessionDir } from './store.mjs';
import { messageId } from './inbox-store.mjs';
import { enrollRecovery, revokeRecovery, readRecovery, listRecovery, recoveryDecision, recoveryPass,
  reconnectMessage } from './recovery.mjs';

// Hermetic: a disposable temp registry, no real Pi launch, no network beyond a
// loopback stub. Clear any ambient mailbox selector so a managed session cannot
// change where these tests read.
delete process.env.EDDA_RETURN_ROOT;
delete process.env.EDDA_OWNER_REF;
delete process.env.EDDA_RETURN_OWNER;

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

async function fixture(t) {
  const dir = await mkdtemp(join(tmpdir(), 'edda-recovery-impl-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const root = join(dir, 'registry');
  await mkdir(root, { recursive: true });
  return { dir, root };
}

// A managed run record. `phase` drives `managedStatus` fallback; `owner: null`
// produces a session-addressed run that enrollment must refuse.
async function managedRun(root, { runId = randomUUID(), sessionId = randomUUID(), owner = 'assistant/x',
  phase = 'ready', stateKind = 'json', runner = null } = {}) {
  const dir = join(root, 'managed', runId);
  await mkdir(join(dir, 'sessions'), { recursive: true });
  await writeFile(join(dir, 'config.json'), JSON.stringify({ version: 1, runId, root: resolve(root),
    project: resolve(root), prompt: 'SECRET-PROMPT', provider: 'test-provider', model: 'test-model',
    release: { id: 'a'.repeat(64) }, ...(owner ? { owner } : {}) }));
  if (stateKind === 'json') {
    await writeFile(join(dir, 'state.json'), JSON.stringify({ runId, sessionId, ...(owner ? { owner } : {}), phase,
      sessionFile: join(dir, 'sessions', 'x.jsonl'), updatedAt: '2026-09-13T00:00:00.000Z' }));
  } else if (stateKind === 'nul') {
    await writeFile(join(dir, 'state.json'), Buffer.alloc(1887, 0));
  }
  if (runner) {
    await writeFile(join(dir, 'owner.json'), JSON.stringify({ runId, serviceId: runner.serviceId, pid: 4242,
      port: runner.port, token: 'b'.repeat(64) }));
  }
  return { runId, sessionId, dir };
}

// Point a run's Pi channel at a loopback `port` so `requestSession` reaches it.
async function channelOwner(root, sessionId, port) {
  const dir = sessionDir(root, sessionId);
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, 'owner.json'), JSON.stringify({ sessionId, instanceId: randomUUID(), pid: 4242,
    port, token: 'c'.repeat(64) }));
}

async function writeSupervision(root, sessionId, value) {
  const dir = join(root, 'supervision');
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, `${digest(sessionId)}.json`), JSON.stringify({ sessionId, ...value }));
}

// A closed loopback port: connects are refused, which `requestSession` reports as
// an unknown delivery outcome.
async function closedPort() {
  const server = createServer();
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  const port = server.address().port;
  await new Promise((done) => server.close(done));
  return port;
}

// A stub runner/status and message endpoint. Records each POST /messages body.
async function runnerServer(t, { runId, sessionId, live = true, messageStatus = 'accepted', owner = 'assistant/x' }) {
  const serviceId = randomUUID();
  const messages = [];
  const server = createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/status') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ runId, serviceId, sessionId, owner, live, status: live ? 'ready' : 'stopped' }));
      return;
    }
    if (req.method === 'POST' && req.url === '/messages') {
      const chunks = [];
      req.on('data', (chunk) => chunks.push(chunk));
      req.on('end', () => {
        const body = JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}');
        messages.push(body);
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ sessionId, id: body.id, status: messageStatus, accepted: true }));
      });
      return;
    }
    res.writeHead(404, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: 'not found' }));
  });
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  t.after(() => new Promise((done) => server.close(done)));
  return { serviceId, port: server.address().port, messages };
}

const basePolicy = (overrides = {}) => ({ version: 1, enabled: true, scope: 'bounded scope',
  maxAttempts: 3, cooldownMs: 60000, attempts: [], ...overrides });

test('enroll refuses an unowned run and names the adoption fix', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, { owner: null });
  await assert.rejects(enrollRecovery(root, run.runId, { scope: 'do the thing' }), (error) => {
    assert.match(error.message, /not owner-bound/);
    assert.match(error.message, new RegExp(`owner adopt --run ${run.runId} --owner`));
    assert.match(error.message, new RegExp(`run-resume ${run.runId} --runtime current`));
    return true;
  });
  assert.equal(readRecovery(root, run.runId), null);
});

test('enroll validates scope and limits and records the runner identity', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, {});
  await assert.rejects(enrollRecovery(root, run.runId, {}), /scope/i);
  await assert.rejects(enrollRecovery(root, run.runId, { scope: 'x', maxAttempts: 0 }), /max-attempts/);
  await assert.rejects(enrollRecovery(root, run.runId, { scope: 'x', maxAttempts: 11 }), /max-attempts/);
  await assert.rejects(enrollRecovery(root, run.runId, { scope: 'x', cooldownMs: -1 }), /cooldown-ms/);
  await assert.rejects(enrollRecovery(root, run.runId, { scope: 'x'.repeat(8001) }), /scope/i);
  assert.equal(readRecovery(root, randomUUID()), null);
  const policy = await enrollRecovery(root, run.runId, { scope: 'do the thing' });
  assert.equal(policy.runId, run.runId);
  assert.equal(policy.sessionId, run.sessionId);
  assert.equal(policy.owner, 'assistant/x');
  assert.equal(policy.maxAttempts, 3);
  assert.equal(policy.cooldownMs, 60000);
  assert.equal(policy.enabled, true);
  assert.deepEqual(policy.attempts, []);
  assert.ok(Number.isFinite(Date.parse(policy.enrolledAt)));
});

test('revoke disables the policy, keeps history and makes it not enrolled', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, {});
  await enrollRecovery(root, run.runId, { scope: 'do the thing' });
  const status = { live: false, lastRecordedPhase: 'ready', continuity: 'owner-bound', owner: 'assistant/x' };
  const before = recoveryDecision({ policy: readRecovery(root, run.runId), status });
  assert.equal(before.decision, 'eligible');
  const revoked = revokeRecovery(root, run.runId, { reason: 'operator decision' });
  assert.equal(revoked.enabled, false);
  assert.equal(revoked.revokedReason, 'operator decision');
  const after = recoveryDecision({ policy: readRecovery(root, run.runId), status });
  assert.deepEqual(after, { decision: 'skip', reason: 'not_enrolled' });
  assert.throws(() => revokeRecovery(root, randomUUID(), {}), /no recovery enrollment/);
});

test('enroll keeps prior attempts when re-scoping an enrolled run', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, {});
  const first = await enrollRecovery(root, run.runId, { scope: 'first' });
  const withHistory = { ...first, attempts: [{ at: new Date().toISOString(), outcome: 'refused', reason: 'x', reconnect: 'none' }] };
  const path = join(run.dir, 'recovery.json');
  await writeFile(path, JSON.stringify(withHistory));
  const again = await enrollRecovery(root, run.runId, { scope: 'second', maxAttempts: 5 });
  assert.equal(again.scope, 'second');
  assert.equal(again.maxAttempts, 5);
  assert.equal(again.attempts.length, 1);
  assert.equal(again.enabled, true);
});

test('recoveryDecision classifies each boundary with its own reason', () => {
  const now = Date.parse('2026-09-13T12:00:00.000Z');
  const status = { live: false, lastRecordedPhase: 'ready', continuity: 'owner-bound', owner: 'assistant/x' };
  assert.deepEqual(recoveryDecision({}), { decision: 'skip', reason: 'not_enrolled' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy({ enabled: false }), status, now }),
    { decision: 'skip', reason: 'not_enrolled' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy(), status: { ...status, live: true }, now }),
    { decision: 'skip', reason: 'live_holder' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy(), status: { ...status, lastRecordedPhase: 'stopped' }, now }),
    { decision: 'skip', reason: 'intentionally_stopped' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy(), status: { ...status, lastRecordedPhase: 'stopping' }, now }),
    { decision: 'attention', reason: 'stopping_unproven' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy(), status, supervision: { enabled: false, action: 'paused' }, now }),
    { decision: 'skip', reason: 'paused' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy(), status: { live: false, status: 'record_unavailable', lastRecordedPhase: null }, now }),
    { decision: 'attention', reason: 'record_unavailable' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy(), status: { ...status, continuity: 'session-addressed', owner: null }, now }),
    { decision: 'attention', reason: 'owner_missing' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy(), status: { ...status, owner: null }, now }),
    { decision: 'attention', reason: 'owner_missing' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy({ maxAttempts: 'many' }), status, now }),
    { decision: 'attention', reason: 'policy_invalid' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy({ cooldownMs: 1.5 }), status, now }),
    { decision: 'attention', reason: 'policy_invalid' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy({ attempts: [{ outcome: 'refused' }] }), status, now }),
    { decision: 'attention', reason: 'policy_invalid' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy({ attempts: [{ at: '2026-09-13T11:59:30.000Z' }] }), status, now }),
    { decision: 'skip', reason: 'cooldown' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy({ attempts: [
    { at: '2026-09-13T11:00:00.000Z' }, { at: '2026-09-13T11:01:00.000Z' }, { at: '2026-09-13T11:02:00.000Z' }] }), status, now }),
    { decision: 'skip', reason: 'attempts_exhausted' });
  assert.deepEqual(recoveryDecision({ policy: basePolicy(), status, now }),
    { decision: 'eligible', reason: 'eligible' });
});

test('a pass never resumes a live holder, a stop, a pause, a corrupt record or an exhausted run', async (t) => {
  const { root } = await fixture(t);
  const results = {};
  const resumeCalls = [];

  // live holder: a reachable runner status reports live
  const live = await managedRun(root, {});
  await enrollRecovery(root, live.runId, { scope: 'x' });
  const server = await runnerServer(t, { runId: live.runId, sessionId: live.sessionId, live: true });
  await writeFile(join(live.dir, 'owner.json'), JSON.stringify({ runId: live.runId, serviceId: server.serviceId,
    pid: 4242, port: server.port, token: 'b'.repeat(64) }));

  // intentional stop: unreachable runner with a recorded stopped phase
  const stopped = await managedRun(root, { phase: 'stopped' });
  await enrollRecovery(root, stopped.runId, { scope: 'x' });

  // paused: an existing supervision pause marker
  const paused = await managedRun(root, {});
  await enrollRecovery(root, paused.runId, { scope: 'x' });
  await writeSupervision(root, paused.sessionId, { enabled: false, action: 'paused' });

  // record unavailable: corrupt state.json, never repaired
  const corrupt = await managedRun(root, {});
  await enrollRecovery(root, corrupt.runId, { scope: 'x' });
  const corruptPath = join(corrupt.dir, 'state.json');
  await writeFile(corruptPath, Buffer.alloc(1887, 0));

  // attempts exhausted
  const exhausted = await managedRun(root, {});
  await enrollRecovery(root, exhausted.runId, { scope: 'x' });
  const policy = readRecovery(root, exhausted.runId);
  await writeFile(join(exhausted.dir, 'recovery.json'), JSON.stringify({ ...policy,
    attempts: [{ at: '2026-09-13T11:00:00.000Z', outcome: 'refused', reason: 'x', reconnect: 'none' },
      { at: '2026-09-13T11:01:00.000Z', outcome: 'refused', reason: 'x', reconnect: 'none' },
      { at: '2026-09-13T11:02:00.000Z', outcome: 'refused', reason: 'x', reconnect: 'none' }] }));

  const now = Date.parse('2026-09-13T12:30:00.000Z');
  const pass = await recoveryPass(root, { max: 5, now, ownerReturns: async () => ({ status: 'ok', pending: 0 }),
    resume: async (_root, run) => { resumeCalls.push(run); return { live: true }; } });
  for (const entry of pass.results) results[entry.runId] = entry;
  assert.equal(resumeCalls.length, 0);
  assert.equal(pass.resumed, 0);
  assert.equal(results[live.runId].reason, 'live_holder');
  assert.equal(results[stopped.runId].reason, 'intentionally_stopped');
  assert.equal(results[paused.runId].reason, 'paused');
  assert.equal(results[corrupt.runId].decision, 'attention');
  assert.equal(results[corrupt.runId].reason, 'record_unavailable');
  assert.equal(results[exhausted.runId].reason, 'attempts_exhausted');
});

test('an unreadable or identity-mismatched supervision record is attention, never resumed', async (t) => {
  const { root } = await fixture(t);
  const resumeCalls = [];
  const resume = async (_root, id) => { resumeCalls.push(id); return { live: true }; };

  // unreadable: NUL bytes where a pause marker would be
  const corrupt = await managedRun(root, {});
  await enrollRecovery(root, corrupt.runId, { scope: 'x' });
  await mkdir(join(root, 'supervision'), { recursive: true });
  await writeFile(join(root, 'supervision', `${digest(corrupt.sessionId)}.json`), Buffer.alloc(200, 0));

  // identity mismatch: a readable record that names another session
  const mismatch = await managedRun(root, {});
  await enrollRecovery(root, mismatch.runId, { scope: 'x' });
  await writeSupervision(root, mismatch.sessionId, { sessionId: 'other-session', enabled: true, action: 'observed' });

  // control: a readable pause is a skip, not attention
  const paused = await managedRun(root, {});
  await enrollRecovery(root, paused.runId, { scope: 'x' });
  await writeSupervision(root, paused.sessionId, { enabled: false, action: 'paused' });

  const pass = await recoveryPass(root, { max: 5, resume, ownerReturns: async () => ({ status: 'ok', pending: 0 }) });
  const results = Object.fromEntries(pass.results.map((entry) => [entry.runId, entry]));
  assert.equal(results[corrupt.runId].decision, 'attention');
  assert.equal(results[corrupt.runId].reason, 'supervision_unavailable');
  assert.equal(results[mismatch.runId].decision, 'attention');
  assert.equal(results[mismatch.runId].reason, 'supervision_unavailable');
  assert.equal(results[paused.runId].decision, 'skip');
  assert.equal(results[paused.runId].reason, 'paused');
  assert.equal(resumeCalls.length, 0);
  assert.equal(pass.resumed, 0);
});

test('an enrolled run whose owner identity is gone is attention, never resumed', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, {});
  await enrollRecovery(root, run.runId, { scope: 'x' });
  await writeFile(join(run.dir, 'config.json'), JSON.stringify({ version: 1, runId: run.runId, root: resolve(root),
    project: resolve(root), release: { id: 'a'.repeat(64) } }));
  await writeFile(join(run.dir, 'state.json'), JSON.stringify({ runId: run.runId, sessionId: run.sessionId, phase: 'ready' }));
  let called = false;
  const pass = await recoveryPass(root, { resume: async () => { called = true; return { live: true }; },
    ownerReturns: async () => ({ status: 'ok', pending: 0 }) });
  const entry = pass.results.find((r) => r.runId === run.runId);
  assert.equal(entry.decision, 'attention');
  assert.equal(entry.reason, 'owner_missing');
  assert.equal(called, false);
});

test('a malformed policy surfaces as policy_invalid and is never resumed', async (t) => {
  const { root } = await fixture(t);
  const resumeCalls = [];
  const resume = async (_root, id) => { resumeCalls.push(id); return { live: true }; };

  const badLimit = await managedRun(root, {});
  await enrollRecovery(root, badLimit.runId, { scope: 'x' });
  await writeFile(join(badLimit.dir, 'recovery.json'), JSON.stringify({ ...readRecovery(root, badLimit.runId), maxAttempts: 'many' }));

  const badCooldown = await managedRun(root, {});
  await enrollRecovery(root, badCooldown.runId, { scope: 'x' });
  await writeFile(join(badCooldown.dir, 'recovery.json'), JSON.stringify({ ...readRecovery(root, badCooldown.runId), cooldownMs: 1.5 }));

  const badAttempt = await managedRun(root, {});
  await enrollRecovery(root, badAttempt.runId, { scope: 'x' });
  await writeFile(join(badAttempt.dir, 'recovery.json'), JSON.stringify({ ...readRecovery(root, badAttempt.runId),
    attempts: [{ outcome: 'refused', reason: 'x', reconnect: 'none' }] }));

  const pass = await recoveryPass(root, { max: 5, resume, ownerReturns: async () => ({ status: 'ok', pending: 0 }) });
  assert.equal(resumeCalls.length, 0);
  for (const run of [badLimit, badCooldown, badAttempt]) {
    const entry = pass.results.find((r) => r.runId === run.runId);
    assert.equal(entry.decision, 'attention');
    assert.equal(entry.reason, 'policy_invalid');
  }
});

test('a stopping run is not proven dead and is never resumed', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, { phase: 'stopping' });
  await enrollRecovery(root, run.runId, { scope: 'x' });
  let called = false;
  const pass = await recoveryPass(root, { resume: async () => { called = true; return { live: true }; },
    ownerReturns: async () => ({ status: 'ok', pending: 0 }) });
  const entry = pass.results.find((r) => r.runId === run.runId);
  assert.equal(entry.decision, 'attention');
  assert.equal(entry.reason, 'stopping_unproven');
  assert.equal(called, false);
});

test('an eligible run resumes once, sends one deterministic reconnect, then cools down', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, {});
  await enrollRecovery(root, run.runId, { scope: 'fix the parser only' });
  const server = await runnerServer(t, { runId: run.runId, sessionId: run.sessionId, live: true });
  await channelOwner(root, run.sessionId, server.port);

  const resumeCalls = [];
  const resume = async (_root, id) => { resumeCalls.push(id); return { live: true }; };
  const now = Date.parse('2026-09-13T12:00:00.000Z');
  const pass = await recoveryPass(root, { max: 1, now, resume,
    ownerReturns: async () => ({ status: 'ok', pending: 2 }) });
  assert.equal(pass.status, 'recovery_pass');
  assert.equal(pass.considered, 1);
  assert.equal(pass.resumed, 1);
  assert.deepEqual(resumeCalls, [run.runId]);
  assert.equal(pass.results[0].outcome, 'resumed');
  assert.equal(pass.results[0].reconnect, 'sent');

  const expectedId = messageId(digest(`edda-recovery-v1:${run.runId}:1`));
  assert.equal(server.messages.length, 1);
  assert.equal(server.messages[0].id, expectedId);
  assert.equal(server.messages[0].mode, 'followUp');
  assert.match(server.messages[0].message, /bounded recovery/i);
  assert.match(server.messages[0].message, /not the original prompt/i);
  assert.match(server.messages[0].message, /fix the parser only/);
  assert.match(server.messages[0].message, /milestone|stopping reason/i);
  assert.ok(Buffer.byteLength(server.messages[0].message) <= 1200);

  const stored = readRecovery(root, run.runId);
  assert.equal(stored.attempts.length, 1);
  assert.equal(stored.attempts[0].outcome, 'resumed');
  assert.equal(stored.attempts[0].reconnect, 'sent');

  // An immediate second pass is refused by cooldown and sends no second message.
  const second = await recoveryPass(root, { max: 1, now: now + 1000, resume,
    ownerReturns: async () => ({ status: 'ok', pending: 0 }) });
  assert.equal(second.resumed, 0);
  assert.equal(second.results[0].reason, 'cooldown');
  assert.equal(resumeCalls.length, 1);
  assert.equal(server.messages.length, 1);
});

test('a throwing resume is recorded as refused and never throws out of the pass', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, {});
  await enrollRecovery(root, run.runId, { scope: 'x' });
  const now = Date.parse('2026-09-13T12:00:00.000Z');
  const pass = await recoveryPass(root, { max: 1, now,
    resume: async () => { throw new Error('session file identity mismatch'); },
    ownerReturns: async () => ({ status: 'ok', pending: 0 }) });
  assert.equal(pass.resumed, 0);
  assert.equal(pass.results[0].outcome, 'refused');
  assert.equal(pass.results[0].reconnect, 'none');
  const stored = readRecovery(root, run.runId);
  assert.equal(stored.attempts.length, 1);
  assert.equal(stored.attempts[0].outcome, 'refused');
  assert.match(stored.attempts[0].reason, /identity mismatch/);
});

test('max bounds how many runs are resumed in one pass', async (t) => {
  const { root } = await fixture(t);
  const a = await managedRun(root, {}), b = await managedRun(root, {});
  await enrollRecovery(root, a.runId, { scope: 'x' });
  await enrollRecovery(root, b.runId, { scope: 'x' });
  const resumeCalls = [];
  const now = Date.parse('2026-09-13T12:00:00.000Z');
  const pass = await recoveryPass(root, { max: 1, now,
    resume: async (_root, id) => { resumeCalls.push(id); return { live: true }; },
    ownerReturns: async () => ({ status: 'ok', pending: 0 }) });
  assert.equal(resumeCalls.length, 1);
  assert.equal(pass.resumed, 1);
  const deferred = pass.results.filter((entry) => entry.outcome === 'deferred');
  assert.equal(deferred.length, 1);
  assert.equal(deferred[0].decision, 'eligible');
  // The deferred run recorded no attempt.
  assert.equal(readRecovery(root, deferred[0].runId).attempts.length, 0);
});

test('an unknown reconnect receipt is recorded and not resent on an immediate pass', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, {});
  await enrollRecovery(root, run.runId, { scope: 'x' });
  await channelOwner(root, run.sessionId, await closedPort());
  const now = Date.parse('2026-09-13T12:00:00.000Z');
  const pass = await recoveryPass(root, { max: 1, now,
    resume: async () => ({ live: true }),
    ownerReturns: async () => ({ status: 'ok', pending: 0 }) });
  assert.equal(pass.results[0].outcome, 'resumed');
  assert.equal(pass.results[0].reconnect, 'unknown');
  const stored = readRecovery(root, run.runId);
  assert.equal(stored.attempts.length, 1);
  assert.equal(stored.attempts[0].reconnect, 'unknown');
  const second = await recoveryPass(root, { max: 1, now: now + 1000,
    resume: async () => { throw new Error('must not resume again'); },
    ownerReturns: async () => ({ status: 'ok', pending: 0 }) });
  assert.equal(second.resumed, 0);
  assert.equal(second.results[0].reason, 'cooldown');
});

test('a pass for an unknown single run is not_enrolled, not attention', async (t) => {
  const { root } = await fixture(t);
  const run = await managedRun(root, {});
  let called = false;
  const pass = await recoveryPass(root, { runId: run.runId,
    resume: async () => { called = true; return { live: true }; } });
  assert.equal(pass.considered, 1);
  assert.equal(pass.resumed, 0);
  assert.deepEqual(pass.results[0], { runId: run.runId, decision: 'skip', reason: 'not_enrolled',
    outcome: null, reconnect: 'none', receipt: null });
  assert.equal(called, false);
});

test('listRecovery is bounded to managed run directories and surfaces a corrupt policy', async (t) => {
  const { root } = await fixture(t);
  const good = await managedRun(root, {});
  await enrollRecovery(root, good.runId, { scope: 'x' });
  const bad = await managedRun(root, {});
  await enrollRecovery(root, bad.runId, { scope: 'x' });
  await writeFile(join(bad.dir, 'recovery.json'), Buffer.alloc(64, 0));
  await mkdir(join(root, 'managed', 'not-a-run'), { recursive: true });
  await writeFile(join(root, 'managed', 'not-a-run', 'recovery.json'), JSON.stringify({ runId: 'nope' }));
  const listed = listRecovery(root);
  assert.equal(listed.length, 2);
  const goodEntry = listed.find((entry) => entry.runId === good.runId);
  const badEntry = listed.find((entry) => entry.runId === bad.runId);
  assert.equal(goodEntry.policy.scope, 'x');
  assert.equal(badEntry.policy, null);
  assert.equal(badEntry.error.code, 'record_unavailable');
});

test('reconnectMessage stays bounded and names the declared scope', () => {
  const text = reconnectMessage({ runId: 'r', scope: 's'.repeat(8000), attempt: 1, pending: 3 });
  assert.ok(Buffer.byteLength(text) <= 1200);
  assert.match(text, /bounded recovery/i);
  assert.match(text, /not the original prompt/i);
  assert.match(text, /pending for your owner: 3/);
});
