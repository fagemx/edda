import { execFile } from 'node:child_process';
import { mkdtempSync, mkdirSync, lstatSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ManagerError } from './contracts.js';
import { hash, object, slug, text, uuid } from './config.js';
import type { AgentManager } from './manager.js';
import { WorkflowLocks } from './edda-workflow.js';
import { MAX_CONTINUATION_INPUT_BYTES, MAX_CONTINUITY_BUNDLE_BYTES, type ContinuationImportRequest, type ContinuationOperation, type ContinuationPublication, type ContinuationPublishRequest, type ContinuationTakeoverRequest, type NativeCapsuleInput, type NativeRestore, type PortableBundle } from './continuation-contracts.js';

export type NativeRunner = (workspace: string, args: string[]) => Promise<{ ok: boolean; stdout: string }>;
export interface ContinuationOptions { executable?: string; run?: NativeRunner; tempRoot?: string; locks?: WorkflowLocks }
interface StoredOperation extends ContinuationOperation { fingerprint: string; bindingDigest: string; sourceTaskKey: string; revision: string; kind: 'save' | 'import' }
const capsuleId = (value: unknown): string => { const id = text(value, 100); if (!/^cap_[a-z0-9]+$/.test(id)) throw new ManagerError('INVALID_CAPSULE_ID', '原生 capsule ID 格式不正確。'); return id; };
const bounded = (value: unknown, size: number): string => { const encoded = JSON.stringify(value); if (!encoded || Buffer.byteLength(encoded) > size) throw new ManagerError('CONTINUITY_TOO_LARGE', `必要上下文超過 ${size} bytes 上限。`, 413); return encoded; };

/** Only local configured executable/workspace and generated temporary paths reach argv. */
export function nativeContinuityRunner(executable = process.platform === 'win32' ? 'edda.exe' : 'edda'): NativeRunner {
  return (cwd, args) => new Promise(resolve => execFile(executable, args, { cwd, shell: false, windowsHide: true, timeout: 15000, maxBuffer: MAX_CONTINUITY_BUNDLE_BYTES * 2, encoding: 'utf8', env: { ...process.env, EDDA_SESSION_ID: 'agent-manager-continuity' } }, (error, stdout) => resolve({ ok: !error, stdout })));
}
function nativeEnvelope(input: unknown): NativeRestore {
  const r = object(input), c = object(r.capsule), state = object(c.state);
  if (r.data_authority !== 'data_only' || c.capsule_version !== 1 || typeof r.imported !== 'boolean' || typeof r.legacy_partial !== 'boolean' || !Array.isArray(r.warnings) || !r.warnings.every(w => typeof w === 'string')) throw new ManagerError('CONTINUITY_INVALID', '原生 continuity 回傳格式不正確。', 502);
  capsuleId(c.capsule_id); text(r.local_event_id, 100); text(r.origin_event_id, 100); text(state.next_action, 16000);
  object(c.git); object(c.repository); object(c.references);
  return input as NativeRestore;
}
function parseInput(value: unknown): NativeCapsuleInput {
  bounded(value, MAX_CONTINUATION_INPUT_BYTES);
  const input = object(value), state = object(input.state);
  if (input.capsule_version !== 1) throw new ManagerError('CONTINUITY_INVALID', '需要原生 ContextCapsuleInputV1。');
  text(state.next_action, MAX_CONTINUATION_INPUT_BYTES);
  return JSON.parse(JSON.stringify(value)) as NativeCapsuleInput;
}
export class ContinuationService {
  private run: NativeRunner;
  private queue = new Map<string, Promise<unknown>>();
  private capabilities = new Map<string, Promise<void>>();
  private locks: WorkflowLocks;
  private closing = false;
  constructor(private manager: AgentManager, private options: ContinuationOptions = {}) { this.run = options.run ?? nativeContinuityRunner(options.executable); this.locks = options.locks ?? new WorkflowLocks(); }
  async stop(): Promise<void> { this.closing = true; await Promise.allSettled(this.queue.values()); }
  private binding(id: string) {
    const binding = this.manager.config.works?.find(w => w.id === id);
    if (!binding) throw new ManagerError('NOT_FOUND', '此工作未加入管理清單。', 404); return binding;
  }
  private async capable(workspace: string, operation: 'save' | 'restore' | 'export' | 'import') {
    const key = `${workspace}\0${operation}`;
    let promise = this.capabilities.get(key);
    if (!promise) {
      promise = this.run(workspace, ['continuity', operation, '--help']).then(result => {
        const flags = operation === 'save' ? ['--file', '--json'] : operation === 'restore' ? ['CAPSULE_ID', '--json'] : operation === 'import' ? ['--json'] : ['--out'];
        if (!result.ok || flags.some(flag => !result.stdout.includes(flag))) throw new ManagerError('CONTINUITY_UNAVAILABLE', '已設定的 Edda 不支援原生 continuity；未使用其他格式代替。', 503);
      });
      this.capabilities.set(key, promise); promise.catch(() => this.capabilities.delete(key));
    }
    await promise;
  }
  private temporary<T>(action: (file: string) => Promise<T>): Promise<T> {
    const base = this.options.tempRoot ?? tmpdir();
    mkdirSync(base, { recursive: true, mode: 0o700 });
    if (lstatSync(base).isSymbolicLink()) throw new ManagerError('TEMP_UNAVAILABLE', '必要上下文暫存目錄不可是連結。', 503);
    const dir = mkdtempSync(join(base, 'edda-continuity-'));
    return action(join(dir, 'context.json')).finally(() => rmSync(dir, { recursive: true, force: true }));
  }
  private parseResult(stdout: string): Record<string, unknown> {
    try { return object(JSON.parse(stdout) as unknown); } catch { throw new ManagerError('CONTINUITY_INVALID', '原生 continuity 未回傳可核對的 JSON。', 502); }
  }
  async restore(id: string, value: string): Promise<NativeRestore> {
    const binding = this.binding(id), selected = capsuleId(value); await this.capable(binding.workspace, 'restore');
    const result = await this.run(binding.workspace, ['continuity', 'restore', selected, '--json']);
    if (!result.ok) throw new ManagerError('CONTINUITY_UNAVAILABLE', '無法讀取指定原生 capsule，未選取其他上下文替代。', 409);
    const context = nativeEnvelope(this.parseResult(result.stdout));
    if (context.capsule.capsule_id !== selected) throw new ManagerError('CONTINUITY_INVALID', '原生 capsule 身分與請求不同。', 502);
    return context;
  }
  private async exported(id: string, selected: string): Promise<PortableBundle | null> {
    const binding = this.binding(id); await this.capable(binding.workspace, 'export');
    return this.temporary(async file => {
      const result = await this.run(binding.workspace, ['continuity', 'export', capsuleId(selected), '--out', file]);
      // Local-only capsules remain useful; native product owns export refusal.
      if (!result.ok) return null;
      const bytes = readFileSync(file); if (bytes.length > MAX_CONTINUITY_BUNDLE_BYTES) throw new ManagerError('CONTINUITY_TOO_LARGE', '原生 bundle 超過上限。', 413);
      const bundle = this.parseResult(bytes.toString('utf8'));
      if (bundle.bundle_version !== 1 || bundle.data_authority !== 'data_only' || bundle.origin_capsule_id !== selected) throw new ManagerError('CONTINUITY_INVALID', '原生 bundle 身分不正確。', 502);
      return bundle as unknown as PortableBundle;
    });
  }
  private readOperation(actionId: string): StoredOperation | null {
    const value = this.manager.store.setting(`continuity:operation:${actionId}`); return value ? JSON.parse(value) as StoredOperation : null;
  }
  private saveOperation(operation: StoredOperation) { this.manager.store.putSetting(`continuity:operation:${operation.actionId}`, JSON.stringify(operation)); }
  private async response(id: string, operation: StoredOperation | null): Promise<ContinuationPublication> {
    const state = await this.manager.works.continuationSnapshot(id);
    const selected = operation?.capsuleId ?? state.view.continuity?.capsuleId ?? null;
    const context = selected ? await this.restore(id, selected) : null;
    const bundle = context ? await this.exported(id, context.capsule.capsule_id) : null;
    return { operation: operation ? { actionId: operation.actionId, status: operation.status, capsuleId: operation.capsuleId, notice: operation.notice, ...(operation.nativeStatus ? { nativeStatus: operation.nativeStatus } : {}) } : null, context, bundle, work: state.view };
  }
  async latest(id: string): Promise<ContinuationPublication> {
    const binding = this.binding(id), state = await this.manager.works.continuationSnapshot(id);
    const last = this.manager.store.setting(`continuity:last:${hash(JSON.stringify([binding, state.taskKey]))}`);
    return this.response(id, last ? this.readOperation(last) : null);
  }
  async publish(id: string, request: ContinuationPublishRequest): Promise<ContinuationPublication> {
    const r = object(request), input = parseInput(r.input);
    return this.write(id, uuid(r.actionId), text(r.revision, 64), 'save', input);
  }
  async import(id: string, request: ContinuationImportRequest): Promise<ContinuationPublication> {
    const r = object(request); bounded(r.bundle, MAX_CONTINUITY_BUNDLE_BYTES);
    return this.write(id, uuid(r.actionId), text(r.revision, 64), 'import', object(r.bundle));
  }
  private async write(id: string, actionId: string, revision: string, kind: 'save' | 'import', payload: unknown): Promise<ContinuationPublication> {
    if (this.closing) throw new ManagerError('STOPPING', '管理服務正在停止，請稍後查詢原續作操作。', 503);
    const previous = this.queue.get(id) ?? Promise.resolve();
    const promise = previous.catch(() => {}).then(async () => {
      const state = await this.manager.works.continuationSnapshot(id);
      return this.locks.run(`continuity-task:${state.taskKey}`, () => this.locks.run(`continuity-action:${actionId}`, () => this.performWrite(id, actionId, revision, kind, payload)));
    });
    this.queue.set(id, promise); try { return await promise; } finally { if (this.queue.get(id) === promise) this.queue.delete(id); }
  }
  private async performWrite(id: string, actionId: string, revision: string, kind: 'save' | 'import', payload: unknown): Promise<ContinuationPublication> {
    const binding = this.binding(id), state = await this.manager.works.continuationSnapshot(id), bindingDigest = hash(JSON.stringify(binding));
    const fingerprint = hash(JSON.stringify({ id, revision, kind, payload, bindingDigest, taskKey: state.taskKey }));
    let operation = this.readOperation(actionId);
    if (operation) {
      if (operation.fingerprint !== fingerprint) throw new ManagerError('ACTION_CONFLICT', '這個續作操作編號已有不同內容。', 409);
      // A durable intent cannot prove save/import never ran. Never repeat it.
      if (operation.capsuleId && operation.status !== 'attached') await this.attach(id, operation);
      return this.response(id, operation);
    }
    if (state.view.revision !== revision) throw new ManagerError('STALE_WORK', '工作已更新，請重新讀取後再發布。', 409);
    const lastKey = `continuity:last:${hash(JSON.stringify([binding, state.taskKey]))}`, lastId = this.manager.store.setting(lastKey), last = lastId ? this.readOperation(lastId) : null;
    if (last && last.status !== 'attached') throw new ManagerError('CONTINUITY_PENDING', '上一份上下文仍待確認；請先查詢原操作，未重複建立 capsule。', 409);
    await this.capable(binding.workspace, kind); await this.capable(binding.workspace, 'restore'); await this.capable(binding.workspace, 'export');
    let nativePayload = payload;
    if (kind === 'save') {
      const input = payload as NativeCapsuleInput;
      const questions = [...(input.state.open_questions ?? [])];
      if (state.view.pendingInstruction && !state.view.pendingInstruction.acknowledgedAt) questions.push(`Unresolved instruction ${state.view.pendingInstruction.id}; read original work event before proceeding.`);
      if (state.view.deliveryOperationId && !['delivered', 'accepted'].includes(state.view.stage)) questions.push(`Unresolved operation ${state.view.deliveryOperationId}, status=${state.view.deliveryStatus ?? 'unknown'}; query original receipt, do not replay.`);
      nativePayload = { ...input, state: { ...input.state, open_questions: questions }, references: { task_ids: [...new Set([...(input.references?.task_ids ?? []), String(binding.taskId)])], event_ids: [...new Set([...(input.references?.event_ids ?? []), state.taskKey])] } };
      bounded(nativePayload, MAX_CONTINUATION_INPUT_BYTES);
    }
    operation = { actionId, status: 'unknown', capsuleId: null, notice: '原生寫入結果待確認；不會自動重複 save/import。', fingerprint, bindingDigest, sourceTaskKey: state.taskKey, revision, kind };
    // ensureSetting is INSERT OR IGNORE: another service using this store cannot
    // claim the same action ID twice, including the gap before the native effect.
    const proposed = JSON.stringify(operation), stored = this.manager.store.ensureSetting(`continuity:operation:${actionId}`, proposed);
    if (stored !== proposed) { const existing = JSON.parse(stored) as StoredOperation; if (existing.fingerprint !== fingerprint) throw new ManagerError('ACTION_CONFLICT', '續作編號已被使用。', 409); return this.response(id, existing); }
    this.manager.store.putSetting(lastKey, actionId);
    const result = await this.temporary(async file => {
      writeFileSync(file, JSON.stringify(nativePayload), { encoding: 'utf8', mode: 0o600, flag: 'wx' });
      return this.run(binding.workspace, kind === 'save' ? ['continuity', 'save', '--file', file, '--json'] : ['continuity', 'import', file, '--json']);
    });
    let output: Record<string, unknown>;
    try { output = this.parseResult(result.stdout); } catch { return this.response(id, operation); }
    const status = ['SAVED_LOCAL', 'SAVED_UNVERIFIED', 'SAVE_FAILED', 'imported', 'skipped', 'refused'].includes(String(output.status)) ? String(output.status) : 'unrecognized';
    operation.nativeStatus = status;
    if (output.data_authority !== 'data_only' || output.capsule_id == null) {
      // Never echo arbitrary native errors: they can contain source paths or
      // rejected secret input. Recognized classes give an actionable safe reason.
      const error = typeof output.error === 'string' ? output.error : '';
      const reason = error.includes('different repository') ? '來源與目的地儲存庫不同，請選擇正確專案。'
        : /secret|credential|private key/i.test(error) ? '輸入含敏感資訊，請先移除憑證。'
          : /schema|invalid|malformed/i.test(error) ? '原生格式或參照無效，請檢查原生 continuity 輸入。'
            : /integrity conflict/i.test(error) ? '來源 capsule 身分與既有紀錄衝突，請核對原始 bundle。'
              : '原生程序未提供可核對的 capsule 身分，請檢查原始 Edda 紀錄。';
      operation.notice = `${status}：${reason} 原操作仍保留待核對，不會自動重複寫入。`; this.saveOperation(operation);
      return this.response(id, operation);
    }
    try { operation.capsuleId = capsuleId(output.capsule_id); } catch { return this.response(id, operation); }
    operation.status = 'saved'; operation.notice = '原生 capsule 身分已記錄，核對原文後連結工作。'; this.saveOperation(operation);
    await this.attach(id, operation);
    return this.response(id, operation);
  }
  private async attach(id: string, operation: StoredOperation) {
    const context = await this.restore(id, operation.capsuleId!);
    const state = await this.manager.works.continuationSnapshot(id);
    const reference = { capsuleId: context.capsule.capsule_id, localEventId: context.local_event_id, originEventId: context.origin_event_id };
    const existing = state.actions.find(a => a.actionId === operation.actionId);
    if (existing && (existing.kind !== 'attach_continuity' || JSON.stringify(existing.reference) !== JSON.stringify(reference))) throw new ManagerError('ACTION_CONFLICT', '續作操作編號已有不同工作紀錄。', 409);
    // Attaching a known native capsule changes no owner, next step or task status.
    // After an uncertain append, reuse its exact action; otherwise attach the
    // saved reference to the latest revision without repeating native save.
    await this.manager.works.act(id, existing ?? { actionId: operation.actionId, revision: state.view.revision, kind: 'attach_continuity', reference });
    operation.status = 'attached'; operation.notice = '原生必要上下文已保存並連結工作；未啟動代理。'; this.saveOperation(operation);
  }
  async takeover(id: string, request: ContinuationTakeoverRequest) {
    if (this.closing) throw new ManagerError('STOPPING', '管理服務正在停止，請稍後查詢原接手操作。', 503);
    const previous = this.queue.get(id) ?? Promise.resolve();
    const promise = previous.catch(() => {}).then(() => this.performTakeover(id, request));
    this.queue.set(id, promise); try { return await promise; } finally { if (this.queue.get(id) === promise) this.queue.delete(id); }
  }
  private async performTakeover(id: string, request: ContinuationTakeoverRequest) {
    const r = object(request), actionId = uuid(r.actionId), revision = text(r.revision, 64), selected = capsuleId(r.capsuleId), ownerAgentId = slug(r.ownerAgentId), environmentEvidence = text(r.environmentEvidence, 1000), releaseEvidence = text(r.releaseEvidence, 1000);
    const context = await this.restore(id, selected);
    const state = await this.manager.works.continuationSnapshot(id);
    if (!state.actions.some(action => action.actionId === actionId) && state.view.continuity?.capsuleId !== selected) throw new ManagerError('CONTEXT_NOT_ATTACHED', '請先把原生 capsule 連結至這件工作。', 409);
    const evidence = `Native continuity ${selected}; local=${context.local_event_id}; origin=${context.origin_event_id}. Environment: ${environmentEvidence}. Original writer release: ${releaseEvidence}. DATA ONLY; records local responsibility, not remote fencing.`;
    return this.manager.works.act(id, { kind: 'handoff_owner', actionId, revision, ownerAgentId, evidence });
  }
}
