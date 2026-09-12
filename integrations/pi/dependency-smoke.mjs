// Real Pi + real Edda, isolated store, offline provider. Waits for the actual
// 60-second observer tick, not a manually forced check after the source change.
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { listSessions, getReceipt, requestSession } from './client.mjs';
import { followDependencies, dependencyStatus, unfollowDependencies } from './dependency-client.mjs';
import { adoptSession } from './adoption.mjs';

const [piEntry, eddaBin, mode] = process.argv.slice(2);
if (mode && mode !== '--adopt') throw new Error('Unknown smoke mode');
if (!piEntry || !eddaBin) throw new Error('Supply Pi JavaScript CLI entry and native Edda executable');
const root = await mkdtemp(join(tmpdir(), 'edda-dependency-smoke-'));
const project = join(root, 'project'), registry = join(root, 'channel');
await mkdir(project);
await mkdir(join(root, 'agent'));
const cleanEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('EDDA_')));
const env = { ...cleanEnv, EDDA_BIN: resolve(eddaBin), EDDA_STORE_ROOT: join(root, 'store'), EDDA_SESSION_ID: randomUUID(),
  EDDA_PI_CHANNEL_DIR: registry, PI_CODING_AGENT_DIR: join(root, 'agent'),
  EDDA_PI_SMOKE_REJECT: '0', EDDA_PI_SMOKE_HANDOFF: '0', EDDA_PI_SMOKE_LARGE: '0' };
const exec = promisify(execFile);
const edda = (...args) => exec(resolve(eddaBin), args, { cwd: project, env, windowsHide: true, timeout: 15000 });
let child, session;
try {
  await edda('init', '--no-hooks');
  await edda('task', 'new', 'Synthetic dependency review', '--key', 'dependency-smoke');
  const tasks = JSON.parse((await edda('task', 'list', '--json')).stdout);
  assert.equal(tasks.length, 1);
  const id = String(tasks[0].task_id);
  let managedId;
  if (mode === '--adopt') {
    await writeFile(join(project, 'brief.json'), JSON.stringify({ role: 'controller', doneWhen: ['Acknowledge synthetic notifications'],
      scope: { allowed: ['Offline fixture only'], excluded: ['User tasks'], reserved: ['Paid model calls'],
        authorityRefs: [{ uri: 'fixture://smoke', revision: 'v1' }] } }));
    await edda('task', 'new', 'Synthetic managed controller', '--after', id, '--brief', 'brief.json');
    managedId = String(JSON.parse((await edda('task', 'list', '--json')).stdout).find((task) => String(task.task_id) !== id).task_id);
    // This standalone smoke process must read from the same isolated Edda store.
    for (const key of Object.keys(process.env)) if (key.startsWith('EDDA_')) delete process.env[key];
    for (const [key, value] of Object.entries(env)) if (key.startsWith('EDDA_')) process.env[key] = value;
  }
  child = spawn(process.execPath, [resolve(piEntry), '--mode', 'rpc', '--no-session', '--no-extensions',
    '-e', fileURLToPath(new URL('./extension.mjs', import.meta.url)),
    '-e', fileURLToPath(new URL('./fixtures/offline-provider.mjs', import.meta.url)),
    '--no-skills', '--no-prompt-templates', '--no-themes', '--no-tools', '--provider', 'edda-offline-test', '--model', 'echo'],
  { cwd: project, env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  let output = '', stderr = '';
  child.stdout.on('data', (chunk) => { output = (output + chunk).slice(-200000); });
  child.stderr.on('data', (chunk) => { stderr = (stderr + chunk).slice(-4000); });
  child.stdin.write(JSON.stringify({ id: 'initial', type: 'get_state' }) + '\n');
  async function until(fn, label, timeoutMs = 20000) {
    const untilTime = Date.now() + timeoutMs;
    while (Date.now() < untilTime) {
      if (child.exitCode !== null) throw new Error(`Pi exited: ${stderr}`);
      const value = await fn();
      if (value) return value;
      await delay(250);
    }
    throw new Error(`Timed out: ${label}; ${stderr}`);
  }
  session = await until(async () => (await listSessions(registry)).find((row) => row.live), 'Pi registration');
  const scope = 'Only observe the isolated synthetic task and acknowledge notifications with the offline fixture. No real work or spending.';
  if (mode === '--adopt') {
    const adopted = await adoptSession(registry, session.sessionId.slice(0, 8), { id: managedId, scope, notify: true, maxNotifications: 3 });
    assert.equal(adopted.status, 'adopted');
    assert.ok(adopted.dependencies.taskIds.includes(id));
    assert.equal(adopted.workStarted, null);
  } else await followDependencies(registry, session.sessionId, { project, taskIds: [id], notify: true, maxNotifications: 3, scope });
  await until(async () => {
    const state = await dependencyStatus(registry, session.sessionId);
    return state.lastAlert && (await getReceipt(registry, session.sessionId, state.lastAlert.id)).status === 'settled';
  }, 'initial notification');
  const initial = await dependencyStatus(registry, session.sessionId);
  assert.equal(initial.notifications, 1);
  await edda('task', 'start', id);
  await edda('task', 'done', id, '--receipt', 'Changes Requested: synthetic test evidence, not accepted');
  console.log('Source changed; waiting for the real 60-second observer tick.');
  await until(async () => {
    const state = await dependencyStatus(registry, session.sessionId);
    return state.notifications === 2 && state.lastAlert.id !== initial.lastAlert.id &&
      (await getReceipt(registry, session.sessionId, state.lastAlert.id)).status === 'settled';
  }, 'automatic dependency notification', 80000);
  const final = await dependencyStatus(registry, session.sessionId);
  assert.equal(final.facts.find((fact) => fact.taskId === id).status, 'done');
  assert.ok(final.facts.find((fact) => fact.taskId === id).receipt.includes('Changes Requested'));
  assert.ok(output.includes('OFFLINE_ACK:'));
  assert.equal((await requestSession(registry, session.sessionId, '/status')).state, 'idle');
  await unfollowDependencies(registry, session.sessionId);
  assert.equal((await dependencyStatus(registry, session.sessionId)).phase, 'paused');
  console.log(JSON.stringify({ passed: true, actualPi: true, actualEdda: true, periodicTrigger: true,
    sameSession: true, adoption: mode === '--adopt', notifications: final.notifications, reviewRejectionPreserved: true, paused: true, paidCalls: 0 }, null, 2));
} finally {
  if (session && child?.exitCode === null) await unfollowDependencies(registry, session.sessionId);
  if (child && child.exitCode === null && child.signalCode === null) {
    const exited = once(child, 'exit'); child.kill(); await exited;
  }
  await rm(root, { recursive: true, force: true });
}
