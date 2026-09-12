import { createHash } from 'node:crypto';
import { isAbsolute, resolve } from 'node:path';
import { lstatSync, readFileSync } from 'node:fs';
import { ManagerError, MAX_MESSAGE_BYTES, type AgentBinding, type ManagerConfig, type SendRequest } from './contracts.js';

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
    if (role !== 'manager' && role !== 'worker') throw new ManagerError('INVALID_CONFIG', '代理角色不正確。');
    const sessionId = text(a.sessionId, 200);
    if (!/^[a-zA-Z0-9][a-zA-Z0-9_.:-]*$/.test(sessionId)) throw new ManagerError('INVALID_CONFIG', 'Session 識別碼不正確。');
    const projectId = slug(a.projectId);
    if (!projects.some((p) => p.id === projectId)) throw new ManagerError('INVALID_CONFIG', '代理所屬專案未登記。');
    return { id: slug(a.id), name: text(a.name), role, projectId, registryRoot: path(a.registryRoot), sessionId,
      runId: a.runId == null ? null : uuid(a.runId), workspace: path(a.workspace), summaryFile: a.summaryFile == null ? null : path(a.summaryFile) };
  });
  unique(projects.map((p) => p.id)); unique(agents.map((a) => a.id));
  unique(agents.map((a) => `${a.registryRoot}\0${a.sessionId}`));
  const refreshMs = c.refreshMs ?? 3000;
  if (!Number.isSafeInteger(refreshMs) || Number(refreshMs) < 1000 || Number(refreshMs) > 60000) throw new ManagerError('INVALID_CONFIG', '更新間隔必須是 1 到 60 秒。');
  return { version: 1, projects: projects.sort((a, b) => a.priority - b.priority), agents, refreshMs: Number(refreshMs) };
}
export function loadConfig(file: string): ManagerConfig {
  const stat = lstatSync(file);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 262144) throw new ManagerError('INVALID_CONFIG', '設定必須是小於 256 KiB 的一般檔案。');
  try { return parseConfig(JSON.parse(readFileSync(file, 'utf8')) as unknown); }
  catch (error) { if (error instanceof ManagerError) throw error; throw new ManagerError('INVALID_CONFIG', '無法解析管理設定。'); }
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
