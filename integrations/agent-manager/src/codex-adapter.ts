import { createHash, randomUUID } from 'node:crypto';
import { openSync, closeSync, readSync, fstatSync, realpathSync } from 'node:fs';
import { isAbsolute, normalize } from 'node:path';
import { ManagerError, type AgentBinding, type AgentObservation, type PiAdapter, type PublicEntry, type RuntimeState } from './contracts.js';
import type { NativeSessionEvent } from './session-contracts.js';
import type { ManagerStore } from './store.js';

const HEADER = 256 * 1024, BUDGET = 1024 * 1024, LINE = 256 * 1024;
const digest = (value: string | Buffer): string => createHash('sha256').update(value).digest('hex');
const pathKey = (value: string): string => process.platform === 'win32' ? normalize(value).toLowerCase() : normalize(value);
interface Snapshot {
  version: 2; identity: string; incarnation: string; offset: number; checkpoint: string;
  skipping: boolean; historyComplete: boolean; state: RuntimeState; progress: string | null;
  diagnostic: string | null; entries: PublicEntry[]; events: NativeSessionEvent[];
}
function bytes(fd: number, start: number, count: number): Buffer {
  const buffer = Buffer.alloc(Math.max(0, count));
  return buffer.subarray(0, readSync(fd, buffer, 0, buffer.length, start));
}
function timestamp(value: unknown): string | null {
  return typeof value === 'string' && value.length <= 64 && Number.isFinite(Date.parse(value)) ? value : null;
}
function object(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
}
function identifier(value: unknown): string | null {
  return typeof value === 'string' && /^[a-zA-Z0-9_-]{1,160}$/.test(value) ? value : null;
}

/** Observes an explicitly selected rollout. This adapter never controls a process. */
export class CodexAdapter implements PiAdapter {
  constructor(private readonly store: ManagerStore) {}

  private read(binding: AgentBinding): Snapshot {
    const file = binding.transcriptFile;
    if (!file || !isAbsolute(file) || !isAbsolute(binding.workspace)) throw new ManagerError('INVALID_SOURCE', 'Codex 來源必須是明確的絕對路徑。');
    let fd: number | undefined;
    try {
      if (pathKey(realpathSync(file)) !== pathKey(file)) throw new ManagerError('INVALID_SOURCE', 'Codex 來源不可經由連結。');
      fd = openSync(file, 'r');
      const stat = fstatSync(fd);
      if (!stat.isFile()) throw new ManagerError('INVALID_SOURCE', 'Codex 來源必須是一般檔案。');
      const header = bytes(fd, 0, Math.min(HEADER, stat.size));
      const end = header.indexOf(10);
      if (end < 0) throw new ManagerError('INVALID_SOURCE', 'Codex 來源缺少完整身分紀錄。');
      const meta = object(JSON.parse(header.subarray(0, end).toString('utf8')));
      const payload = object(meta?.payload);
      if (meta?.type !== 'session_meta' || payload?.id !== binding.sessionId ||
          (payload.session_id !== undefined && payload.session_id !== binding.sessionId) ||
          typeof payload.cwd !== 'string' || pathKey(payload.cwd) !== pathKey(binding.workspace)) {
        throw new ManagerError('IDENTITY_MISMATCH', 'Codex 來源身分或工作目錄不符，未讀取對話。', 409);
      }
      const identity = digest(`${stat.dev}:${stat.ino}:${stat.birthtimeMs}:${header.subarray(0, end).toString('utf8')}`);
      const key = `codex-observer:${digest(JSON.stringify([pathKey(file), binding.sessionId, pathKey(binding.workspace)]))}`;
      const saved = this.store.setting(key);
      let state: Snapshot | null = null;
      if (saved) { try { state = JSON.parse(saved) as Snapshot; } catch { /* Rebuild bounded projection. */ } }
      const reset = !state || state.version !== 2 || state.identity !== identity || state.offset > stat.size ||
        state.checkpoint !== digest(bytes(fd, Math.max(0, state.offset - 64), Math.min(64, state.offset)));
      const allowance = BUDGET - header.length - 128;
      if (reset) {
        const offset = Math.max(end + 1, stat.size - allowance);
        state = { version: 2, identity, incarnation: randomUUID(), offset, checkpoint: '', skipping: offset > end + 1,
          historyComplete: offset === end + 1, state: 'unknown', progress: null, diagnostic: saved ? '來源已更換或截斷；重新讀取有限歷史。' : null,
          entries: [], events: [] };
      }
      const current = state!;
      const chunk = bytes(fd, current.offset, Math.min(allowance, stat.size - current.offset));
      let position = 0;
      while (position < chunk.length) {
        const newline = chunk.indexOf(10, position);
        if (newline < 0) {
          if (current.skipping || chunk.length - position > LINE) {
            current.skipping = true; current.historyComplete = false;
            current.diagnostic = '略過過大的紀錄；部分歷史不可用。'; position = chunk.length;
          }
          break; // A partial line stays on disk, never persisted as raw private text.
        }
        if (current.skipping) { current.skipping = false; position = newline + 1; continue; }
        if (newline - position > LINE) { current.historyComplete = false; current.diagnostic = '略過過大的紀錄；部分歷史不可用。'; }
        else {
          try { this.consume(current, JSON.parse(chunk.subarray(position, newline).toString('utf8')), current.offset + position, binding.sessionId); }
          catch { current.historyComplete = false; current.diagnostic = '略過損壞的紀錄；部分歷史不可用。'; }
        }
        position = newline + 1;
      }
      current.offset += position;
      current.checkpoint = digest(bytes(fd, Math.max(0, current.offset - 64), Math.min(64, current.offset)));
      this.store.putSetting(key, JSON.stringify(current));
      return current;
    } catch (error) {
      if (error instanceof ManagerError) throw error;
      throw new ManagerError('SOURCE_UNAVAILABLE', 'Codex 紀錄目前無法讀取；無法據此判定代理停止。', 503);
    } finally { if (fd !== undefined) closeSync(fd); }
  }

  private consume(state: Snapshot, raw: unknown, offset: number, sessionId: string): void {
    const record = object(raw), payload = object(record?.payload);
    if (!record || !payload) return;
    const at = timestamp(record.timestamp);
    const id = `codex:${digest(`${state.identity}:${state.incarnation}:${offset}`)}`;
    if (record.type === 'event_msg' && at) {
      const type = payload.type;
      const item = object(payload.item);
      const kind = type === 'task_started' ? 'started' : type === 'task_complete' ? 'reply_ended' :
        type === 'turn_aborted' ? 'interrupted' : type === 'item_completed' && item?.type === 'SubAgentActivity' && identifier(item.agent_thread_id) ? 'child_reference' : null;
      if (kind) {
        // A source reset changes conversation cursors, not the identity of an
        // already observed native event. Replayed lifecycle evidence deduplicates.
        const nativeId = `codex:${digest(JSON.stringify([sessionId, kind, identifier(payload.turn_id), at, identifier(payload.id), kind === 'child_reference' ? identifier(item?.id) : null, kind === 'child_reference' ? identifier(item?.agent_thread_id) : null]))}`;
        state.events.push({ id: nativeId, kind, at, turnId: identifier(payload.turn_id), category: null, httpStatus: null,
          ...(kind === 'child_reference' ? { childSessionId: identifier(item?.agent_thread_id)! } : {}) });
        state.events = state.events.slice(-128);
        state.progress = at;
        if (kind === 'started') state.state = 'running';
        if (kind === 'reply_ended') state.state = 'idle';
        if (kind === 'interrupted') state.state = 'unknown';
      }
    }
    // Only public message channels; never copy tool results, reasoning, or metadata instructions.
    if (record.type !== 'response_item' || payload.type !== 'message' ||
        (payload.role !== 'user' && payload.role !== 'assistant') ||
        (payload.role === 'assistant' && payload.channel !== 'final' && payload.channel !== 'commentary') || !Array.isArray(payload.content)) return;
    const parts = payload.content.map(object).filter((part) => part && (part.type === 'input_text' || part.type === 'output_text') && typeof part.text === 'string');
    const text = parts.map((part) => part!.text as string).join('\n');
    if (!text) return;
    state.entries.push({ id, timestamp: at, kind: 'message', role: payload.role, text: text.slice(0, 8192), truncated: text.length > 8192, toolName: null, toolError: false });
    state.entries = state.entries.slice(-64);
    if (at) state.progress = at;
  }

  async observe(binding: AgentBinding): Promise<AgentObservation> {
    const state = this.read(binding);
    return { state: state.state, instanceId: state.incarnation, observedAt: new Date().toISOString(), heartbeatAt: null,
      lastProgressAt: state.progress, lastEvent: state.events.at(-1)?.kind ?? null, source: 'recorded', stale: true,
      reason: state.diagnostic ?? '僅為已記錄的回合狀態；沒有程序存活證據。', model: null, usage: null,
      capabilities: { conversation: true, send: false }, latestMessage: state.entries.at(-1) ?? null, degraded: null,
      ownerMailbox: null,
      sessionEvidence: { sessionId: binding.sessionId, evidenceSource: 'recorded', historyComplete: state.historyComplete, events: state.events } };
  }
  async conversation(binding: AgentBinding, after?: string) {
    const state = this.read(binding);
    const index = after ? state.entries.findIndex((entry) => entry.id === after) : -1;
    if (after && index < 0) throw new ManagerError('STALE_CURSOR', '對話游標已過期，請重新讀取。', 409);
    const cursor = state.entries.at(-1)?.id ?? null;
    return { instanceId: state.incarnation, entries: state.entries.slice(index + 1), cursor, headCursor: cursor,
      hasMore: false, observedAt: new Date().toISOString(), source: 'recorded' as const };
  }
  async send(): Promise<never> { throw new ManagerError('UNSUPPORTED', '此 Codex 來源僅支援讀取，未傳送訊息。', 405); }
  async receipt(): Promise<null> { return null; }
}
