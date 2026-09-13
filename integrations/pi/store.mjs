import { mkdirSync, lstatSync, chmodSync, readFileSync, writeFileSync, renameSync,
  unlinkSync, readdirSync, existsSync, openSync, closeSync, fsyncSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
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

// A record that exists but cannot be used (bad JSON, wrong shape, oversized, a
// symlink) degrades per record. readJson keeps strict semantics for callers that
// must distinguish "absent" from "unusable", but the thrown error is typed and
// its message never embeds the record bytes (JSON.parse messages do).
export class RecordUnavailableError extends Error {
  constructor(record = 'record') {
    super(`Record unavailable: ${record}; it was not repaired`);
    this.name = 'RecordUnavailableError';
    this.code = 'record_unavailable';
    this.record = record;
  }
}

export function readJson(path) {
  let stat;
  try { stat = lstatSync(path); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024 * 1024) throw new RecordUnavailableError(basename(path));
  let text;
  try { text = readFileSync(path, 'utf8'); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
  try { return JSON.parse(text); }
  catch { throw new RecordUnavailableError(basename(path)); }
}

// Tolerant per-record access: ENOENT yields `{ value: null, error: null }`; an
// unusable record yields `{ value: null, error: { code, record } }` with no record
// bytes. Genuine failures (permissions, I/O) still throw so they are not hidden.
export function readRecord(path) {
  try { return { value: readJson(path), error: null }; }
  catch (error) {
    if (error.code === 'record_unavailable') {
      return { value: null, error: { code: error.code, record: error.record, message: error.message } };
    }
    throw error;
  }
}

// Every record is written to a fresh tmp file and renamed into place, so a
// reader never sees a half-written document. The rename is only durable once the
// tmp's data is on disk: on NTFS a hard interruption between the tmp write and
// the rename can leave the target with its recorded length but zero-filled data,
// which reads back as an all-NUL document (GH-715 saw the same shape; the
// 2026-09-13 managed registry left four run and eight service `state.json` files
// all-NUL with normal lengths). Flushing the tmp before the rename makes a crash
// leave either the old complete record or the new complete record — never NUL.
const fileIo = {
  open: (path) => openSync(path, 'wx', 0o600),
  write: (fd, text) => writeFileSync(fd, text),
  flush: (fd) => fsyncSync(fd),
  close: (fd) => closeSync(fd),
  rename: (from, to) => renameSync(from, to),
  unlink: (path) => unlinkSync(path),
  // A directory flush is what makes the rename itself durable on POSIX; Windows
  // cannot open a directory for this, and flushes the file data instead.
  flushDir: (dir) => {
    if (process.platform === 'win32') return;
    let fd;
    try { fd = openSync(dir, 'r'); fsyncSync(fd); }
    catch { /* best-effort: the file data is already flushed */ }
    finally { if (fd !== undefined) { try { closeSync(fd); } catch { /* closed */ } } }
  },
};
export function writeJson(path, data, exclusive = false, io = fileIo) {
  const text = JSON.stringify(data, null, 2) + '\n';
  if (exclusive) {
    const fd = io.open(path);
    try { io.write(fd, text); io.flush(fd); } finally { io.close(fd); }
    return;
  }
  const tmp = `${path}.${randomUUID()}.tmp`;
  try {
    const fd = io.open(tmp);
    try { io.write(fd, text); io.flush(fd); } finally { io.close(fd); }
    io.rename(tmp, path);
    io.flushDir(dirname(resolve(path)));
  } finally {
    try { io.unlink(tmp); } catch (error) { if (error.code !== 'ENOENT') throw error; }
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
    // Read each record independently: a corrupt state.json must not hide a
    // readable owner.json, and neither may abort the registry-wide read.
    const state = readRecord(join(dir, 'state.json')), owner = readRecord(join(dir, 'owner.json'));
    return { dir, state: state.value, owner: owner.value, stateError: state.error, ownerError: owner.error };
  }).filter((r) => r.state || r.owner || r.stateError || r.ownerError);
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
