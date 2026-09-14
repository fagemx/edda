import { spawn } from 'node:child_process';
import { mkdirSync, openSync, closeSync, unlinkSync, realpathSync, lstatSync, readSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { managedDir, installRuntime, verifyRelease, findPiEntry, alive, inside } from './managed-store.mjs';
import { readJson, readRecord, writeJson, validateId, digest, sessionDir, recover } from './store.mjs';
import { requestSession, getReceipt } from './client.mjs';
import { readTranscript } from './conversation.mjs';

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
    const { value: state, error: stateError } = readRecord(join(dir, 'state.json'));
    // A corrupt state.json is a per-run degradation: config.json still holds the
    // run identity. Whitelist fields; never spread config (it holds the prompt).
    if (stateError) return { runId: id, project: config.project, release: config.release,
      provider: config.provider ?? null, model: config.model ?? null, thinking: config.thinking ?? null,
      owner: config.owner ?? null, returnOwner: config.returnOwner ?? null,
      status: 'record_unavailable', state: 'record_unavailable', live: false, lastRecordedPhase: null,
      initialReceipt: { status: 'unknown', live: false }, error: stateError,
      nextAction: 'The run state record is unreadable and was not repaired. Use the identity shown; inspect the owned session directory before any resume.' };
    // Runner snapshots refresh the receipt without writing it back to state.json.
    // After stop/crash, the channel receipt is authoritative, not the launch copy.
    let initialReceipt = state?.initialReceipt || null;
    if (state?.sessionId && initialReceipt?.id) {
      try { initialReceipt = await getReceipt(root, state.sessionId, initialReceipt.id); }
      catch { initialReceipt = { id: initialReceipt.id, status: 'unknown', lastRecordedStatus: initialReceipt.status, live: false }; }
    }
    return { ...(state || {}), initialReceipt, runId: id, live: false, status: 'runner_unreachable', lastRecordedPhase: state?.phase,
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
  extensions = [], agentDir, noTools = false, noSkills = false, tools, thinking, owner, returnOwner } = {}) {
  runId = validateId(runId); root = resolve(root);
  if (!project) throw new Error('launch requires --project');
  project = realpathSync(resolve(project));
  if (!lstatSync(project).isDirectory()) throw new Error('Project must be a directory');
  if (prompt !== undefined && (typeof prompt !== 'string' || !prompt.trim() || Buffer.byteLength(prompt) > 16384)) throw new Error('Initial prompt must contain 1..16384 UTF-8 bytes');
  for (const value of [provider, model]) if (value !== undefined && (typeof value !== 'string' || !value.trim() || value.length > 300)) throw new Error('Invalid provider/model');
  for (const value of [owner, returnOwner]) if (value !== undefined && (typeof value !== 'string' || !/^[\p{L}\p{N}_.@ /:-]{1,200}$/u.test(value))) throw new Error('Invalid owner/return-owner label');
  if (thinking !== undefined && !['off', 'minimal', 'low', 'medium', 'high', 'xhigh'].includes(thinking)) throw new Error('Invalid thinking level');
  if (tools !== undefined && (noTools || !Array.isArray(tools) || !tools.length || tools.length > 16 || tools.some((name) => typeof name !== 'string' || !/^[a-z][a-z0-9_]{0,79}$/.test(name)))) throw new Error('Select explicit tool names or noTools, not both');
  if (!Array.isArray(extensions) || extensions.length > 8) throw new Error('Select at most eight trusted extra extensions');
  extensions = extensions.map((path) => {
    const file = realpathSync(resolve(path));
    if (!lstatSync(file).isFile()) throw new Error('Extra extension must be a file');
    return file;
  });
  if (agentDir) agentDir = realpathSync(resolve(agentDir));
  const pi = findPiEntry(piEntry);
  const inputs = { project, pi, provider: provider || null, model: model || null, prompt: prompt ?? null,
    extensions, agentDir: agentDir || null, noTools: Boolean(noTools), thinking: thinking || null,
    owner: owner || null, returnOwner: returnOwner || null, ...(noSkills ? { noSkills: true } : {}), ...(tools ? { tools } : {}) };
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

function readSessionHeader(file) {
  const headerFd = openSync(file, 'r'), buffer = Buffer.alloc(16384);
  let size;
  try { size = readSync(headerFd, buffer, 0, buffer.length, 0); } finally { closeSync(headerFd); }
  const newline = buffer.subarray(0, size).indexOf(10);
  if (newline < 0) throw new Error('Session header exceeds bounded read');
  try { return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(buffer.subarray(0, newline))); }
  catch { throw new Error('Session header is unreadable; it was not repaired'); }
}

function ownedSessionFile(dir, config, state) {
  if (!state?.sessionId || !state.sessionFile || !inside(join(dir, 'sessions'), state.sessionFile)) throw new Error('No owned persisted session to resume or read');
  if (lstatSync(join(dir, 'sessions')).isSymbolicLink()) throw new Error('Managed session directory must not be a link');
  const file = realpathSync(state.sessionFile);
  if (!inside(realpathSync(join(dir, 'sessions')), file) || !lstatSync(file).isFile()) throw new Error('Session file escaped managed storage');
  const header = readSessionHeader(file);
  if (header.type !== 'session' || header.id !== state.sessionId || realpathSync(header.cwd) !== config.project) throw new Error('Session file identity/project mismatch');
  return file;
}

// Without state.json, recover runId -> sessionId by enumerating the run's owned
// sessions directory: exactly one transcript whose header matches the project.
function recoverOwnedSession(dir, config) {
  const sessionsDir = join(dir, 'sessions');
  let info; try { info = lstatSync(sessionsDir); } catch { info = null; }
  if (!info || !info.isDirectory() || info.isSymbolicLink()) throw new Error('Managed session directory must not be a link');
  const owned = realpathSync(sessionsDir), candidates = [];
  for (const name of readdirSync(sessionsDir).filter((entry) => entry.endsWith('.jsonl')).sort()) {
    const file = join(sessionsDir, name);
    let stat; try { stat = lstatSync(file); } catch { continue; }
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024 * 1024 * 1024) continue;
    let real; try { real = realpathSync(file); } catch { continue; }
    if (!inside(owned, real)) continue;
    let header; try { header = readSessionHeader(real); } catch { continue; }
    if (header?.type !== 'session' || typeof header.id !== 'string' || !header.id) continue;
    try { if (realpathSync(header.cwd) === config.project) candidates.push({ file: real, sessionId: header.id }); }
    catch { /* header cwd is not resolvable; not this project's transcript */ }
  }
  if (!candidates.length) throw new Error('No owned persisted session to read');
  if (candidates.length > 1) throw new Error('Multiple owned persisted sessions; pass an explicit session identity');
  return candidates[0];
}

export async function managedConversation(root, id, options = {}) {
  id = validateId(id);
  const { dir, config } = runConfig(root, id);
  const { value: state, error: stateError } = readRecord(join(dir, 'state.json'));
  if (!stateError) {
    if (state?.runId !== id) throw new Error('Managed state identity mismatch');
    const file = ownedSessionFile(dir, config, state);
    return { runId: id, sessionId: state.sessionId, evidenceSource: 'persisted_session',
      conversation: await readTranscript(file, options),
      notice: 'Read-only persisted public history, not live branch or process proof. No session was resumed.' };
  }
  const recovered = recoverOwnedSession(dir, config);
  return { runId: id, sessionId: recovered.sessionId, evidenceSource: 'persisted_session',
    error: stateError, recoveredWithoutState: true,
    conversation: await readTranscript(recovered.file, options),
    notice: 'Read-only persisted public history recovered from the owned session directory because the run state record is unreadable. Not live branch or process proof; no session was resumed.' };
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
    ownedSessionFile(dir, config, state);
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
