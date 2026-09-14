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

// Applying the channel ACL spawns Windows PowerShell, and a loaded CI runner can
// make a cold start exceed the spawn timeout, which surfaces as
// `spawnSync powershell.exe ETIMEDOUT` and fails whichever unrelated test first
// calls `privateRoot()`. The ACL step is idempotent (`private-directory.ps1`
// re-sets the same protected ACL and re-verifies it), so a timeout is retried
// within a bounded window. The private-root guarantee is unchanged: a success
// still means the script ran and verified, a non-timeout error still fails on the
// first attempt, and an exhausted window is a typed failure — never a skipped
// check.
//
// The per-attempt spawn timeout shrinks on retry, so the whole bounded sequence
// costs ~24 s of PowerShell time — not three full 15 s attempts — while still
// giving a loaded runner a fresh chance after a cold-start overrun.
const ACL_ATTEMPT_TIMEOUT_MS = [12000, 6000, 6000];
// The pause before retry `i + 1`.
const ACL_RETRY_MS = [0, 500, 1500];
const aclIo = {
  run: (script, root, timeout) => execFileSync('powershell.exe',
    ['-NoLogo', '-NoProfile', '-NonInteractive', '-File', script, '-Path', root],
    { windowsHide: true, timeout, stdio: 'pipe' }),
  // `sleepSync` is declared below with the record-write retry; the arrow defers
  // the lookup so it is initialized by the time `privateRoot` runs.
  sleep: (ms) => sleepSync(ms),
};

// An ACL that could not be applied before the retry window closed. Typed so the
// failure is visible and distinguishable from a genuine ACL refusal (which is
// thrown unchanged, on the first attempt).
export class PrivateRootError extends Error {
  constructor(root, attempts, cause) {
    super(`Channel root ACL failed after ${attempts} attempts: ${basename(root)} (${cause.code || 'unknown'})`);
    this.name = 'PrivateRootError';
    this.code = 'private_root_failed';
    this.root = root;
    this.errno = cause.code || null;
    this.cause = cause;
  }
}

// Exported so a bounded-retry test can drive it without a real PowerShell.
export function runPrivateDirectoryAcl(root, io = aclIo) {
  const script = fileURLToPath(new URL('./private-directory.ps1', import.meta.url));
  for (let attempt = 0; ; attempt += 1) {
    const timeout = ACL_ATTEMPT_TIMEOUT_MS[Math.min(attempt, ACL_ATTEMPT_TIMEOUT_MS.length - 1)];
    try { io.run(script, root, timeout); return; }
    catch (error) {
      if (error.code !== 'ETIMEDOUT') throw error;
      if (attempt >= ACL_ATTEMPT_TIMEOUT_MS.length - 1) throw new PrivateRootError(root, attempt + 1, error);
      io.sleep(ACL_RETRY_MS[attempt + 1]);
    }
  }
}

export function privateRoot(root, io = aclIo) {
  root = resolve(root);
  mkdirSync(root, { recursive: true, mode: 0o700 });
  const stat = lstatSync(root);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('Channel root must be a real directory');
  if (process.platform === 'win32') {
    runPrivateDirectoryAcl(root, io);
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
// reader never sees a half-written *renamed* document; the exclusive path creates
// the destination directly (`wx`) and relies on the same file flush. The write is
// only durable once the data is on disk: on NTFS a hard interruption can leave
// the target with its recorded length but zero-filled data, which reads back as
// an all-NUL document. The rename moves whatever is durable, so the NUL is
// observed once the rename of unflushed bytes has landed; a crash strictly before
// the rename leaves the previous complete record intact (GH-715 saw the same
// shape; the 2026-09-13 managed registry left four run and eight service
// `state.json` files all-NUL with normal lengths). Flushing the data before the
// rename makes a crash leave either the old complete record or the new complete
// record — never NUL.
//
// On Windows the replace itself can also fail transiently: a reader walking the
// registry (`edda-pi runs`; the agent-manager poll) or a scanner can hold the
// destination without delete-sharing, and `renameSync` then throws `EPERM`. That
// killed a live managed runner on 2026-09-14 from the `managed-runner` heartbeat.
// The replace is retried a bounded number of times on the Windows replace errnos,
// so a transient reader cannot end a run; a failure that outlives the window is a
// typed `RecordWriteError`, never a silent success and never a weakened barrier.
const TOLERATED_DIR_FSYNC = new Set(['EINVAL', 'ENOTSUP', 'EBADF']);
// `EINVAL`/`ENOTSUP` mean "directory fsync not supported here" and are tolerated
// because the file data is already durable. `EBADF` is tolerated deliberately and
// separately: the synchronous open -> fsync -> close in `flushDir` cannot produce
// it today, so this changes no current behaviour, but it keeps a spurious
// platform errno from becoming a reported write failure. Every other failure
// propagates.
const RETRYABLE_RENAME = new Set(['EPERM', 'EBUSY']);
// The pause before retry `i + 1`; cumulative wall time ~0.5 s — long enough to
// outlast a reader's pass over the record, short enough not to stall a writer.
const RENAME_RETRY_MS = [0, 15, 35, 75, 150, 200];
const sleepSync = (ms) => { if (ms > 0) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms); };

// A write that exhausted its bounded retries. Typed so a caller (the managed
// runner's heartbeat) can degrade visibly rather than treat it as success or die
// on an opaque `EPERM`.
export class RecordWriteError extends Error {
  constructor(path, attempts, cause) {
    super(`State record write failed after ${attempts} attempts: ${basename(path)} (${cause.code || 'unknown'})`);
    this.name = 'RecordWriteError';
    this.code = 'record_write_failed';
    this.record = basename(path);
    this.errno = cause.code || null;
    this.cause = cause;
  }
}

function renameBounded(io, from, to) {
  for (let attempt = 0; ; attempt += 1) {
    try { io.rename(from, to); return attempt + 1; }
    catch (error) {
      if (!RETRYABLE_RENAME.has(error.code)) throw error;
      if (attempt >= RENAME_RETRY_MS.length - 1) throw new RecordWriteError(to, attempt + 1, error);
      (io.sleep ?? sleepSync)(RENAME_RETRY_MS[attempt + 1]);
    }
  }
}
const fileIo = {
  open: (path) => openSync(path, 'wx', 0o600),
  write: (fd, text) => writeFileSync(fd, text),
  flush: (fd) => fsyncSync(fd),
  close: (fd) => closeSync(fd),
  rename: (from, to) => renameSync(from, to),
  unlink: (path) => unlinkSync(path),
  // The parent-directory flush is what makes the rename itself durable on POSIX.
  // Windows cannot open a directory for this, so there the file-data flush above
  // is the barrier. Some POSIX filesystems reject a directory fsync with a
  // "not supported here" errno; those are tolerated because the file data is
  // already durable. Any other failure propagates: the directory entry is not
  // known to be durable and a coordination writer must not report success.
  flushDir: (dir) => {
    if (process.platform === 'win32') return;
    let fd;
    try { fd = openSync(dir, 'r'); }
    catch (error) { if (TOLERATED_DIR_FSYNC.has(error.code)) return; throw error; }
    try { fsyncSync(fd); }
    catch (error) { if (!TOLERATED_DIR_FSYNC.has(error.code)) throw error; }
    finally { try { closeSync(fd); } catch { /* the fsync result already decided */ } }
  },
};
export function writeJson(path, data, exclusive = false, io = fileIo) {
  const text = JSON.stringify(data, null, 2) + '\n';
  if (exclusive) {
    const fd = io.open(path);
    try { io.write(fd, text); io.flush(fd); } finally { io.close(fd); }
    io.flushDir(dirname(resolve(path)));
    return;
  }
  const tmp = `${path}.${randomUUID()}.tmp`;
  try {
    const fd = io.open(tmp);
    try { io.write(fd, text); io.flush(fd); } finally { io.close(fd); }
    renameBounded(io, tmp, path);
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
