import { DatabaseSync } from 'node:sqlite';
import { existsSync, lstatSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { ManagerError, type AgentBinding, type ManagerEvent, type OperationStatus, type OperationView, type SendRequest } from './contracts.js';
import { hash } from './config.js';
import type { BindingObservationState, OwnerInboxAck, OwnerInboxEvent } from './owner-inbox-contracts.js';

export class ManagerStore {
  private db: DatabaseSync;
  constructor(root: string) {
    mkdirSync(root, { recursive: true, mode: 0o700 });
    const file = join(root, 'manager.sqlite');
    if (lstatSync(root).isSymbolicLink() || (existsSync(file) && (lstatSync(file).isSymbolicLink() || !lstatSync(file).isFile()))) throw new Error('Manager storage must not be a link');
    for (const suffix of ['-wal', '-shm', '-journal']) if (existsSync(file + suffix) && lstatSync(file + suffix).isSymbolicLink()) throw new Error('Manager sidecar must not be a link');
    this.db = new DatabaseSync(file);
    const version = this.db.prepare('PRAGMA user_version').get()?.user_version;
    if (version !== 0 && version !== 1 && version !== 2) { this.db.close(); throw new Error('Unsupported manager storage version'); }
    this.db.exec(`PRAGMA journal_mode=WAL; PRAGMA busy_timeout=3000;
      CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, agent_id TEXT NOT NULL, target TEXT NOT NULL, proof_rank INTEGER NOT NULL DEFAULT 0, data TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL, agent_id TEXT NOT NULL, kind TEXT NOT NULL, summary TEXT NOT NULL, operation_id TEXT);
      CREATE TABLE IF NOT EXISTS observations(agent_id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS owner_inbox(id TEXT PRIMARY KEY, work_id TEXT NOT NULL, data TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS owner_binding_observations(id TEXT PRIMARY KEY, data TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS owner_inbox_acks(id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, event_id TEXT NOT NULL);
      PRAGMA user_version=2;`);
    // A committed intent followed by a crash is never evidence that no send occurred.
    for (const row of this.db.prepare("SELECT data FROM operations WHERE json_extract(data,'$.status')='prepared'").all()) {
      const operation = JSON.parse(String(row.data)) as OperationView;
      this.update(operation.id, 'unknown', '服務重啟後結果待確認；不會自動重送。');
    }
  }
  close(): void { this.db.close(); }
  inboxEvents(workId: string, projectId: string, taskId: number, limit = 100): OwnerInboxEvent[] {
    return this.db.prepare("SELECT data FROM owner_inbox WHERE work_id=? AND json_extract(data,'$.projectId')=? AND json_extract(data,'$.taskId')=? ORDER BY json_extract(data,'$.acknowledgedAt') IS NOT NULL, rowid DESC LIMIT ?").all(workId, projectId, taskId, Math.max(1, Math.min(limit, 201))).map(r => JSON.parse(String(r.data)) as OwnerInboxEvent);
  }
  inboxEvent(id: string): OwnerInboxEvent | null {
    const row = this.db.prepare('SELECT data FROM owner_inbox WHERE id=?').get(id);
    return row ? JSON.parse(String(row.data)) as OwnerInboxEvent : null;
  }
  bindingObservation(id: string): BindingObservationState {
    const row = this.db.prepare('SELECT data FROM owner_binding_observations WHERE id=?').get(id);
    return row ? JSON.parse(String(row.data)) as BindingObservationState : { unavailable: false, transition: 0, latestChildEventAt: null };
  }
  recordInboxObservation(bindingId: string, state: BindingObservationState, events: OwnerInboxEvent[]): void {
    this.db.exec('BEGIN IMMEDIATE');
    try {
      for (const event of events) this.db.prepare('INSERT OR IGNORE INTO owner_inbox(id,work_id,data) VALUES(?,?,?)').run(event.id, event.workId, JSON.stringify(event));
      this.db.prepare('INSERT INTO owner_binding_observations(id,data) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data').run(bindingId, JSON.stringify(state));
      this.db.exec('COMMIT');
    } catch (error) { this.db.exec('ROLLBACK'); throw error; }
  }
  acknowledgeInbox(request: OwnerInboxAck): OwnerInboxEvent {
    const fingerprint = hash(JSON.stringify(request));
    this.db.exec('BEGIN IMMEDIATE');
    try {
      const existing = this.db.prepare('SELECT fingerprint FROM owner_inbox_acks WHERE id=?').get(request.actionId);
      if (existing && existing.fingerprint !== fingerprint) throw new ManagerError('ACK_CONFLICT', '此確認編號已使用於其他內容。', 409);
      const event = this.inboxEvent(request.eventId);
      if (!event) throw new ManagerError('NOT_FOUND', '找不到這則通知。', 404);
      if (event.acknowledgementId && event.acknowledgementId !== request.actionId) throw new ManagerError('ALREADY_ACKNOWLEDGED', '這則通知已確認。', 409);
      if (!existing) {
        event.acknowledgedAt = new Date().toISOString(); event.acknowledgementId = request.actionId; event.evidence = request.evidence;
        this.db.prepare('INSERT INTO owner_inbox_acks(id,fingerprint,event_id) VALUES(?,?,?)').run(request.actionId, fingerprint, request.eventId);
        this.db.prepare('UPDATE owner_inbox SET data=? WHERE id=?').run(JSON.stringify(event), event.id);
      }
      this.db.exec('COMMIT'); return event;
    } catch (error) { this.db.exec('ROLLBACK'); throw error; }
  }
  setting(key: string): string | null { const row = this.db.prepare('SELECT value FROM settings WHERE key=?').get(key); return typeof row?.value === 'string' ? row.value : null; }
  putSetting(key: string, value: string): void { this.db.prepare('INSERT INTO settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value').run(key, value); }
  ensureSetting(key: string, value: string): string {
    this.db.prepare('INSERT OR IGNORE INTO settings(key,value) VALUES(?,?)').run(key, value);
    return this.setting(key)!;
  }
  operation(id: string): OperationView | null {
    const row = this.db.prepare('SELECT data FROM operations WHERE id=?').get(id);
    return typeof row?.data === 'string' ? JSON.parse(row.data) as OperationView : null;
  }
  operations(limit = 40): OperationView[] {
    return this.db.prepare('SELECT data FROM operations ORDER BY rowid DESC LIMIT ?').all(limit).map((r) => JSON.parse(String(r.data)) as OperationView);
  }
  existing(agentId: string, request: SendRequest): OperationView | null {
    const row = this.db.prepare('SELECT fingerprint,data FROM operations WHERE id=?').get(request.operationId);
    if (!row) return null;
    if (row.fingerprint !== hash(JSON.stringify({ agentId, ...request }))) throw new ManagerError('CONFLICT', '這個操作編號已有不同的內容，未再次傳送。', 409);
    return JSON.parse(String(row.data)) as OperationView;
  }
  target(id: string): AgentBinding | null {
    const row = this.db.prepare('SELECT target FROM operations WHERE id=?').get(id);
    return typeof row?.target === 'string' ? JSON.parse(row.target) as AgentBinding : null;
  }
  begin(binding: AgentBinding, request: SendRequest): OperationView {
    const agentId = binding.id;
    const now = new Date().toISOString();
    const result: OperationView = { id: request.operationId, agentId, instanceId: request.instanceId, basisCursor: request.basisCursor,
      message: request.message, mode: request.mode, status: 'prepared', createdAt: now, updatedAt: now, notice: '操作已記錄，傳送結果尚未確認。' };
    this.db.exec('BEGIN IMMEDIATE');
    try {
      this.db.prepare('INSERT INTO operations(id,fingerprint,agent_id,target,data) VALUES(?,?,?,?,?)').run(result.id, hash(JSON.stringify({ agentId, ...request })), agentId, JSON.stringify(binding), JSON.stringify(result));
      this.addEvent(agentId, 'message', '已記錄傳送意圖', result.id);
      this.db.exec('COMMIT');
    } catch (error) { this.db.exec('ROLLBACK'); throw error; }
    return result;
  }
  update(id: string, status: OperationStatus, notice: string): OperationView {
    const current = this.operation(id);
    if (!current) throw new ManagerError('NOT_FOUND', '找不到這次操作。', 404);
    if (['settled', 'failed'].includes(current.status) || (current.status === status && current.notice === notice)) return current;
    const ranks: Record<OperationStatus, number> = { prepared: 0, unconfirmed: 0, accepted: 1, queued: 2, started: 3, settled: 4, failed: 4, unknown: -1 };
    const previousRank = Number(this.db.prepare('SELECT proof_rank FROM operations WHERE id=?').get(id)?.proof_rank || 0);
    if (ranks[status] >= 0 && ranks[status] < previousRank) return current;
    const result = { ...current, status, notice, updatedAt: new Date().toISOString() };
    this.db.exec('BEGIN IMMEDIATE');
    try {
      this.db.prepare('UPDATE operations SET data=?,proof_rank=? WHERE id=?').run(JSON.stringify(result), Math.max(previousRank, ranks[status]), id);
      if (current.status !== status) this.addEvent(current.agentId, 'message', notice, id);
      this.db.exec('COMMIT');
    } catch (error) { this.db.exec('ROLLBACK'); throw error; }
    return result;
  }
  observation(agentId: string, fingerprint: string, summary: string): void {
    if (this.db.prepare('SELECT fingerprint FROM observations WHERE agent_id=?').get(agentId)?.fingerprint === fingerprint) return;
    this.db.exec('BEGIN IMMEDIATE');
    try {
      this.db.prepare('INSERT INTO observations(agent_id,fingerprint) VALUES(?,?) ON CONFLICT(agent_id) DO UPDATE SET fingerprint=excluded.fingerprint').run(agentId, fingerprint);
      this.addEvent(agentId, 'observation', summary, null);
      this.db.exec('COMMIT');
    } catch (error) { this.db.exec('ROLLBACK'); throw error; }
  }
  private addEvent(agentId: string, kind: ManagerEvent['kind'], summary: string, operationId: string | null): void {
    this.db.prepare('INSERT INTO events(at,agent_id,kind,summary,operation_id) VALUES(?,?,?,?,?)').run(new Date().toISOString(), agentId, kind, summary.slice(0, 500), operationId);
  }
  events(limit = 60): ManagerEvent[] {
    return this.db.prepare('SELECT * FROM events ORDER BY id DESC LIMIT ?').all(limit).map((r) => ({ id: Number(r.id), at: String(r.at),
      agentId: String(r.agent_id), kind: r.kind as ManagerEvent['kind'], summary: String(r.summary), operationId: r.operation_id == null ? null : String(r.operation_id) }));
  }
}
