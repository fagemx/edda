import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, mkdir, rm, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { startSupervisor, supervisorStatus, stopSupervisor } from './supervisor-client.mjs';
import { managedStatus, stopManaged } from './managed-client.mjs';
import { requestSession } from './client.mjs';

const [piEntry, eddaBin, mode, provider, model] = process.argv.slice(2);
if (!piEntry || !eddaBin || (mode && mode !== '--live-manager') || (mode && (!provider || !model))) throw new Error('Supply Pi entry, Edda binary, optionally --live-manager PROVIDER MODEL');
const root = await mkdtemp(join(tmpdir(), 'edda-supervisor-smoke-'));
const project = join(root, 'project'), registry = join(root, 'registry'), agentDir = join(root, 'agent');
await mkdir(join(project, 'a'), { recursive: true }); await mkdir(join(project, 'b')); await mkdir(agentDir);
for (const key of Object.keys(process.env)) if (key.startsWith('EDDA_')) delete process.env[key];
Object.assign(process.env, { EDDA_BIN: resolve(eddaBin), EDDA_STORE_ROOT: join(root, 'edda-store'), EDDA_SESSION_ID: randomUUID() });
const exec = promisify(execFile);
const edda = (...args) => exec(resolve(eddaBin), args, { cwd: project, windowsHide: true, timeout: 15000 });
let id, passed = false;
async function until(fn, label, timeout = 90000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { const value = await fn(); if (value) return value; await delay(200); }
  throw new Error(`Supervisor smoke timed out: ${label}`);
}
try {
  await edda('init', '--no-hooks');
  await edda('task', 'new', 'ASK supervisor fixture A', '--assignee', 'fixture-a', '--brief', 'Write the assigned temporary result and record task completion. Existing operator approval covers this fixture.');
  const a = String(JSON.parse((await edda('task', 'list', '--json')).stdout)[0].task_id);
  await edda('task', 'new', 'Supervisor fixture B', '--assignee', 'fixture-b', '--after', a, '--brief', 'After A, complete only the second temporary fixture and record its task receipt.');
  const b = String(JSON.parse((await edda('task', 'list', '--json')).stdout).find((t) => String(t.task_id) !== a).task_id);
  const fixture = fileURLToPath(new URL('./fixtures/supervisor-provider.mjs', import.meta.url));
  const profile = { piEntry: resolve(piEntry), provider: 'edda-supervisor-fixture', model: 'echo', agentDir, extensions: [fixture] };
  id = randomUUID();
  const config = { version: 1, id, project, authority: { instruction: 'Both selected temporary fixture tasks are already approved. Continue the assigned file operation and task receipts without asking the operator again. No other work, tools outside the fixture, spending, or new scope is authorized.',
    source: { uri: 'fixture://operator-approved', revision: 'v1' } },
    tasks: [{ id: a, cwd: join(project, 'a') }, { id: b, cwd: join(project, 'b') }], worker: profile,
    manager: mode ? { piEntry: resolve(piEntry), provider, model, thinking: 'low' } : profile,
    maxDecisions: 3, maxResponses: 3, pollMs: 1000 };
  await startSupervisor(registry, config);
  const first = await until(async () => {
    const s = await supervisorStatus(registry, id);
    return s.workers?.[a]?.observed?.piState === 'idle' ? s : null;
  }, 'first worker question');
  const runA = first.workers[a].runId;
  await stopSupervisor(registry, id);
  const before = await requestSession(registry, first.workers[a].sessionId, '/conversation?limit=50');
  await startSupervisor(registry, config);
  const complete = await until(async () => {
    const s = await supervisorStatus(registry, id);
    if (s.phase === 'failed' || s.recentCases?.some((c) => ['invalid_proposal', 'missing_proposal', 'escalate', 'response_limit'].includes(c.status) || c.managerReceipt === 'failed')) throw new Error(`Supervisor decision failed: ${s.attention}`);
    return s.phase === 'tasks_done' ? s : null;
  }, 'automatic continuation and dependent dispatch', mode ? 180000 : 90000);
  assert.equal(complete.workers[a].runId, runA);
  assert.equal(complete.responses, 1);
  assert.equal(complete.decisions, 1);
  assert.equal(await readFile(join(project, 'a/result.txt'), 'utf8'), 'FIXTURE_DONE\n');
  assert.equal(await readFile(join(project, 'b/result.txt'), 'utf8'), 'FIXTURE_DONE\n');
  const after = await requestSession(registry, first.workers[a].sessionId, '/conversation?limit=50');
  assert.equal(after.entries.filter((e) => e.role === 'user' && e.text?.includes('[EDDA_SUPERVISOR_TASK]')).length, 1);
  assert.ok(after.entries.length >= before.entries.length);
  await delay(2500);
  assert.equal((await supervisorStatus(registry, id)).decisions, complete.decisions);
  const manager = await managedStatus(registry, complete.managerRunId);
  passed = true;
  console.log(JSON.stringify({ passed: true, actualPi: true, actualEdda: true, workers: 2, automaticResponses: complete.responses,
    managerDecisions: complete.decisions, serviceRestart: true, noDuplicateInitialTask: true, idleModelCalls: 0,
    liveManager: Boolean(mode), managerModel: manager.model, reportedUsage: manager.usage || null, acceptance: 'unverified_task_receipts_only' }, null, 2));
} finally {
  if (id) {
    const state = await supervisorStatus(registry, id);
    if (!passed) { await writeFile(join(root, 'failure-evidence.json'), JSON.stringify(state, null, 2)); console.log(`Failure evidence: ${root}`); }
    await stopSupervisor(registry, id);
    for (const runId of [state.managerRunId, ...Object.values(state.workers || {}).map((w) => w.runId)].filter(Boolean)) {
      try { if ((await managedStatus(registry, runId)).live) await stopManaged(registry, runId, { abort: true }); } catch { /* not launched */ }
    }
  }
  if (passed) await rm(root, { recursive: true, force: true });
}
