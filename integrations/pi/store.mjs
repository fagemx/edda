import { mkdirSync, lstatSync, chmodSync, readFileSync, writeFileSync, renameSync,
  unlinkSync, readdirSync, existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

export const defaultRoot = () => resolve(process.env.EDDA_PI_CHANNEL_DIR || join(homedir(), '.edda-pi-sessions'));
export function validateSession(id) {
  if (typeof id !== 'string' || !/^[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,199}$/.test(id)) throw new Error('Invalid session ID');
  return id;
}
export function validateId(id) {
  if (typeof id !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) {
    throw new Error('Message/instance ID must be a UUID');
  }
  return id.toLowerCase();
}
export const digest = (value) => createHash('sha256').update(value).digest('hex');
export const sessionDir = (root, id) => join(resolve(root), digest(validateSession(id)));

export function privateRoot(root) {
  root = resolve(root);
  mkdirSync(root, { recursive: true, mode: 0o700 });
  const stat = lstatSync(root);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('Channel root must be a real directory');
  if (process.platform === 'win32') {
    execFileSync('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-File',
      fileURLToPath(new URL('./private-directory.ps1', import.meta.url)), '-Path', root], { windowsHide: true, timeout: 15000, stdio: 'pipe' });
  } else {
    if (stat.uid !== process.getuid()) throw new Error('Channel root belongs to another user');
    chmodSync(root, 0o700);
  }
  return root;
}

export function readJson(path) {
  try {
    const stat = lstatSync(path);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024 * 1024) throw new Error('Invalid channel record');
    return JSON.parse(readFileSync(path, 'utf8'));
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

export function writeJson(path, data, exclusive = false) {
  const text = JSON.stringify(data, null, 2) + '\n';
  if (exclusive) return writeFileSync(path, text, { flag: 'wx', mode: 0o600 });
  const tmp = `${path}.${randomUUID()}.tmp`;
  try {
    writeFileSync(tmp, text, { flag: 'wx', mode: 0o600 });
    renameSync(tmp, path);
  } finally {
    try { unlinkSync(tmp); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
}

export function openStore(root, sessionId, owner) {
  privateRoot(root);
  const dir = sessionDir(root, sessionId);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  if (lstatSync(dir).isSymbolicLink()) throw new Error('Symlink in channel registry');
  return guarded(dir, () => createStore(dir, owner));
}

function guarded(dir, action) {
  const path = join(dir, 'lifecycle.lock');
  try { writeFileSync(path, '', { flag: 'wx', mode: 0o600 }); }
  catch (error) {
    if (error.code === 'EEXIST') throw new Error('Session lifecycle operation already in progress; do not retry automatically');
    throw error;
  }
  try { return action(); } finally { unlinkSync(path); }
}

function createStore(dir, owner) {
  const ownerPath = join(dir, 'owner.json');
  try { writeJson(ownerPath, owner, true); }
  catch (error) {
    if (error.code === 'EEXIST') throw new Error('Session already has a channel owner; inspect status or explicitly recover a dead owner');
    throw error;
  }
  mkdirSync(join(dir, 'receipts'), { recursive: true, mode: 0o700 });
  return {
    dir,
    owner: (value) => writeJson(ownerPath, value),
    state: (value) => writeJson(join(dir, 'state.json'), value),
    receipt: (id) => readJson(join(dir, 'receipts', `${validateId(id)}.json`)),
    receipts: () => readdirSync(join(dir, 'receipts')).filter((p) => p.endsWith('.json'))
      .map((p) => readJson(join(dir, 'receipts', p))),
    putReceipt: (value) => writeJson(join(dir, 'receipts', `${validateId(value.id)}.json`), value),
    release: () => guarded(dir, () => {
      if (readJson(ownerPath)?.instanceId === owner.instanceId) unlinkSync(ownerPath);
    }),
  };
}

export function registry(root) {
  if (!existsSync(root)) return [];
  return readdirSync(root).filter((name) => /^[0-9a-f]{64}$/.test(name)).map((name) => {
    const dir = join(root, name);
    if (lstatSync(dir).isSymbolicLink()) throw new Error('Symlink in channel registry');
    return { state: readJson(join(dir, 'state.json')), owner: readJson(join(dir, 'owner.json')) };
  }).filter((r) => r.state || r.owner);
}

export function recover(root, sessionId, instanceId) {
  const dir = sessionDir(root, sessionId);
  return guarded(dir, () => recoverLocked(dir, sessionId, instanceId));
}

function recoverLocked(dir, sessionId, instanceId) {
  const path = join(dir, 'owner.json');
  const owner = readJson(path);
  if (!owner || owner.instanceId !== validateId(instanceId)) throw new Error('Owner instance changed or absent');
  if (!Number.isSafeInteger(owner.pid) || owner.pid <= 0) throw new Error('Invalid owner PID');
  try { process.kill(owner.pid, 0); }
  catch (error) {
    if (error.code !== 'ESRCH') throw new Error('Cannot prove owner is dead');
    // No automatic replay: persisted accepted/queued/started receipts remain evidence.
    unlinkSync(path);
    return { sessionId, instanceId, recovered: true, receiptsPreserved: true };
  }
  throw new Error('Owner process is still alive; recovery refused');
}
