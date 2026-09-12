import { spawn } from 'node:child_process';
import { mkdirSync, openSync, closeSync, unlinkSync, realpathSync, lstatSync, readSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { managedDir, installRuntime, verifyRelease, findPiEntry, alive, inside } from './managed-store.mjs';
import { readJson, writeJson, validateId, digest, sessionDir, recover } from './store.mjs';
import { requestSession } from './client.mjs';

function runConfig(root, id) {
  const dir = managedDir(root, id), config = readJson(join(dir, 'config.json'));
  if (!config || config.runId !== id || config.root !== resolve(root)) throw new Error('Managed run configuration not found or identity mismatch');
  return { dir, config };
}
async function runnerRequest(root, id, operation, abort = false) {
  const { dir } = runConfig(root, id), owner = readJson(join(dir, 'owner.json'));
  if (!owner || owner.runId !== id || !/^[0-9a-f]{64}$/.test(owner.token) || !Number.isInteger(owner.port) || owner.port < 1 || owner.port > 65535) throw new Error('Managed runner unavailable');
  validateId(owner.serviceId);
  const response = await fetch(`http://127.0.0.1:${owner.port}/${operation}`, { method: operation === 'stop' ? 'POST' : 'GET',
    headers: { authorization: `Bearer ${owner.token}`, 'x-edda-instance': owner.serviceId, ...(abort ? { 'x-edda-abort': 'true' } : {}) }, redirect: 'error', signal: AbortSignal.timeout(operation === 'stop' ? 10000 : 3000) });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || 'Managed request failed');
  if (result.runId !== id || result.serviceId !== owner.serviceId) throw new Error('Managed runner response identity mismatch');
  return result;
}
export async function managedStatus(root, id) {
  id = validateId(id);
  const { dir, config } = runConfig(root, id);
  try { return await runnerRequest(root, id, 'status'); }
  catch {
    const state = readJson(join(dir, 'state.json'));
    return { ...(state || {}), runId: id, live: false, status: 'runner_unreachable', lastRecordedPhase: state?.phase,
      release: config.release, nextAction: state?.sessionFile ? 'Inspect and explicitly run-resume; initial work is not replayed.' : 'Inspect the launch evidence; do not create a duplicate on a timeout.' };
  }
}
export async function stopManaged(root, id, { abort = false } = {}) {
  id = validateId(id);
  const { dir } = runConfig(root, id), owner = readJson(join(dir, 'owner.json'));
  const result = await runnerRequest(root, id, 'stop', abort);
  const deadline = Date.now() + 5000;
  while (alive(owner?.pid) && Date.now() < deadline) await delay(50);
  return alive(owner?.pid) ? { ...result, status: 'stop_pending', nextAction: 'Query run-status before resume; the runner is still exiting.' } : result;
}

function startRunner(root, dir, config, resume) {
  const serviceId = randomUUID();
  const previous = readJson(join(dir, 'state.json'));
  writeJson(join(dir, 'state.json'), { ...previous, runId: config.runId, serviceId, phase: 'launch_requested', updatedAt: new Date().toISOString() });
  const fd = openSync(join(dir, 'runner.log'), 'a', 0o600);
  let child;
  try {
    child = spawn(process.execPath, [join(config.release.path, 'managed-runner.mjs'), resolve(root), config.runId, serviceId, ...(resume ? ['--resume'] : [])],
      { cwd: config.project, detached: true, windowsHide: true, stdio: ['ignore', fd, fd] });
    child.on('error', (error) => { writeJson(join(dir, 'state.json'), { ...previous, runId: config.runId, serviceId, phase: 'failed', error: error.message }); });
    child.unref();
  } finally { closeSync(fd); }
  return serviceId;
}
async function awaitLaunch(root, id, serviceId) {
  const deadline = Date.now() + 30000;
  let result;
  while (Date.now() < deadline) {
    result = await managedStatus(root, id);
    if (result.serviceId === serviceId && (result.live || ['failed', 'exited', 'stopped'].includes(result.lastRecordedPhase))) return result;
    await delay(150);
  }
  return { ...result, status: 'launch_pending', nextAction: 'Query run-status with this run ID; do not launch a new ID merely because startup is slow.' };
}

export async function launchManaged(root, { runId = randomUUID(), project, piEntry, provider, model, prompt,
  extensions = [], agentDir, noTools = false, thinking } = {}) {
  runId = validateId(runId); root = resolve(root);
  if (!project) throw new Error('launch requires --project');
  project = realpathSync(resolve(project));
  if (!lstatSync(project).isDirectory()) throw new Error('Project must be a directory');
  if (prompt !== undefined && (typeof prompt !== 'string' || !prompt.trim() || Buffer.byteLength(prompt) > 16384)) throw new Error('Initial prompt must contain 1..16384 UTF-8 bytes');
  for (const value of [provider, model]) if (value !== undefined && (typeof value !== 'string' || !value.trim() || value.length > 300)) throw new Error('Invalid provider/model');
  if (thinking !== undefined && !['off', 'minimal', 'low', 'medium', 'high', 'xhigh'].includes(thinking)) throw new Error('Invalid thinking level');
  if (!Array.isArray(extensions) || extensions.length > 8) throw new Error('Select at most eight trusted extra extensions');
  extensions = extensions.map((path) => {
    const file = realpathSync(resolve(path));
    if (!lstatSync(file).isFile()) throw new Error('Extra extension must be a file');
    return file;
  });
  if (agentDir) agentDir = realpathSync(resolve(agentDir));
  const pi = findPiEntry(piEntry);
  const inputs = { project, pi, provider: provider || null, model: model || null, prompt: prompt ?? null,
    extensions, agentDir: agentDir || null, noTools: Boolean(noTools), thinking: thinking || null };
  const dir = managedDir(root, runId, true), previous = readJson(join(dir, 'config.json'));
  if (previous) {
    if (previous.inputsDigest !== digest(JSON.stringify(inputs))) throw new Error('Run ID already has different launch inputs');
    return managedStatus(root, runId);
  }
  const release = installRuntime(root);
  const config = { version: 1, runId, root, ...inputs, inputsDigest: digest(JSON.stringify(inputs)), release };
  writeJson(join(dir, 'config.json'), config, true);
  mkdirSync(join(dir, 'sessions'), { mode: 0o700 });
  const serviceId = startRunner(root, dir, config, false);
  return awaitLaunch(root, runId, serviceId);
}

export async function resumeManaged(root, id) {
  id = validateId(id);
  const { dir, config } = runConfig(root, id);
  const lock = join(dir, 'resume.lock');
  const fd = openSync(lock, 'wx', 0o600);
  try {
    const existing = await managedStatus(root, id);
    if (existing.live) return existing;
    const state = readJson(join(dir, 'state.json'));
    const owner = readJson(join(dir, 'owner.json'));
    if (alive(owner?.pid) || alive(state?.runnerPid)) throw new Error('Previous runner may still be alive; no duplicate launch');
    if (!state?.sessionId || !state.sessionFile || !inside(join(dir, 'sessions'), state.sessionFile)) throw new Error('No owned persisted session to resume');
    if (lstatSync(join(dir, 'sessions')).isSymbolicLink()) throw new Error('Managed session directory must not be a link');
    const file = realpathSync(state.sessionFile);
    if (!inside(realpathSync(join(dir, 'sessions')), file) || !lstatSync(file).isFile()) throw new Error('Session file escaped managed storage');
    const headerFd = openSync(file, 'r'), buffer = Buffer.alloc(16384);
    let size;
    try { size = readSync(headerFd, buffer, 0, buffer.length, 0); } finally { closeSync(headerFd); }
    const newline = buffer.subarray(0, size).indexOf(10);
    if (newline < 0) throw new Error('Session header exceeds bounded read');
    const header = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(buffer.subarray(0, newline)));
    if (header.type !== 'session' || header.id !== state.sessionId || realpathSync(header.cwd) !== config.project) throw new Error('Session file identity/project mismatch');
    try {
      const live = await requestSession(root, state.sessionId, '/status');
      if (live.live) throw Object.assign(new Error('Pi is still running; reconnect instead of launching another process'), { live: true });
    } catch (error) { if (error.live) throw error; }
    const piOwner = readJson(join(sessionDir(root, state.sessionId), 'owner.json'));
    if (alive(state.childPid) || alive(piOwner?.pid)) throw new Error('Previous Pi may still be alive; recovery refused');
    if (piOwner) recover(root, state.sessionId, piOwner.instanceId);
    verifyRelease(config.release);
    const serviceId = startRunner(root, dir, config, true);
    return await awaitLaunch(root, id, serviceId);
  } finally { closeSync(fd); unlinkSync(lock); }
}
