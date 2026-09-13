import { execFile } from 'node:child_process';
import { mkdirSync, lstatSync, existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { ManagerError } from './contracts.js';
import { hash, object, text } from './config.js';
import type { WorkBinding } from './workflow-contracts.js';

export interface CanonicalTask { id: number; key: string; title: string; status: string; receipt: string | null; updatedAt: string }
export interface LedgerNote { id: string; at: string; text: string }
export interface WorkflowLedger {
  task(binding: WorkBinding): Promise<CanonicalTask>;
  notes(binding: WorkBinding): Promise<LedgerNote[]>;
  append(binding: WorkBinding, value: string): Promise<void>;
}
export type EddaRunner = (workspace: string, args: string[]) => Promise<string>;
export function eddaRunner(executable = process.platform === 'win32' ? 'edda.exe' : 'edda'): EddaRunner {
  return (cwd, args) => new Promise((resolve, reject) => {
    execFile(executable, args, { cwd, shell: false, windowsHide: true, timeout: 15000, maxBuffer: 8 * 1024 * 1024,
      encoding: 'utf8', env: { ...process.env, EDDA_SESSION_ID: 'agent-manager-workflow' } }, (error, stdout) => {
      if (error) reject(new ManagerError('LEDGER_UNAVAILABLE', '無法讀寫此工作的 Edda 紀錄；請確認本機 Edda 與專案路徑。', 503));
      else resolve(stdout);
    });
  });
}
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
