import { realpathSync } from 'node:fs';
import { isAbsolute, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { ManagerError, type AdapterReceipt, type AgentBinding, type AgentObservation, type ConversationView, type DiscoveredRun, type DiscoveryReport, type OperationStatus, type OperationView, type PiAdapter, type PublicEntry, type RuntimeState, type SendRequest } from './contracts.js';
import type { NativeSessionEvent } from './session-contracts.js';

// This is the only legacy-module boundary. All values crossing back are projected
// from unknown JSON into explicit DTO fields; no Pi owner/runner object is returned.
interface LegacyClient {
  requestSession(root: string, id: string, path: string, body?: unknown, timeout?: number, instance?: string): Promise<unknown>;
  getReceipt(root: string, id: string, operationId: string): Promise<unknown>;
  listSessions?(root: string): Promise<unknown>;
}
interface LegacyManaged { managedStatus(root: string, id: string): Promise<unknown> }
interface LegacyActivation { listManagedRuns?(root: string, options?: { limit?: number }): Promise<unknown> }
interface LegacyStore { privateRoot(root: string): string; defaultRoot?(): string }
export const defaultPiRoot = (): string => fileURLToPath(new URL('../../../pi/', import.meta.url));
async function optionalModule<T>(piRoot: string, file: string): Promise<T> {
  try { return await import(pathToFileURL(resolve(piRoot, file)).href) as T; } catch { return {} as T; }
}
export async function secureRoot(root: string, piRoot = defaultPiRoot()): Promise<string> {
  const legacy = await import(pathToFileURL(resolve(piRoot, 'store.mjs')).href) as LegacyStore;
  return legacy.privateRoot(root);
}
const record = (value: unknown): Record<string, unknown> => value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
const str = (value: unknown, max = 1000): string | null => typeof value === 'string' ? value.slice(0, max) : null;
const date = (value: unknown): string | null => typeof value === 'string' && Number.isFinite(Date.parse(value)) ? value : null;
const number = (value: unknown): number | null => typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
const states: RuntimeState[] = ['running', 'executing_tool', 'idle', 'waiting_user', 'stopped', 'unknown'];
const discoveryStates: RuntimeState[] = ['running', 'executing_tool', 'idle', 'waiting_user', 'stopped', 'unavailable', 'unknown'];
const MAX_DISCOVERY_ROOTS = 32, MAX_DISCOVERY_ROWS = 200, MAX_MANAGED_RUNS = 100;
const statuses: OperationStatus[] = ['prepared', 'unconfirmed', 'accepted', 'queued', 'started', 'settled', 'failed', 'unknown'];
function canonical(path: string): string { try { return realpathSync(path); } catch { return resolve(path); } }
function assertState(binding: AgentBinding, value: unknown, instance?: string): Record<string, unknown> {
  const state = record(value);
  if (state.live !== true || state.sessionId !== binding.sessionId || typeof state.instanceId !== 'string' ||
    typeof state.cwd !== 'string' || canonical(state.cwd) !== canonical(binding.workspace)) throw new ManagerError('UNAVAILABLE', '代理身分或工作目錄無法確認。', 409);
  if (instance && state.instanceId !== instance) throw new ManagerError('INSTANCE_CHANGED', '代理已換成新的執行實例，請更新畫面後再傳送。', 409);
  return state;
}
export function publicEntries(value: unknown): PublicEntry[] {
  if (!Array.isArray(value)) throw new ManagerError('INVALID_SOURCE', '代理對話格式不正確。', 502);
  return value.slice(0, 50).flatMap((raw): PublicEntry[] => {
    const e = record(raw), id = str(e.id, 200);
    if (!id) return [];
    if (e.kind === 'tool_result') return [{ id, timestamp: date(e.timestamp), kind: 'tool_result', role: null,
      text: '', truncated: false, toolName: str(e.toolName, 100), toolError: e.isError === true }];
    if (e.kind !== 'message' || (e.role !== 'assistant' && e.role !== 'user')) return [];
    return [{ id, timestamp: date(e.timestamp), kind: 'message', role: e.role, text: str(e.text, 16000) || '',
      truncated: e.truncated === true || (typeof e.text === 'string' && e.text.length > 16000), toolName: null, toolError: false }];
  });
}
function receipt(value: unknown, binding: AgentBinding, id: string, instance: string): AdapterReceipt {
  const r = record(value);
  if (r.id !== id || r.sessionId !== binding.sessionId || r.instanceId !== instance) throw new ManagerError('UNKNOWN_RECEIPT', '回執身分不一致，結果尚待確認。', 502);
  return { id, sessionId: binding.sessionId, instanceId: instance,
    status: statuses.includes(r.status as OperationStatus) ? r.status as OperationStatus : 'unknown' };
}
export const unavailable = (): AgentObservation => ({ state: 'unavailable', instanceId: null, observedAt: new Date().toISOString(),
  heartbeatAt: null, lastProgressAt: null, lastEvent: null, source: 'unavailable', stale: true, reason: '目前無法讀取代理；不代表工作已完成。',
  model: null, usage: null, capabilities: { conversation: false, send: false }, latestMessage: null });

export class ChannelAdapter implements PiAdapter {
  private cache = new Map<string, { instance: string; progress: string | null; latest: PublicEntry | null }>();
  private nativeEvents = new Map<string, Map<string, NativeSessionEvent>>();
  private constructor(private client: LegacyClient, private managed: LegacyManaged, private activation: LegacyActivation = {}, private defaultRootFn?: () => string) {}
  static async create(piRoot = defaultPiRoot()): Promise<ChannelAdapter> {
    const client = await import(pathToFileURL(resolve(piRoot, 'client.mjs')).href) as LegacyClient;
    const managed = await import(pathToFileURL(resolve(piRoot, 'managed-client.mjs')).href) as LegacyManaged;
    if (typeof client.requestSession !== 'function' || typeof client.getReceipt !== 'function' || typeof managed.managedStatus !== 'function') throw new Error('Unsupported Pi adapter library');
    const activation = await optionalModule<LegacyActivation>(piRoot, 'activation.mjs');
    const store = await optionalModule<LegacyStore>(piRoot, 'store.mjs');
    return new ChannelAdapter(client, managed, activation, typeof store.defaultRoot === 'function' ? store.defaultRoot : undefined);
  }
  // The normal/effective Pi registry (EDDA_PI_CHANNEL_DIR, else ~/.edda-pi-sessions)
  // is a bounded, documented root, not an arbitrary home scan. Null when the
  // loaded Pi module does not expose it.
  defaultRegistryRoot(): string | null {
    if (!this.defaultRootFn) return null;
    try { return this.defaultRootFn(); } catch { return null; }
  }
  validateMessage(request: SendRequest): void {
    const envelope = { id: request.operationId, message: request.message, sender: 'operator-console', mode: request.mode };
    if (Buffer.byteLength(JSON.stringify(envelope)) > 24576) throw new ManagerError('TOO_LARGE', '訊息包含較多跳脫字元，超過代理通道的容量；請縮短內容。', 413);
  }
  // Read-only bounded discovery over the roots the operator already configured.
  // It reuses the validated managed-run and session listings (owner identity is
  // authenticated by Pi), never reads registry files here, and never assigns,
  // sends or launches. A managed run and its live session merge into one row.
  async discover(registryRoots: string[]): Promise<DiscoveryReport> {
    const report: DiscoveryReport = { runs: [], failures: [] };
    const uniqueRoots = [...new Set(registryRoots)];
    const roots = uniqueRoots.slice(0, MAX_DISCOVERY_ROOTS);
    // A silently truncated root set would contradict the documented bounded root
    // set; report the drop as a source issue instead of hiding it.
    const dropped = uniqueRoots.slice(MAX_DISCOVERY_ROOTS);
    const firstDropped = dropped[0];
    if (firstDropped !== undefined) report.failures.push({ registryRoot: firstDropped,
      message: `已明確設定的來源超過 ${MAX_DISCOVERY_ROOTS} 個上限；其餘 ${dropped.length} 個未探索。` });
    const listSessions = this.client.listSessions, listManaged = this.activation.listManagedRuns;
    if (typeof listSessions !== 'function' && typeof listManaged !== 'function') {
      report.failures.push(...roots.map((registryRoot) => ({ registryRoot, message: '這個 Pi 版本沒有可用的探索介面；未進行探索。' })));
      return report;
    }
    await Promise.all(roots.map(async (root) => {
      const found: DiscoveredRun[] = [], problems: string[] = [];
      if (typeof listManaged === 'function') {
        try {
          const listing = record(await listManaged(root, { limit: MAX_MANAGED_RUNS })), runs = Array.isArray(listing.runs) ? listing.runs : [];
          for (const raw of runs.slice(0, MAX_MANAGED_RUNS)) {
            const value = record(raw), runId = str(value.runId, 64);
            if (!runId || !/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(runId)) continue;
            const sessionId = typeof value.sessionId === 'string' && /^[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,199}$/.test(value.sessionId) ? value.sessionId : null;
            const project = value.project, phase = str(value.recordedPhase, 40) ?? 'unknown', stopped = ['stopped', 'failed', 'exited'].includes(phase);
            found.push({ registryRoot: root, sessionId, runId, instanceId: null, live: false, source: 'recorded',
              state: stopped ? 'stopped' : 'unknown', workspace: typeof project === 'string' && isAbsolute(project) ? project : null,
              lastProgressAt: date(value.updatedAt), reason: str(value.error, 300) ?? (stopped ? '上次管理紀錄為已停止；目前沒有即時連線。' : '僅有管理紀錄，沒有即時連線；不代表工作已完成。') });
          }
        } catch { problems.push('此來源的管理執行清單目前無法讀取；其他來源不受影響。'); }
      }
      if (typeof listSessions === 'function') {
        try {
          const rows = await listSessions(root);
          if (!Array.isArray(rows)) problems.push('此來源的 session 清單格式不正確；未採用任何項目。');
          else for (const raw of rows.slice(0, MAX_DISCOVERY_ROWS)) {
            const value = record(raw), sessionId = str(value.sessionId, 200);
            const live = value.live === true, cwd = value.cwd;
            if (!sessionId || !/^[a-zA-Z0-9][a-zA-Z0-9_.:-]*$/.test(sessionId)) continue;
            const reason = live ? null : value.state === 'stopped' ? '上次紀錄為已停止；目前沒有即時連線。' : '目前沒有可連線的註冊擁有者；不代表工作已完成。';
            found.push({ registryRoot: root, sessionId, runId: null, instanceId: typeof value.instanceId === 'string' ? value.instanceId : null,
              state: discoveryStates.includes(value.state as RuntimeState) ? value.state as RuntimeState : value.state === 'unreachable' ? 'unavailable' : 'unknown',
              live, source: live ? 'live' : 'recorded', workspace: typeof cwd === 'string' && isAbsolute(cwd) ? cwd : null,
              lastProgressAt: date(value.lastProgressAt), reason });
          }
        } catch { problems.push('此來源的 session 清單目前無法讀取；其他來源不受影響。'); }
      }
      report.runs.push(...found);
      if (problems.length) report.failures.push({ registryRoot: root, message: problems.join(' ') });
    }));
    return report;
  }
  async observe(binding: AgentBinding): Promise<AgentObservation> {
    let managed: Record<string, unknown> = {};
    const eventKey = `${binding.registryRoot}\0${binding.sessionId}`;
    let events = this.nativeEvents.get(eventKey);
    if (!events) { events = new Map(); this.nativeEvents.set(eventKey, events); }
    const remember = (event: NativeSessionEvent) => { events!.set(event.id, event); while (events!.size > 128) events!.delete(events!.keys().next().value!); };
    try {
      if (binding.runId) {
        try {
          const value = record(await this.managed.managedStatus(binding.registryRoot, binding.runId));
          if (value.runId === binding.runId && (!value.sessionId || value.sessionId === binding.sessionId)) managed = value;
        } catch { /* direct selected session still may be reachable */ }
      }
      const state = assertState(binding, await this.client.requestSession(binding.registryRoot, binding.sessionId, '/status'));
      const instance = String(state.instanceId), progress = date(state.lastProgressAt);
      const capabilities = Array.isArray(state.capabilities) ? state.capabilities : [];
      let latest = this.cache.get(binding.id)?.instance === instance ? this.cache.get(binding.id)?.latest || null : null;
      if (capabilities.includes('conversation') && (this.cache.get(binding.id)?.instance !== instance || this.cache.get(binding.id)?.progress !== progress)) {
        try {
          const page = record(await this.client.requestSession(binding.registryRoot, binding.sessionId, '/conversation?limit=12', undefined, 2500, instance));
          for (const raw of Array.isArray(page.entries) ? page.entries : []) {
            const entry = record(raw), at = date(entry.timestamp), id = str(entry.id, 200);
            if (entry.role !== 'assistant' || !id || !at || !['stop', 'length', 'error', 'aborted'].includes(String(entry.stopReason))) continue;
            const error = record(managed.modelError), status = Number(error.httpStatus);
            remember({ id: `${binding.sessionId}:${id}`, at, turnId: null, kind: entry.stopReason === 'error' ? 'provider_error' : entry.stopReason === 'aborted' ? 'interrupted' : 'reply_ended',
              category: entry.stopReason === 'error' ? status === 402 ? 'credit_or_quota' : ['credit_or_quota', 'authentication', 'timeout'].includes(String(error.category)) ? String(error.category) : 'provider_error' : null,
              httpStatus: entry.stopReason === 'error' && Number.isInteger(status) && status >= 400 && status <= 599 ? status : null });
          }
          const recent = publicEntries(page.entries).filter((e) => e.role === 'assistant' && e.text.trim()).at(-1);
          if (recent) latest = { ...recent, text: recent.text.slice(0, 700), truncated: recent.truncated || recent.text.length > 700 };
          this.cache.set(binding.id, { instance, progress, latest });
        } catch { /* conversation failure must not suppress live process evidence */ }
      }
      const heartbeatAt = date(state.heartbeatAt), age = heartbeatAt ? Date.now() - Date.parse(heartbeatAt) : Infinity;
      const model = record(managed.model), usage = record(managed.usage), modelError = record(managed.modelError);
      return { state: states.includes(state.state as RuntimeState) ? state.state as RuntimeState : 'unknown', instanceId: instance,
        observedAt: new Date().toISOString(), heartbeatAt, lastProgressAt: progress, lastEvent: str(state.lastEvent, 100),
        source: 'live', stale: age > 15000 || age < -5000, reason: modelError.category ? `模型回報：${str(modelError.category, 100)}` : null,
        model: typeof model.provider === 'string' && typeof model.id === 'string' ? { provider: model.provider, id: model.id } : null,
        usage: managed.usage ? { tokens: number(usage.tokens), reportedCost: number(usage.reportedCost) } : null,
        capabilities: { conversation: capabilities.includes('conversation'), send: capabilities.includes('send') }, latestMessage: latest,
        sessionEvidence: { sessionId: binding.sessionId, evidenceSource: 'live', historyComplete: false, events: [...events.values()] } };
    } catch {
      const model = record(managed.model), usage = record(managed.usage), stopped = managed.lastRecordedPhase === 'stopped';
      const stoppedAt = date(managed.updatedAt) ?? date(managed.lastProgressAt);
      if (stopped && stoppedAt) remember({ id: `${binding.sessionId}:managed-stop:${stoppedAt}`, kind: 'interrupted', at: stoppedAt, turnId: null, category: 'managed_stop', httpStatus: null });
      return { ...unavailable(), state: stopped ? 'stopped' : 'unavailable',
        reason: stopped ? '上次管理紀錄為已停止；目前沒有即時連線。' : '目前無法讀取代理；不代表工作已完成。',
        lastProgressAt: date(managed.lastProgressAt),
        model: typeof model.provider === 'string' && typeof model.id === 'string' ? { provider: model.provider, id: model.id } : null,
        usage: managed.usage ? { tokens: number(usage.tokens), reportedCost: number(usage.reportedCost) } : null,
        sessionEvidence: { sessionId: binding.sessionId, evidenceSource: stopped ? 'recorded' : 'unavailable', historyComplete: false, events: [...events.values()] } };
    }
  }
  async conversation(binding: AgentBinding, after?: string): Promise<Omit<ConversationView, 'agentId' | 'selectionRevision'>> {
    let instance: string | null = null;
    try {
      const state = assertState(binding, await this.client.requestSession(binding.registryRoot, binding.sessionId, '/status'));
      instance = String(state.instanceId);
      const params = new URLSearchParams({ limit: '30' }); if (after) params.set('after', after);
      const r = record(await this.client.requestSession(binding.registryRoot, binding.sessionId, `/conversation?${params}`, undefined, 2500, String(state.instanceId)));
      return { instanceId: String(state.instanceId), entries: publicEntries(r.entries), cursor: str(r.cursor, 200), headCursor: str(r.headCursor, 200),
        hasMore: r.hasMore === true, observedAt: new Date().toISOString(), source: 'live' };
    } catch (error) {
      // Pi0.7 deliberately sanitizes plain cursor exceptions. A successful fresh
      // page on the SAME instance proves that a reset is available; it does not
      // require exposing the legacy exception or changing a live Pi extension.
      let resetAvailable = false;
      if (after && instance) {
        try { await this.client.requestSession(binding.registryRoot, binding.sessionId, '/conversation?limit=30', undefined, 2500, instance); resetAvailable = true; }
        catch { /* unavailable/replaced source remains unavailable */ }
      }
      if (resetAvailable) throw new ManagerError('STALE_CURSOR', '原對話游標無法接續，請重新讀取目前分支。', 409);
      if (error instanceof ManagerError) throw error;
      throw new ManagerError('UNAVAILABLE', '目前無法讀取這個代理的公開對話。', 503);
    }
  }
  async send(binding: AgentBinding, request: SendRequest): Promise<AdapterReceipt> {
    this.validateMessage(request);
    let state: Record<string, unknown>;
    try { state = assertState(binding, await this.client.requestSession(binding.registryRoot, binding.sessionId, '/status'), request.instanceId); }
    catch (error) { if (error instanceof ManagerError) throw error; throw new ManagerError('UNAVAILABLE', '代理目前無法確認，尚未傳送。', 409); }
    if (state.state === 'waiting_user') throw new ManagerError('WAITING_USER', '代理正在等待原介面的結構化選項；一般訊息無法取代它。', 409);
    // New append/steer message, not an atomic answer to a prior question. Pi owns
    // the exact-instance check and message UUID deduplication at the destination.
    const r = await this.client.requestSession(binding.registryRoot, binding.sessionId, '/messages',
      { id: request.operationId, message: request.message, sender: 'operator-console', mode: request.mode }, 2500, request.instanceId);
    return receipt(r, binding, request.operationId, request.instanceId);
  }
  async receipt(binding: AgentBinding, operation: OperationView): Promise<AdapterReceipt | null> {
    try { return receipt(await this.client.getReceipt(binding.registryRoot, binding.sessionId, operation.id), binding, operation.id, operation.instanceId); }
    catch { return null; }
  }
}
