import { execFileSync } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig } from '../src/config.js';
import { EddaWorkflowLedger, eddaRunner, WorkflowLocks } from '../src/edda-workflow.js';
import { unavailable } from '../src/pi-adapter.js';
import { serve } from '../src/http.js';

// Isolated native CLI + actual HTTP/browser fixture. No worker is launched.
const executable = process.argv[2]; if (!executable) throw new Error('Supply a native Edda executable');
const root = mkdtempSync(join(tmpdir(), 'manager-continuity-browser-'));
const source = join(root, 'source'), destination = join(root, 'destination');
const command = (cwd: string, exe: string, args: string[]) => execFileSync(exe, args, { cwd, windowsHide: true, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
mkdirSync(source); command(source, 'git', ['init']); command(source, 'git', ['config', 'user.name', 'Fixture']); command(source, 'git', ['config', 'user.email', 'fixture@example.invalid']);
writeFileSync(join(source, 'example.txt'), 'Saved artifact'); command(source, 'git', ['add', '.']); command(source, 'git', ['commit', '-m', 'fixture']);
command(root, 'git', ['clone', '--no-hardlinks', source, destination]);
command(source, 'git', ['remote', 'add', 'origin', 'https://example.invalid/continuity/browser.git']);
command(destination, 'git', ['remote', 'set-url', 'origin', 'https://example.invalid/continuity/browser.git']);
for (const dir of [source, destination]) { command(dir, executable, ['init', '--no-hooks']); command(dir, executable, ['task', 'new', dir === source ? '來源工作' : '目的地接手工作']); }
const config = parseConfig({ version: 1, continuityExecutable: executable, projects: [{ id: 'fixture', name: '必要上下文實測' }], agents: [source, destination].map((workspace, i) => ({ id: i ? 'destination' : 'source', name: i ? '接手管理者' : '來源管理者', role: 'manager', projectId: 'fixture', workspace, registryRoot: root, sessionId: randomUUID() })), works: [source, destination].map((workspace, i) => ({ id: i ? 'destination' : 'source', projectId: 'fixture', taskId: 1, workspace, ownerAgentId: i ? 'destination' : 'source' })) });
const store = new ManagerStore(join(root, 'store'));
const manager = new AgentManager(config, store, { observe: async () => unavailable(), conversation: async () => ({ entries: [], cursor: null, headCursor: null, hasMore: false, observedAt: new Date().toISOString(), source: 'unavailable', instanceId: null }), send: async () => { throw new Error('Fixture cannot send'); }, receipt: async () => null }, { ledger: new EddaWorkflowLedger(eddaRunner(executable)), locks: new WorkflowLocks(join(root, 'locks')) });
for (const id of ['source', 'destination']) { const state = await manager.works.continuationSnapshot(id); await manager.works.act(id, { actionId: randomUUID(), revision: state.view.revision, kind: 'initialize', nextStep: '檢查保存的產物，再接續原工作' }); }
await manager.start(); const token = randomBytes(32).toString('hex');
const gateway = await serve(manager, token, { port: 0, onStop: () => { void (async () => { await manager.stop(); await gateway.close(); store.close(); })(); } });
writeFileSync(join(root, 'launch.json'), JSON.stringify({ origin: gateway.origin, url: `${gateway.origin}/#token=${token}`, token, root }), { mode: 0o600 });
console.log(JSON.stringify({ root, origin: gateway.origin, launchFile: join(root, 'launch.json') }));
