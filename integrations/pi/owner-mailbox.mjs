import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

// A thin wrapper over the existing Rust `edda return` owner mailbox. It never
// writes mailbox files itself and never throws into the session: a session must
// keep working when the native CLI is absent, unknown or superseded.
const exec = promisify(execFile);
const maxBytes = 1024 * 1024;

export function validateOwner(owner) {
  if (typeof owner !== 'string' || owner.length < 1 || owner.length > 200 || /[\u0000-\u001f\u007f]/.test(owner)) {
    throw new Error('Owner reference must be 1..200 characters without control characters');
  }
  return owner;
}

function normalizeCommand(command) {
  if (!command || typeof command.file !== 'string' || !command.file) throw new Error('Owner mailbox command must name an executable');
  const args = command.args ?? [];
  if (!Array.isArray(args) || args.some((value) => typeof value !== 'string')) throw new Error('Owner mailbox command args must be strings');
  return { file: command.file, args: [...args] };
}

function parseJson(text) {
  if (typeof text !== 'string') return null;
  try {
    const value = JSON.parse(text);
    return value && typeof value === 'object' && !Array.isArray(value) ? value : null;
  } catch { return null; }
}

function clip(text, limit) {
  const bytes = Buffer.from(text);
  if (bytes.length <= limit) return text;
  let end = limit;
  while (end > 0 && (bytes[end] & 0xc0) === 0x80) end--;
  return bytes.subarray(0, end).toString('utf8');
}

function optionalText(value, limit) {
  if (value === null || value === undefined) return null;
  if (typeof value !== 'string') return undefined;
  return clip(value, limit);
}

// Normalise one CLI return record into the bounded presentation fields. A
// record that does not carry the required fields is rejected so a malformed
// claim is never presented as partial content.
function safeReturn(record) {
  if (!record || typeof record !== 'object' || Array.isArray(record)) return null;
  if (typeof record.work !== 'string' || typeof record.status !== 'string') return null;
  const result = optionalText(record.result, 600);
  if (result === undefined) return null;
  const deliverable = optionalText(record.deliverable, 500);
  if (deliverable === undefined) return null;
  const postedAt = optionalText(record.posted_at, 40);
  if (postedAt === undefined) return null;
  return {
    id: typeof record.id === 'string' ? record.id : null,
    work: clip(record.work, 200),
    status: clip(record.status, 40),
    result,
    deliverable,
    posted_at: postedAt,
  };
}

function cliError(error) {
  return [error?.stderr, error?.stdout, error?.message].filter((part) => typeof part === 'string').join('\n');
}

export function createOwnerMailbox({ owner, sessionId, cwd, command, timeoutMs = 15000 }) {
  validateOwner(owner);
  if (typeof sessionId !== 'string' || !sessionId) throw new Error('Owner mailbox requires a session id');
  if (typeof cwd !== 'string' || !cwd) throw new Error('Owner mailbox requires a working directory');
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) throw new Error('Owner mailbox timeout must be positive');
  const cli = normalizeCommand(command ?? { file: process.env.EDDA_BIN || 'edda', args: [] });
  let state = { owner, status: 'unbound', replaced: false };
  let closed = false;

  const invoke = (args) => exec(cli.file, [...cli.args, ...args], {
    cwd, windowsHide: true, timeout: timeoutMs, maxBuffer: maxBytes, encoding: 'utf8',
  });
  const unavailable = () => {
    state = { owner, status: 'unavailable', replaced: false };
    return { status: 'unavailable', replaced: false };
  };

  async function bind() {
    if (closed) return { status: 'unavailable', replaced: false };
    try {
      const first = parseJson((await invoke(['return', 'bind', '--owner', owner, '--session', sessionId, '--json'])).stdout);
      if (!first || first.owner !== owner) return unavailable();
      const replacedSession = typeof first.replacedSession === 'string' ? first.replacedSession : null;
      const replaced = Boolean(replacedSession && replacedSession !== sessionId);
      state = { owner, status: replaced ? 'replaced' : 'bound', replaced };
      return { status: state.status, holder: 'current', replaced };
    } catch (error) {
      if (!/is held by session/.test(cliError(error))) return unavailable();
      try {
        const status = parseJson((await invoke(['return', 'status', '--owner', owner, '--json'])).stdout);
        const holder = status && typeof status.holder === 'string' ? status.holder : null;
        if (!holder) { state = { owner, status: 'unbound', replaced: false }; return { status: 'unbound', replaced: false }; }
        if (holder === sessionId) { state = { owner, status: 'bound', replaced: false }; return { status: 'bound', holder: 'current', replaced: false }; }
        // Explicit replacement taken from the persisted owner record, not from
        // arbitrary caller input.
        const rebind = parseJson((await invoke(['return', 'bind', '--owner', owner, '--session', sessionId,
          '--replaces-session', holder, '--json'])).stdout);
        if (!rebind || rebind.owner !== owner) return unavailable();
        state = { owner, status: 'replaced', replaced: true };
        return { status: 'replaced', holder: 'current', replaced: true };
      } catch { return unavailable(); }
    }
  }

  async function claim() {
    if (closed) return { status: 'unavailable', returns: [] };
    try {
      const parsed = parseJson((await invoke(['return', 'claim', '--owner', owner, '--session', sessionId, '--json'])).stdout);
      if (!parsed || parsed.owner !== owner || !Array.isArray(parsed.returns) ||
        !Number.isInteger(parsed.count) || parsed.count !== parsed.returns.length) {
        return { status: 'unavailable', returns: [] };
      }
      const returns = parsed.returns.map(safeReturn);
      if (returns.some((record) => record === null)) return { status: 'unavailable', returns: [] };
      if (returns.length > 0) return { status: 'ok', returns };
      return { status: 'empty', returns: [] };
    } catch (error) {
      const text = cliError(error);
      if (/no owner binding/.test(text)) {
        state = { ...state, status: 'unbound' };
        return { status: 'unbound', returns: [] };
      }
      if (/superseded session cannot claim|is held by session/.test(text)) {
        state = { ...state, status: 'replaced' };
        return { status: 'superseded', returns: [] };
      }
      return { status: 'unavailable', returns: [] };
    }
  }

  return {
    owner,
    bind,
    claim,
    state: () => ({ ...state }),
    close: () => { closed = true; },
  };
}
