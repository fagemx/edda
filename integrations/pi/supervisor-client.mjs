import { spawn } from 'node:child_process';
import { openSync, closeSync, unlinkSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { readJson, writeJson, digest, validateId } from './store.mjs';
import { installRuntime, verifyRelease, alive } from './managed-store.mjs';
import { supervisorDir, normalizeSupervisor, loadSupervisor, pauseToken } from './supervisor-store.mjs';

async function request(root, id) {
  const { dir } = loadSupervisor(root, id), owner = readJson(join(dir, 'owner.json'));
  if (!owner || owner.id !== id || !/^[0-9a-f]{64}$/.test(owner.token) || !Number.isInteger(owner.port) || owner.port < 1 || owner.port > 65535) throw new Error('Supervisor endpoint unavailable');
  validateId(owner.serviceId);
  const response = await fetch(`http://127.0.0.1:${owner.port}/status`, { redirect: 'error', signal: AbortSignal.timeout(2500),
    headers: { authorization: `Bearer ${owner.token}`, 'x-edda-instance': owner.serviceId } });
  const result = await response.json();
  if (!response.ok || result.supervisorId !== id || result.serviceId !== owner.serviceId) throw new Error('Supervisor endpoint identity mismatch');
  return result;
}
export async function supervisorStatus(root, id) {
  id = validateId(id);
  const { dir } = loadSupervisor(root, id);
  try { return await request(root, id); }
  catch {
    const life = readJson(join(dir, 'lifecycle.json'));
    const state = readJson(join(dir, 'state.json'));
    return { supervisorId: id, live: false, phase: life?.phase === 'completed' ? state?.phase : life?.phase || 'unavailable', pid: life?.pid,
      lastTaskPhase: state?.phase, error: life?.error || state?.attention || null,
      decisions: state?.decisions || 0, responses: state?.responses || 0, workers: state?.workers || {},
      managerRunId: state?.managerRunId || null, pending: state?.pending || null,
      acceptance: 'unverified', operatorNotified: false };
  }
}
export async function stopSupervisor(root, id) {
  id = validateId(id);
  const { dir } = loadSupervisor(root, id);
  writeJson(join(dir, 'pause.json'), { id, nonce: randomUUID(), pausedAt: new Date().toISOString() });
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    const result = await supervisorStatus(root, id);
    if (!result.live && !alive(result.pid)) return { ...result, phase: 'paused', workersInterrupted: false };
    await delay(100);
  }
  return { supervisorId: id, phase: 'pause_requested', workersInterrupted: false, nextAction: 'Inspect supervisor-status; an in-flight read/launch may still be settling.' };
}
export async function startSupervisor(root, value) {
  root = resolve(root);
  const config = normalizeSupervisor(value), id = config.id, dir = supervisorDir(root, id, true);
  const configPath = join(dir, 'config.json'), identity = digest(JSON.stringify(config));
  if (existsSync(configPath)) {
    if (digest(JSON.stringify(loadSupervisor(root, id).config)) !== identity) throw new Error('Supervisor ID already has a different configuration');
    const current = await supervisorStatus(root, id);
    if (current.live || current.phase === 'tasks_done') return current;
  }
  const lock = join(dir, 'launch.lock'), fd = openSync(lock, 'wx', 0o600);
  try {
    if (!existsSync(configPath)) {
      writeJson(configPath, config, true); writeJson(join(dir, 'identity.json'), { digest: identity }, true);
    }
    const life = readJson(join(dir, 'lifecycle.json')), owner = readJson(join(dir, 'owner.json'));
    if (alive(life?.pid) || alive(owner?.pid)) throw new Error('Prior supervisor may still be alive; reconnect instead of duplicating it');
    const runningLock = join(dir, 'service.lock');
    if (existsSync(runningLock)) {
      const prior = readJson(runningLock);
      if (!prior || prior.serviceId !== life?.serviceId || prior.pid !== life?.pid || alive(prior.pid)) throw new Error('Ambiguous supervisor lock; inspect before recovering');
      unlinkSync(runningLock);
    }
    const releaseFile = join(dir, 'runtime.json');
    const release = readJson(releaseFile) || installRuntime(root);
    if (!existsSync(releaseFile)) writeJson(releaseFile, release, true);
    verifyRelease(release);
    const serviceId = randomUUID();
    writeJson(join(dir, 'lifecycle.json'), { id, serviceId, phase: 'launch_requested', expectedPauseToken: pauseToken(dir) });
    const log = openSync(join(dir, 'service.log'), 'a', 0o600);
    try {
      const child = spawn(process.execPath, [join(release.path, 'supervisor-runner.mjs'), root, id, serviceId],
        { cwd: config.project, detached: true, windowsHide: true, stdio: ['ignore', log, log] });
      child.on('error', (error) => writeJson(join(dir, 'lifecycle.json'), { id, serviceId, phase: 'failed', error: error.message }));
      child.unref();
    } finally { closeSync(log); }
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline) {
      const current = await supervisorStatus(root, id);
      if (current.live || ['failed', 'tasks_done'].includes(current.phase)) return current;
      await delay(100);
    }
    return { supervisorId: id, phase: 'launch_pending', live: false, nextAction: 'Query supervisor-status using the same ID.' };
  } finally { closeSync(fd); unlinkSync(lock); }
}
