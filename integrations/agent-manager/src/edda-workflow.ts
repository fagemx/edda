import { execFile } from 'node:child_process';
import { mkdirSync, lstatSync, existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { ManagerError } from './contracts.js';
import { hash, object, text } from './config.js';
import type { OwnerReturnFact, OwnerReturnRead, WorkBinding } from './workflow-contracts.js';

export interface CanonicalTask { id: number; key: string; title: string; status: string; receipt: string | null; updatedAt: string }
export interface LedgerNote { id: string; at: string; text: string }
export interface WorkflowLedger {
  task(binding: WorkBinding): Promise<CanonicalTask>;
  notes(binding: WorkBinding): Promise<LedgerNote[]>;
  append(binding: WorkBinding, value: string): Promise<void>;
  // Optional so observation-only test ledgers stay valid. A failure is reported
  // inside the DTO and never rejects the work row.
  returns?(binding: WorkBinding, env?: Record<string, string>): Promise<OwnerReturnRead | null>;
}
export type EddaRunner = (workspace: string, args: string[], env?: Record<string, string>) => Promise<string>;
export function eddaRunner(executable = process.platform === 'win32' ? 'edda.exe' : 'edda'): EddaRunner {
  return (cwd, args, extra) => new Promise((resolve, reject) => {
    execFile(executable, args, { cwd, shell: false, windowsHide: true, timeout: 15000, maxBuffer: 8 * 1024 * 1024,
      encoding: 'utf8', env: { ...process.env, EDDA_SESSION_ID: 'agent-manager-workflow', ...(extra ?? {}) } }, (error, stdout) => {
      if (error) reject(new ManagerError('LEDGER_UNAVAILABLE', '無法讀寫此工作的 Edda 紀錄；請確認本機 Edda 與專案路徑。', 503));
      else resolve(stdout);
    });
  });
}
// The owner-return read is bounded twice: at most this many pending items are
// examined, and at most this many matched facts are projected into the DTO. The
// scan bound must be applied before matching so a matching return beyond the
// first items is still found; the display bound only trims the projection.
const MAX_RETURN_SCAN = 200;
const MAX_RETURN_MATCHED = 20;
export class EddaWorkflowLedger implements WorkflowLedger {
  constructor(private run: EddaRunner = eddaRunner()) {}
  async task(binding: WorkBinding): Promise<CanonicalTask> {
    const t = object(JSON.parse(await this.run(binding.workspace, ['task', 'show', String(binding.taskId), '--json'])) as unknown);
    if (t.task_id !== binding.taskId) throw new ManagerError('TASK_MISMATCH', 'Edda 回傳的任務與設定不一致。', 409);
    return { id: binding.taskId, key: text(t.created_event_id, 100), title: text(t.title, 2000), status: text(t.status, 50),
      receipt: t.receipt == null ? null : text(t.receipt, 16000), updatedAt: text(t.updated_ts, 100) };
  }
  async notes(binding: WorkBinding): Promise<LedgerNote[]> {
    const output = await this.run(binding.workspace, ['log', '--type', 'note', '--tag', `manager-work-${binding.taskId}`, '--limit', '257', '--json']);
    if (output.trim() === 'No events match the filter.') return [];
    const lines = output.trim() ? output.trim().split(/\r?\n/) : [];
    if (lines.length >= 257) throw new ManagerError('HISTORY_LIMIT', '此工作超過本版 256 筆交接上限，請保留歷史並建立後續任務。', 409);
    return lines.map((line) => {
      const e = object(JSON.parse(line) as unknown), payload = object(e.payload);
      return { id: text(e.event_id, 100), at: text(e.ts, 100), text: text(payload.text, 90000) };
    });
  }
  async append(binding: WorkBinding, value: string): Promise<void> {
    // Argument vector is fixed. Neither API callers nor ledger content can supply
    // executable names, working directories, flags, shell syntax, or task verbs.
    await this.run(binding.workspace, ['note', '--role', 'user', '--tag', `manager-work-${binding.taskId}`, '--', value]);
  }
  // Read-only, bounded, fail-closed: the fixed `return status`/`return pending`
  // argument vectors are the only inputs. Any failure becomes an `error` DTO so
  // a return read can never fail or block the work row, and no registry path or
  // record byte is projected.
  async returns(binding: WorkBinding, env?: Record<string, string>): Promise<OwnerReturnRead | null> {
    const ownerRef = binding.ownerRef;
    if (!ownerRef) return null;
    const failed = (reason: string): OwnerReturnRead => ({ owner: ownerRef, holder: null, pending: 0, total: null, matched: [], dropped: 0, error: reason.slice(0, 300) });
    try {
      const status = object(JSON.parse(await this.run(binding.workspace, ['return', 'status', '--owner', ownerRef, '--json'], env)) as unknown);
      const owner = text(status.owner, 200);
      const pending = status.pending;
      if (!Number.isSafeInteger(pending) || Number(pending) < 0) return failed('負責人回件狀態缺少可用的待領取數量。');
      const total = status.total == null ? null : Number(status.total);
      if (total !== null && (!Number.isSafeInteger(total) || total < 0)) return failed('負責人回件狀態的總數不正確。');
      const list = object(JSON.parse(await this.run(binding.workspace, ['return', 'pending', '--owner', ownerRef, '--json'], env)) as unknown);
      const all = Array.isArray(list.pending) ? list.pending : [];
      // Match across the whole scan window first; the display bound is applied to
      // the matched facts, not to the input, so a matching return past the first
      // few items is still found and counted.
      const items = all.slice(0, MAX_RETURN_SCAN);
      const matchedAll: OwnerReturnFact[] = [];
      let dropped = 0;
      for (const raw of items) {
        let work: string | null = null;
        try {
          const item = object(raw); work = text(item.work, 200);
          if (work !== String(binding.taskId) && work !== binding.id) continue;
          // A matched item whose status is not the bounded vocabulary is a dropped
          // fact, not an absent one: the card must not report a healthy count.
          if (item.status !== 'done' && item.status !== 'failed') { dropped += 1; continue; }
          matchedAll.push({ id: text(item.id, 200), work, status: item.status, result: item.result == null ? null : text(item.result, 2000), postedAt: text(item.posted_at, 100) });
        } catch {
          // Unreadable only where it might be this work's: an unusable item whose
          // id is unknown is conservatively counted as dropped.
          if (work === null || work === String(binding.taskId) || work === binding.id) dropped += 1;
        }
      }
      // The display bound keeps the NEWEST matched facts: the card shows the
      // freshest returns and `deriveWorkProgress`'s newest-wins rule must not lose
      // a fresher return to the bound (the CLI orders `pending` oldest first).
      // Matched facts past the bound are counted, not hidden; likewise the pending
      // items past the scan bound were never examined and may belong to this work.
      // `dropped` therefore covers unusable, out-of-range and over-the-bound
      // items, so the card never reports a healthy count while a return that
      // might belong to this work is withheld.
      const orderedFacts = [...matchedAll].sort((a, b) => b.postedAt.localeCompare(a.postedAt) || a.id.localeCompare(b.id));
      const matched = orderedFacts.slice(0, MAX_RETURN_MATCHED);
      dropped += orderedFacts.length - matched.length + (all.length - items.length);
      return { owner, holder: typeof status.holder === 'string' ? status.holder.slice(0, 200) : null, pending: Number(pending), total, matched, dropped, error: null };
    } catch { return failed('負責人回件狀態暫時無法讀取；未自動重試。'); }
  }
}

/** A kernel-released SQLite transaction is the per-task cross-process mutex.
 * It stores no workflow state and needs no unsafe stale-PID lock reclamation. */
export class WorkflowLocks {
  constructor(private root = join(homedir(), '.edda-agent-manager', 'work-locks')) {}
  async run<T>(taskKey: string, action: () => Promise<T>): Promise<T> {
    mkdirSync(this.root, { recursive: true, mode: 0o700 });
    const file = join(this.root, `${hash(taskKey)}.sqlite`);
    if (lstatSync(this.root).isSymbolicLink()) throw new ManagerError('LOCK_UNAVAILABLE', '工作鎖定目錄不可是連結。', 503);
    for (const name of [file, `${file}-journal`, `${file}-wal`, `${file}-shm`]) {
      if (existsSync(name) && (lstatSync(name).isSymbolicLink() || !lstatSync(name).isFile())) throw new ManagerError('LOCK_UNAVAILABLE', '工作鎖定檔案不正確。', 503);
    }
    const db = new DatabaseSync(file);
    try {
      db.exec('PRAGMA busy_timeout=50');
      try { db.exec('BEGIN IMMEDIATE'); }
      catch { throw new ManagerError('WORK_BUSY', '另一個管理者正在更新此工作，請稍後查詢原操作。', 409); }
      try { return await action(); } finally { db.exec('ROLLBACK'); }
    } finally { db.close(); }
  }
}
