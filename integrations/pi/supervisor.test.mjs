import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, copyFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { setTimeout as delay } from 'node:timers/promises';
import { normalizeSupervisor, supervisorDir } from './supervisor-store.mjs';
import { createSupervisorEngine } from './supervisor-engine.mjs';
import { dispatchable, taskRunId, validateProposal, taskFacts } from './supervisor-policy.mjs';
import { readTask } from './compose-sources.mjs';
import { writeJson, digest } from './store.mjs';
import { managedStatus, stopManaged } from './managed-client.mjs';
import { supervisorStatus, stopSupervisor } from './supervisor-client.mjs';

async function fixture(t, authority = 'Both fixture tasks are already approved; continue within these directories only.') {
  const root = await mkdtemp(join(tmpdir(), 'edda-supervisor-test-'));
  const project = join(root, 'project'), registry = join(root, 'registry'), pkg = join(root, 'pi');
  const a = join(project, 'a'), b = join(project, 'b');
  await mkdir(a, { recursive: true }); await mkdir(b); await mkdir(join(pkg, 'dist/bundle'), { recursive: true });
  await writeFile(join(pkg, 'package.json'), JSON.stringify({ type: 'module', name: '@earendil-works/pi-coding-agent', version: 'fixture' }));
  const piEntry = join(pkg, 'dist/bundle/cli.js'); await copyFile(new URL('./fixtures/managed-pi.mjs', import.meta.url), piEntry);
  const task = (id, patch = {}) => ({ task_id: id, title: id === 1 ? 'ASK fixture A' : 'fixture B', status: 'ready', assignee: `worker-${id}`,
    created_event_id: `evt-${id}`, after: [], attempts: 0, scope_paths: [], receipt: null, ...patch });
  const put = (id, patch) => writeFile(join(project, `task-${id}.json`), JSON.stringify(task(id, patch)));
  await put(1); await put(2, { status: 'blocked', after: [1] });
  const config = normalizeSupervisor({ version: 1, id: randomUUID(), project,
    authority: { instruction: authority, source: { uri: 'fixture://operator', revision: 'v1' } },
    tasks: [{ id: '1', cwd: a }, { id: '2', cwd: b }], worker: { piEntry, provider: 'fixture', model: 'echo' }, pollMs: 1000 });
  const dir = supervisorDir(registry, config.id, true);
  writeJson(join(dir, 'config.json'), config); writeJson(join(dir, 'identity.json'), { digest: digest(JSON.stringify(config)) });
  const taskReader = (p, id) => readTask(p, id, { file: process.execPath, args: [fileURLToPath(new URL('./fixtures/edda-task-graph-reader.mjs', import.meta.url))] });
  let active = true;
  const make = () => createSupervisorEngine(registry, config.id, () => active, { taskReader });
  let engine = make();
  t.after(async () => {
    const state = engine.snapshot();
    for (const id of [state.managerRunId, ...Object.values(state.workers).map((w) => w.runId)].filter(Boolean)) {
      try { if ((await managedStatus(registry, id)).live) await stopManaged(registry, id, { abort: true }); } catch { /* not launched */ }
    }
    await rm(root, { recursive: true, force: true });
  });
  return { root, project, registry, config, put, taskReader, get engine() { return engine; },
    restart() { engine = make(); return engine; }, pause() { active = false; }, resume() { active = true; } };
}
async function until(fn) {
  const end = Date.now() + 30000;
  while (Date.now() < end) { if (await fn()) return; await delay(100); }
  throw new Error('Supervisor fixture timeout');
}

test('policy limits actions/identity and dispatches only ready assigned unowned work', () => {
  const task = { task_id: 1, created_event_id: 'created', status: 'ready', assignee: 'a' };
  assert.equal(dispatchable(task), true);
  assert.equal(dispatchable({ ...task, status: 'running' }), false);
  assert.equal(dispatchable(task, { dispatched: true }), false);
  assert.equal(dispatchable({ ...task, assignee: null }), false);
  assert.equal(taskRunId('/project', task), taskRunId('/project', task));
  assert.notEqual(taskRunId('/other', task), taskRunId('/project', task));
  assert.throws(() => validateProposal({ packetId: 'p', taskId: '1', action: 'merge', reason: 'x' }, { id: 'p', task: { id: '1' } }), /Invalid/);
});

test('receipt changes beyond the bounded excerpt still change task evidence identity', () => {
  const base = { task_id: 1, title: 'Review', status: 'done', assignee: 'reviewer', created_event_id: 'evt', after: [], scope_paths: [] };
  const prefix = 'x'.repeat(1500);
  const first = taskFacts({ ...base, receipt: prefix + 'Changes Requested' });
  const second = taskFacts({ ...base, receipt: prefix + 'LGTM' });
  assert.equal(first.receipt.text, second.receipt.text);
  assert.notEqual(first.receiptRevision, second.receiptRevision);
});

test('automatic dispatch, manager continuation and dependent dispatch survive engine restart without duplicate work', async (t) => {
  const f = await fixture(t);
  await f.engine.tick();
  const firstRun = f.engine.snapshot().workers['1'].runId;
  f.restart();
  await until(async () => { await f.engine.tick(); return f.engine.snapshot().responses === 1; });
  assert.equal(f.engine.snapshot().workers['1'].runId, firstRun);
  await until(async () => (await f.taskReader(f.project, '1')).task.status === 'done');
  await f.put(2, { status: 'ready', after: [1] });
  await until(async () => { await f.engine.tick(); return f.engine.snapshot().phase === 'tasks_done'; });
  const before = f.engine.snapshot();
  await f.engine.tick(); await f.engine.tick();
  assert.equal(f.engine.snapshot().decisions, before.decisions);
  assert.equal(f.engine.snapshot().responses, 1);
  assert.equal(await readFile(join(f.project, 'a/result.txt'), 'utf8'), 'DONE\n');
  assert.equal(await readFile(join(f.project, 'b/result.txt'), 'utf8'), 'DONE\n');
});

test('pause prevents dispatch and invalid manager proposals remain visible without worker effects', async (t) => {
  const f = await fixture(t, 'TEST_INVALID_PROPOSAL. Existing fixture scope only.');
  f.pause(); await f.engine.tick(); assert.equal(Object.keys(f.engine.snapshot().workers).length, 0);
  f.resume();
  await until(async () => { await f.engine.tick(); return Object.values(f.engine.snapshot().cases).some((c) => c.status === 'invalid_proposal'); });
  const count = f.engine.snapshot().decisions;
  await f.engine.tick();
  assert.equal(f.engine.snapshot().phase, 'needs_attention');
  assert.equal(f.engine.snapshot().responses, 0);
  assert.equal(f.engine.snapshot().decisions, count);
  assert.equal((await f.taskReader(f.project, '1')).task.status, 'ready');
});

test('running task without explicit managed binding is not duplicated', async (t) => {
  const f = await fixture(t);
  await f.put(1, { status: 'running' });
  await f.engine.tick();
  assert.equal(f.engine.snapshot().workers['1'], undefined);
  assert.equal(f.engine.snapshot().decisions, 0);
});

test('different worktrees of the same repository share one task execution identity', async (t) => {
  const f = await fixture(t), exec = promisify(execFile);
  const git = (...args) => exec('git', ['-C', f.project, ...args], { windowsHide: true });
  await git('init'); await writeFile(join(f.project, 'seed.txt'), 'fixture'); await git('add', 'seed.txt');
  await git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'fixture');
  const peer = join(f.root, 'peer'); await git('worktree', 'add', '--detach', peer, 'HEAD');
  const task = (await f.taskReader(f.project, '1')).task;
  assert.equal(taskRunId(f.project, task), taskRunId(peer, task));
});

test('real supervisor service starts, reconnects and pauses despite unavailable task source', async (t) => {
  const f = await fixture(t), exec = promisify(execFile), configFile = join(f.root, 'config.json');
  await writeFile(configFile, JSON.stringify(f.config));
  const cli = fileURLToPath(new URL('./cli.mjs', import.meta.url));
  const launch = () => exec(process.execPath, [cli, 'supervisor-start', '--config', configFile], {
    windowsHide: true, timeout: 30000, env: { ...process.env, EDDA_BIN: join(f.root, 'missing-edda'), EDDA_PI_CHANNEL_DIR: f.registry } });
  try {
    const first = JSON.parse((await launch()).stdout);
    assert.equal(first.live, true);
    const duplicate = JSON.parse((await launch()).stdout);
    assert.equal(duplicate.serviceId, first.serviceId);
    await until(async () => (await supervisorStatus(f.registry, f.config.id)).phase === 'needs_attention');
    assert.equal((await supervisorStatus(f.registry, f.config.id)).decisions, 0);
    assert.equal((await stopSupervisor(f.registry, f.config.id)).phase, 'paused');
    const restarted = JSON.parse((await launch()).stdout);
    assert.equal(restarted.live, true);
    assert.notEqual(restarted.serviceId, first.serviceId);
  } finally { await stopSupervisor(f.registry, f.config.id); }
});
