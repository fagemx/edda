// Experimental, explicitly owned Pi RPC processes. Not a public session-control API.
import { spawn } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { rpcFrames } from './managed-store.mjs';
import { digest } from './store.mjs';

export const save = (path, value) => writeFileSync(path, JSON.stringify(value, null, 2) + '\n');
export async function sessionSDK(entry) {
  return import(pathToFileURL(join(dirname(dirname(entry)), 'core', 'session-manager.js')));
}
export class ExperimentPi {
  constructor({ entry, cwd, dir, sessionFile, provider, model, thinking = 'low', extensions = [], tools = ['read', 'write', 'edit'] }) {
    mkdirSync(dir, { recursive: true });
    this.dir = dir; this.cwd = cwd; this.pending = new Map(); this.startedAt = Date.now();
    this.metrics = { requests: 0, toolCalls: 0, toolErrors: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, reportedCost: 0, errors: [], writes: [] };
    const env = { ...process.env };
    // No inherited Edda task identity, extensions, skills or automatic provider retries.
    for (const key of Object.keys(env)) if (key.startsWith('EDDA_')) delete env[key];
    this.child = spawn(process.execPath, [entry, '--mode', 'rpc', '--no-extensions', '--no-skills', '--no-prompt-templates',
      '--tools', tools.join(','), '--provider', provider, '--model', model, '--thinking', thinking,
      '--session-dir', dir, ...(sessionFile ? ['--session', sessionFile] : []), ...extensions.flatMap((path) => ['-e', path])],
    { cwd, env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
    this.child.on('error', (error) => this.fail(error));
    this.child.stdin.on('error', (error) => this.fail(error));
    this.child.on('exit', (code) => this.fail(new Error(`Owned Pi exited (${code})`)));
    this.child.stderr.on('data', () => { this.stderrObserved = true; });
    this.child.stdout.on('data', rpcFrames((event) => this.event(event), (error) => this.fail(error)));
  }
  fail(error) {
    for (const item of this.pending.values()) item.reject(error);
    this.pending.clear(); this.turn?.reject(error); this.turn = null;
  }
  event(event) {
    if (event.type === 'response') {
      const item = this.pending.get(event.id);
      if (item) { this.pending.delete(event.id); event.success ? item.resolve(event.data) : item.reject(new Error(event.error || 'RPC command failed')); }
    }
    if (event.type === 'tool_execution_start') {
      this.metrics.toolCalls++;
      if (['write', 'edit'].includes(event.toolName)) this.metrics.writes.push({ tool: event.toolName, path: event.args?.path });
    }
    if (event.type === 'tool_execution_end' && event.isError) this.metrics.toolErrors++;
    if (event.type === 'message_end' && event.message?.role === 'assistant') {
      const message = event.message, usage = message.usage || {};
      this.metrics.requests++;
      for (const key of ['input', 'output', 'cacheRead', 'cacheWrite', 'totalTokens']) this.metrics[key] += Number(usage[key]) || 0;
      this.metrics.reportedCost += Number(usage.cost?.total) || 0;
      if (message.stopReason === 'error' || message.errorMessage) this.metrics.errors.push({ stopReason: message.stopReason,
        category: /429|rate.limit/i.test(message.errorMessage || '') ? 'rate_limit' : 'model_error' });
    }
    if (event.type === 'agent_settled' && this.turn) { this.turn.resolve(); this.turn = null; }
    if (['agent_start', 'agent_settled', 'message_end', 'tool_execution_start', 'tool_execution_end'].includes(event.type)) {
      save(join(this.dir, 'progress.json'), { at: new Date().toISOString(), event: event.type, pid: this.child.pid,
        phase: event.type === 'agent_settled' ? 'idle_execution_unverified' : 'working', metrics: this.metrics });
    }
  }
  async rpc(type, values = {}, timeout = 30000) {
    const id = randomUUID();
    let timer;
    try {
      return await new Promise((resolve, reject) => {
        timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`RPC timeout: ${type}`)); }, timeout);
        this.pending.set(id, { resolve, reject });
        this.child.stdin.write(JSON.stringify({ id, type, ...values }) + '\n');
      });
    } finally { clearTimeout(timer); }
  }
  async ready() {
    const state = await this.rpc('get_state');
    await this.rpc('set_auto_retry', { enabled: false });
    await this.rpc('set_auto_compaction', { enabled: false });
    this.identity = state;
    save(join(this.dir, 'identity.json'), { sessionId: state.sessionId, sessionFile: state.sessionFile,
      cwd: this.cwd, pid: this.child.pid, model: { provider: state.model?.provider, id: state.model?.id }, thinking: state.thinkingLevel });
    return state;
  }
  async prompt(message, timeout = 240000) {
    if (this.turn) throw new Error('Experiment turn already active');
    const startedAt = Date.now(); let timer;
    const settled = new Promise((resolve, reject) => {
      this.turn = { resolve, reject };
      timer = setTimeout(() => { this.turn = null; reject(new Error('Worker timeout')); }, timeout);
    });
    // Attach immediately so an RPC failure cannot leave an unhandled rejection.
    settled.catch(() => {});
    try {
      await this.rpc('prompt', { message });
      await settled;
      const state = await this.rpc('get_state');
      if (state.isStreaming || state.isCompacting || state.pendingMessageCount) throw new Error('Settlement did not produce an idle checkpoint');
      return { elapsedMs: Date.now() - startedAt, state };
    } finally { clearTimeout(timer); this.turn = null; }
  }
  async stop() {
    if (this.child.exitCode !== null || this.child.signalCode !== null) return;
    this.child.stdin.end();
    for (let i = 0; i < 20 && this.child.exitCode === null && this.child.signalCode === null; i++) await delay(100);
    if (this.child.exitCode === null && this.child.signalCode === null) this.child.kill();
    for (let i = 0; i < 50 && this.child.exitCode === null && this.child.signalCode === null; i++) await delay(100);
    if (this.child.exitCode === null && this.child.signalCode === null) throw new Error('Owned process did not stop');
  }
}

export async function checkpoint(parent, SessionManager, destination) {
  const state = await parent.rpc('get_state');
  if (state.isStreaming || state.isCompacting || state.pendingMessageCount) throw new Error('Parent is not idle');
  const bytes = readFileSync(state.sessionFile);
  const manager = SessionManager.open(state.sessionFile);
  if (manager.getSessionId() !== state.sessionId) throw new Error('Parent identity changed');
  const entries = manager.getEntries(), branch = manager.getBranch(manager.getLeafId());
  // This experiment seeds a linear conversation. A branched/compacted source needs
  // a separate public API design; do not silently copy abandoned branches.
  if (entries.length !== branch.length || entries.some((e) => e.type === 'compaction')) throw new Error('Experiment requires a linear, uncompacted checkpoint');
  const messages = manager.buildSessionContext().messages;
  const pending = new Set();
  for (const message of messages) {
    if (message.role === 'assistant') for (const part of message.content || []) if (part.type === 'toolCall') pending.add(part.id);
    if (message.role === 'toolResult') pending.delete(message.toolCallId);
  }
  if (pending.size || messages.at(-1)?.role !== 'assistant') throw new Error('Incomplete tool/assistant checkpoint');
  const after = await parent.rpc('get_state');
  if (after.sessionId !== state.sessionId || after.isStreaming || after.isCompacting || after.pendingMessageCount ||
    !readFileSync(state.sessionFile).equals(bytes)) throw new Error('Parent changed during checkpoint');
  writeFileSync(destination, bytes, { flag: 'wx', mode: 0o600 });
  return { path: destination, digest: digest(bytes), leaf: manager.getLeafId(), sessionId: state.sessionId,
    entries: entries.length, messages: messages.length, bytes: bytes.length };
}

export function forkCheckpoint(SessionManager, source, cwd, dir) {
  if (digest(readFileSync(source.path)) !== source.digest) throw new Error('Checkpoint changed');
  mkdirSync(dir, { recursive: true });
  const childId = randomUUID();
  // Exclusive intent prevents reusing the same experiment worker directory.
  writeFileSync(join(dir, 'fork-intent.json'), JSON.stringify({ childId, cwd, source }), { flag: 'wx' });
  const manager = SessionManager.forkFrom(source.path, cwd, dir, { id: childId });
  if (manager.getSessionId() === source.sessionId || manager.getLeafId() !== source.leaf) throw new Error('Native fork identity/history mismatch');
  save(join(dir, 'fork-receipt.json'), { childId, sessionFile: manager.getSessionFile(), leaf: manager.getLeafId(), sourceDigest: source.digest });
  return { sessionId: childId, sessionFile: manager.getSessionFile() };
}
