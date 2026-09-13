import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile, readFile, realpath, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { listManagedRuns } from './activation.mjs';
import { managedStatus, managedConversation } from './managed-client.mjs';
import { listSessions } from './client.mjs';
import { doctor } from './dependency-client.mjs';
import { readJson, readRecord, sessionDir } from './store.mjs';

// Corruption is per record, never a registry outage. The fixture is a disposable
// temp-dir registry: no live registry, no service, no Pi process.
const exec = promisify(execFile), cli = fileURLToPath(new URL('./cli.mjs', import.meta.url));
const NUL = (bytes = 1887) => Buffer.alloc(bytes, 0); // exact audit shape
const parserText = (value) => /Unexpected token|JSON\.parse|is not valid JSON|Unexpected end/.test(JSON.stringify(value));

async function fixture(t) {
  const dir = await mkdtemp(join(tmpdir(), 'edda-recovery-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const root = join(dir, 'registry'), project = join(dir, 'project');
  await mkdir(project, { recursive: true });
  return { dir, root, project: await realpath(project) };
}

async function managedRun(root, project, { stateKind = 'json', config = {}, state = {} } = {}) {
  const runId = randomUUID(), sessionId = randomUUID();
  const dir = join(root, 'managed', runId), sessions = join(dir, 'sessions');
  await mkdir(sessions, { recursive: true });
  await writeFile(join(dir, 'config.json'), JSON.stringify({ version: 1, runId, root: resolve(root), project,
    prompt: 'SECRET-PROMPT', provider: 'test-provider', model: 'test-model', thinking: 'high',
    release: { id: 'a'.repeat(64), path: join(root, 'releases', 'x') }, ...config }));
  const sessionFile = join(sessions, `2026-09-13T00-00-00-000Z_${sessionId}.jsonl`);
  await writeFile(sessionFile, [
    { type: 'session', id: sessionId, cwd: project, timestamp: '2026-09-13T00:00:00.000Z' },
    { id: 'e1', parentId: null, timestamp: '2026-09-13T00:00:01.000Z', type: 'message', message: { role: 'user', content: 'HELLO-RECOVERED' } },
    { id: 'e2', parentId: 'e1', timestamp: '2026-09-13T00:00:02.000Z', type: 'message', message: { role: 'assistant', content: 'REPLY-RECOVERED' } },
  ].map((value) => JSON.stringify(value)).join('\n') + '\n');
  const statePath = join(dir, 'state.json');
  if (stateKind === 'json') {
    await writeFile(statePath, JSON.stringify({ runId, serviceId: randomUUID(), phase: 'stopped', sessionId, sessionFile,
      updatedAt: '2026-09-13T00:00:00.000Z', error: 'SECRET-ERROR', token: 'SECRET-TOKEN', ...state }));
  } else if (stateKind === 'nul') await writeFile(statePath, NUL());
  else if (stateKind === 'invalid') await writeFile(statePath, '{ SECRET-BROKEN-JSON');
  return { runId, dir, sessionId, sessionFile, statePath };
}

async function serviceSession(root, project, { stateKind = 'json', ownerKind = 'json' } = {}) {
  const sessionId = randomUUID(), dir = sessionDir(root, sessionId);
  await mkdir(dir, { recursive: true });
  const ownerPath = join(dir, 'owner.json');
  if (ownerKind === 'json') {
    await writeFile(ownerPath, JSON.stringify({ sessionId, instanceId: randomUUID(), pid: 4242, port: 9, token: 'b'.repeat(64), cwd: project }));
  } else if (ownerKind === 'nul') await writeFile(ownerPath, NUL());
  const statePath = join(dir, 'state.json');
  if (stateKind === 'json') {
    await writeFile(statePath, JSON.stringify({ sessionId, state: 'idle', cwd: project, token: 'SECRET-TOKEN' }));
  } else if (stateKind === 'nul') await writeFile(statePath, NUL());
  else if (stateKind === 'invalid') await writeFile(statePath, '{ SECRET-BROKEN-JSON');
  return { sessionId, dir, statePath, ownerPath };
}

test('run discovery degrades a NUL state per record and keeps config identity', async (t) => {
  const { root, project } = await fixture(t);
  const healthy = await managedRun(root, project);
  const corrupt = await managedRun(root, project, { stateKind: 'nul' });
  const before = await readFile(corrupt.statePath);
  const result = listManagedRuns(root);
  assert.equal(result.runs.length, 2);
  const healthyRow = result.runs.find((r) => r.runId === healthy.runId), corruptRow = result.runs.find((r) => r.runId === corrupt.runId);
  assert.equal(healthyRow.sessionId, healthy.sessionId);
  assert.equal(healthyRow.recordedPhase, 'stopped');
  assert.equal(healthyRow.error, null);
  assert.equal(corruptRow.project, project);
  assert.equal(corruptRow.releaseId, 'a'.repeat(64));
  assert.equal(corruptRow.recordedPhase, 'unknown');
  assert.equal(corruptRow.error.code, 'record_unavailable');
  assert.equal(corruptRow.error.record, 'state.json');
  assert.ok(!parserText(result));
  assert.doesNotMatch(JSON.stringify(result), /SECRET/);
  assert.deepEqual(await readFile(corrupt.statePath), before);
});

test('run-status returns config identity, not a parser error, when state.json is unreadable', async (t) => {
  const { root, project } = await fixture(t);
  const run = await managedRun(root, project, { stateKind: 'nul' });
  const before = await readFile(run.statePath);
  const result = await managedStatus(root, run.runId);
  assert.equal(result.runId, run.runId);
  assert.equal(result.status, 'record_unavailable');
  assert.equal(result.live, false);
  assert.equal(result.project, project);
  assert.deepEqual(result.release, { id: 'a'.repeat(64), path: join(root, 'releases', 'x') });
  assert.equal(result.provider, 'test-provider');
  assert.equal(result.model, 'test-model');
  assert.equal(result.thinking, 'high');
  assert.deepEqual(result.error, { code: 'record_unavailable', record: 'state.json',
    message: 'Record unavailable: state.json; it was not repaired' });
  assert.ok(!parserText(result));
  assert.doesNotMatch(JSON.stringify(result), /SECRET/);
  assert.deepEqual(await readFile(run.statePath), before);
});

test('run-conversation recovers runId -> sessionId and persisted history without state.json', async (t) => {
  const { root, project } = await fixture(t);
  const run = await managedRun(root, project, { stateKind: 'nul' });
  const before = await readFile(run.statePath);
  const result = await managedConversation(root, run.runId);
  assert.equal(result.evidenceSource, 'persisted_session');
  assert.equal(result.sessionId, run.sessionId);
  assert.equal(result.error.code, 'record_unavailable');
  const texts = result.conversation.entries.map((e) => e.text);
  assert.ok(texts.includes('HELLO-RECOVERED'));
  assert.ok(texts.includes('REPLY-RECOVERED'));
  assert.ok(!parserText(result));
  assert.deepEqual(await readFile(run.statePath), before);
});

test('list survives a NUL service state and recovers identity from owner.json', async (t) => {
  const { root, project } = await fixture(t);
  const corrupt = await serviceSession(root, project, { stateKind: 'nul' });
  const before = await readFile(corrupt.statePath);
  const rows = await listSessions(root);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].sessionId, corrupt.sessionId);
  assert.equal(rows[0].status, 'record_unavailable');
  assert.equal(rows[0].state, 'record_unavailable');
  assert.equal(rows[0].live, false);
  assert.deepEqual(rows[0].error, { code: 'record_unavailable', record: 'state.json',
    message: 'Record unavailable: state.json; it was not repaired' });
  assert.ok(!parserText(rows));
  assert.doesNotMatch(JSON.stringify(rows), /SECRET/);
  assert.deepEqual(await readFile(corrupt.statePath), before);
});

test('doctor succeeds against a corrupt record and names it', async (t) => {
  const { root, project } = await fixture(t);
  const corrupt = await serviceSession(root, project, { stateKind: 'nul' });
  await writeFile(join(root, 'not-a-record.txt'), 'ignored');
  const result = await doctor(root);
  assert.equal(result.status, 'diagnosed');
  assert.deepEqual(result.unreadable, [{ sessionId: corrupt.sessionId, record: 'state.json' }]);
  const selected = await doctor(root, corrupt.sessionId);
  assert.equal(selected.sessions[0].readiness, 'record_unavailable');
  assert.equal(selected.sessions[0].error.code, 'record_unavailable');
  assert.equal(selected.sessions[0].error.record, 'state.json');
  assert.ok(!parserText(result) && !parserText(selected));
});

test('a session with no recoverable identity is omitted from list and named by doctor', async (t) => {
  const { root, project } = await fixture(t);
  const blind = await serviceSession(root, project, { stateKind: 'nul', ownerKind: 'nul' });
  assert.deepEqual(await listSessions(root), []);
  const result = await doctor(root);
  assert.deepEqual(result.unreadable, [{ sessionId: null, record: 'state.json' }]);
  const selected = await doctor(root, blind.sessionId);
  assert.equal(selected.status, 'record_unavailable');
  assert.equal(selected.sessionId, blind.sessionId);
});

test('an absent state record keeps the pre-existing unreachable shape, not record_unavailable', async (t) => {
  const { root, project } = await fixture(t);
  await serviceSession(root, project, { stateKind: 'missing' });
  const rows = await listSessions(root);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].state, 'unreachable');
  assert.equal(rows[0].error, undefined);
  assert.deepEqual((await doctor(root)).unreadable, []);
});

test('a non-NUL invalid record degrades exactly like a NUL record', async (t) => {
  const { root, project } = await fixture(t);
  const nul = await managedRun(root, project, { stateKind: 'nul' });
  const invalid = await managedRun(root, project, { stateKind: 'invalid' });
  const nulStatus = await managedStatus(root, nul.runId), invalidStatus = await managedStatus(root, invalid.runId);
  assert.equal(invalidStatus.status, 'record_unavailable');
  assert.deepEqual(invalidStatus.error, nulStatus.error);
  assert.ok(!parserText(invalidStatus));
  assert.doesNotMatch(JSON.stringify(invalidStatus), /SECRET/);
  const brokenConfig = await managedRun(root, project);
  await writeFile(join(brokenConfig.dir, 'config.json'), NUL());
  const brokenRow = listManagedRuns(root).runs.find((r) => r.runId === brokenConfig.runId);
  assert.equal(brokenRow.error.code, 'record_unavailable');
  assert.equal(brokenRow.error.record, 'config.json');
  assert.equal(brokenRow.project, null);
});

test('readRecord classifies unparseable, oversized and non-file records without parser text', async (t) => {
  const { dir } = await fixture(t);
  const nulPath = join(dir, 'nul.json');
  await writeFile(nulPath, NUL());
  const invalidPath = join(dir, 'invalid.json');
  await writeFile(invalidPath, '{ SECRET-BROKEN-JSON');
  const oversizedPath = join(dir, 'oversized.json');
  await writeFile(oversizedPath, Buffer.alloc(1024 * 1024 + 1, 0x30));
  const directoryPath = join(dir, 'as-directory.json');
  await mkdir(directoryPath);
  for (const path of [nulPath, invalidPath, oversizedPath, directoryPath]) {
    const result = readRecord(path);
    assert.equal(result.value, null);
    assert.equal(result.error.code, 'record_unavailable');
    assert.ok(!parserText(result));
    assert.throws(() => readJson(path), (error) => error.code === 'record_unavailable' && !/Unexpected token|JSON\.parse/.test(error.message));
  }
  assert.deepEqual(readRecord(join(dir, 'missing.json')), { value: null, error: null });
});

test('a symlinked record is a typed degradation, not an outage', async (t) => {
  const { dir } = await fixture(t);
  const target = join(dir, 'target.json');
  await writeFile(target, JSON.stringify({ ok: true }));
  const link = join(dir, 'link.json');
  try { await symlink(target, link, 'file'); }
  catch (error) {
    if (!['EPERM', 'EACCES', 'UNKNOWN', 'ENOSYS'].includes(error.code)) throw error;
    // Windows without developer mode can still create a directory junction.
    try { await symlink(dir, link, 'junction'); }
    catch { t.skip(`symlink unavailable: ${error.code}`); return; }
  }
  const result = readRecord(link);
  assert.equal(result.value, null);
  assert.equal(result.error.code, 'record_unavailable');
});

test('CLI reads print degraded JSON without parser or NUL text', async (t) => {
  const { root, project } = await fixture(t);
  const run = await managedRun(root, project, { stateKind: 'nul' });
  await serviceSession(root, project, { stateKind: 'nul' });
  const env = { ...process.env, EDDA_PI_CHANNEL_DIR: root };
  for (const args of [['runs'], ['list'], ['doctor'], ['run-status', run.runId], ['run-conversation', run.runId]]) {
    const outcome = await exec(process.execPath, [cli, ...args], { env, timeout: 20000 }).catch((error) => error);
    const output = `${outcome.stdout || ''}${outcome.stderr || ''}`;
    assert.doesNotMatch(output, /Unexpected token|JSON\.parse|is not valid JSON/);
    assert.ok(!output.includes('\u0000'));
    assert.doesNotMatch(output, /SECRET/);
    assert.doesNotThrow(() => JSON.parse(outcome.stdout), `stdout for ${args[0]}`);
  }
});
