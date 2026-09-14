import { createHash } from 'node:crypto';
import { isAbsolute, resolve } from 'node:path';
import { lstatSync, readFileSync } from 'node:fs';
import { ManagerError, MAX_MESSAGE_BYTES, type AgentBinding, type ManagerConfig, type RegisterCandidateRequest, type SendRequest } from './contracts.js';

export function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new ManagerError('INVALID_DATA', '資料格式不正確。');
  return value as Record<string, unknown>;
}
export function text(value: unknown, max = 200): string {
  if (typeof value !== 'string' || !value.trim() || value.length > max) throw new ManagerError('INVALID_DATA', '文字欄位為空或過長。');
  return value;
}
export function slug(value: unknown): string {
  const result = text(value, 64);
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(result)) throw new ManagerError('INVALID_ID', '識別碼格式不正確。');
  return result;
}
export function uuid(value: unknown): string {
  const result = text(value, 36).toLowerCase();
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(result)) throw new ManagerError('INVALID_ID', '操作識別碼格式不正確。');
  return result;
}
function path(value: unknown): string {
  const result = text(value, 4096);
  if (!isAbsolute(result)) throw new ManagerError('INVALID_CONFIG', '設定中的路徑必須是絕對路徑。');
  return resolve(result);
}
function array(value: unknown, max: number): unknown[] {
  if (!Array.isArray(value) || value.length > max) throw new ManagerError('INVALID_CONFIG', '設定清單格式或大小不正確。');
  return value;
}
function unique(values: string[]): void {
  if (new Set(values).size !== values.length) throw new ManagerError('INVALID_CONFIG', '設定中有重複的識別碼或代理。');
}
export const hash = (value: string): string => createHash('sha256').update(value).digest('hex');
export const selectionRevision = (binding: AgentBinding): string => hash(JSON.stringify(binding));
export function parseConfig(input: unknown): ManagerConfig {
  const c = object(input);
  if (c.version !== 1) throw new ManagerError('INVALID_CONFIG', '不支援的設定版本。');
  const projects = array(c.projects, 16).map((value) => {
    const p = object(value), priority = p.priority ?? 10;
    if (!Number.isSafeInteger(priority) || Number(priority) < 0 || Number(priority) > 100) throw new ManagerError('INVALID_CONFIG', '優先順序必須是 0 到 100。');
    const resources = array(p.resources ?? [], 16).map((value) => {
      const r = object(value);
      return { id: slug(r.id), name: text(r.name), kind: text(r.kind), details: text(r.details, 2000), owner: text(r.owner, 500), source: text(r.source, 500) };
    });
    unique(resources.map((r) => r.id));
    return { id: slug(p.id), name: text(p.name), priority: Number(priority), resources };
  });
  const agents = array(c.agents, 32).map((value): AgentBinding => {
    const a = object(value), role = a.role;
    if (a.transport != null && a.transport !== 'pi' && a.transport !== 'codex') throw new ManagerError('INVALID_CONFIG', '不支援的代理來源。');
    if (role !== 'manager' && role !== 'worker') throw new ManagerError('INVALID_CONFIG', '代理角色不正確。');
    const sessionId = text(a.sessionId, 200);
    if (!/^[a-zA-Z0-9][a-zA-Z0-9_.:-]*$/.test(sessionId)) throw new ManagerError('INVALID_CONFIG', 'Session 識別碼不正確。');
    const projectId = slug(a.projectId);
    if (!projects.some((p) => p.id === projectId)) throw new ManagerError('INVALID_CONFIG', '代理所屬專案未登記。');
    if (a.transport === 'codex' && a.runId != null) throw new ManagerError('INVALID_CONFIG', 'Codex 記錄來源不能帶入 Pi 執行編號。');
    return { id: slug(a.id), name: text(a.name), role, projectId, registryRoot: path(a.registryRoot ?? (a.transport === 'codex' ? a.workspace : undefined)), sessionId,
      runId: a.runId == null ? null : uuid(a.runId), workspace: path(a.workspace), summaryFile: a.summaryFile == null ? null : path(a.summaryFile),
      ...(a.transport === 'codex' ? { transport: 'codex', transcriptFile: path(a.transcriptFile) } : {}) };
  });
  unique(projects.map((p) => p.id)); unique(agents.map((a) => a.id));
  unique(agents.map((a) => `${a.registryRoot}\0${a.sessionId}`));
  const works = array(c.works ?? [], 32).map((value) => {
    const w = object(value), projectId = slug(w.projectId), ownerAgentId = slug(w.ownerAgentId);
    if (!Number.isSafeInteger(w.taskId) || Number(w.taskId) < 1) throw new ManagerError('INVALID_CONFIG', '工作必須連結有效的 Edda 任務編號。');
    if (!agents.some((a) => a.id === ownerAgentId && a.projectId === projectId)) throw new ManagerError('INVALID_CONFIG', '工作負責人必須是同專案已選取的代理。');
    // Optional owner reference for the native `edda return` mailbox. Absent stays
    // backwards compatible; present must be a bounded, argument-safe locator.
    let ownerRef: string | null = null;
    if (w.ownerRef != null) {
      ownerRef = text(w.ownerRef, 200);
      if (!/^[A-Za-z0-9][A-Za-z0-9/_.:-]{0,199}$/.test(ownerRef)) throw new ManagerError('INVALID_CONFIG', '工作的負責人參照格式不正確。');
    }
    // Optional pinned owner mailbox for the `edda return` read. Absent/null stays
    // backwards compatible; present must be an absolute path.
    const ownerRoot = w.ownerRoot == null ? null : path(w.ownerRoot);
    return { id: slug(w.id), projectId, taskId: Number(w.taskId), workspace: path(w.workspace), ownerAgentId, ownerRef, ownerRoot };
  });
  unique(works.map((w) => w.id)); unique(works.map((w) => `${w.workspace}\0${w.taskId}`));
  const refreshMs = c.refreshMs ?? 3000;
  if (!Number.isSafeInteger(refreshMs) || Number(refreshMs) < 1000 || Number(refreshMs) > 60000) throw new ManagerError('INVALID_CONFIG', '更新間隔必須是 1 到 60 秒。');
  return { version: 1, projects: projects.sort((a, b) => a.priority - b.priority), agents, refreshMs: Number(refreshMs), ...(works.length ? { works } : {}), ...(c.continuityExecutable == null ? {} : { continuityExecutable: path(c.continuityExecutable) }) };
}
export function loadConfig(file: string): ManagerConfig {
  const stat = lstatSync(file);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 262144) throw new ManagerError('INVALID_CONFIG', '設定必須是小於 256 KiB 的一般檔案。');
  try { return parseConfig(JSON.parse(readFileSync(file, 'utf8')) as unknown); }
  catch (error) { if (error instanceof ManagerError) throw error; throw new ManagerError('INVALID_CONFIG', '無法解析管理設定。'); }
}
export function parseRegisterCandidate(input: unknown): RegisterCandidateRequest {
  const r = object(input), role = r.role;
  if (role !== 'manager' && role !== 'worker') throw new ManagerError('INVALID_ROLE', '代理角色不正確。');
  const candidateId = text(r.candidateId, 64).toLowerCase();
  if (!/^[0-9a-f]{16,64}$/.test(candidateId)) throw new ManagerError('INVALID_ID', '候選項目識別碼格式不正確。');
  return { candidateId, id: slug(r.id), name: text(r.name), role, projectId: slug(r.projectId) };
}
export function parseSend(input: unknown): SendRequest {
  const r = object(input), mode = r.mode;
  if (mode !== 'followUp' && mode !== 'steer') throw new ManagerError('INVALID_REQUEST', '請選擇追加訊息或優先指令。');
  if (typeof r.message !== 'string' || !r.message.trim()) throw new ManagerError('INVALID_REQUEST', '訊息不可空白。');
  const message = r.message;
  if (Buffer.byteLength(message) > MAX_MESSAGE_BYTES) throw new ManagerError('TOO_LARGE', '訊息超過 12 KiB。', 413);
  return { operationId: uuid(r.operationId), selectionRevision: text(r.selectionRevision, 64), instanceId: uuid(r.instanceId),
    basisCursor: r.basisCursor == null ? null : text(r.basisCursor, 200), mode, message };
}
