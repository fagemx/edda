import { spawn } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { closeSync, existsSync, lstatSync, openSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { DatabaseSync } from 'node:sqlite';
import { AgentManager } from './manager.js';
import { ManagerStore } from './store.js';
import { ChannelAdapter, defaultPiRoot, secureRoot } from './pi-adapter.js';
import { hash, loadConfig, object, parseConfig, text } from './config.js';
import { serve } from './http.js';
import type { AgentBinding, ManagerConfig, ProjectView } from './contracts.js';
import { RuntimeAdapter } from './runtime-adapter.js';

interface Owner { version: 1; pid: number; instanceId: string; origin: string; token: string; configDigest: string; startedAt: string }
interface Lock { pid: number; instanceId: string }
const [command = 'help', ...args] = process.argv.slice(2), flags = new Map<string, string>();
for (let i = 0; i < args.length; i += 2) {
  const name = args[i], value = args[i + 1];
  if (!name?.startsWith('--') || !value || flags.has(name)) throw new Error('Flags require one unique value each');
  flags.set(name, value);
}
for (const name of flags.keys()) if (!['--root', '--config', '--port', '--pi-root', '--from-delegation', '--instance'].includes(name)) throw new Error(`Unknown option ${name}`);
const root = resolve(flags.get('--root') || join(homedir(), '.edda-agent-manager'));
const configFile = resolve(flags.get('--config') || join(root, 'config.json'));
const piRoot = resolve(flags.get('--pi-root') || defaultPiRoot());
const port = Number(flags.get('--port') ?? 4390);
if (!Number.isSafeInteger(port) || port < 0 || port > 65535) throw new Error('Port must be 0..65535');
const ownerPath = join(root, 'owner.json'), lockPath = join(root, 'service.lock');
function read(file: string): unknown {
  const stat = lstatSync(file);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 262144) throw new Error('Invalid bounded configuration record');
  return JSON.parse(readFileSync(file, 'utf8')) as unknown;
}
function owner(): Owner {
  const value = object(read(ownerPath));
  if (value.version !== 1 || typeof value.origin !== 'string' || !/^http:\/\/127\.0\.0\.1:[0-9]+$/.test(value.origin) ||
    typeof value.token !== 'string' || !/^[a-f0-9]{64}$/.test(value.token) || !Number.isSafeInteger(value.pid) || Number(value.pid) <= 0 || typeof value.instanceId !== 'string') throw new Error('Invalid service owner');
  return value as unknown as Owner;
}
function alive(pid: number): boolean { try { process.kill(pid, 0); return true; } catch (e) { if ((e as NodeJS.ErrnoException).code === 'ESRCH') return false; throw e; } }
// SQLite holds the cross-process lifecycle mutex and the OS releases it on crash.
// A second filesystem lock would itself need racy stale-lock recovery. Never
// remove this database: its stable identity serializes every lock/owner mutation.
function lifecycle<T>(action: () => T): T {
  const file = join(root, 'lifecycle.sqlite');
  for (const path of [file, file + '-journal', file + '-wal', file + '-shm']) {
    if (existsSync(path) && (!lstatSync(path).isFile() || lstatSync(path).isSymbolicLink())) throw new Error('Lifecycle storage must not be a link');
  }
  const db = new DatabaseSync(file);
  try {
    db.exec('PRAGMA busy_timeout=5000; BEGIN IMMEDIATE');
    const result = action();
    db.exec('COMMIT'); return result;
  } finally { db.close(); }
}
function lock(): Lock {
  const value = object(read(lockPath));
  if (!Number.isSafeInteger(value.pid) || Number(value.pid) <= 0 || typeof value.instanceId !== 'string' || !value.instanceId) throw new Error('Invalid service lock');
  return value as unknown as Lock;
}
function matchingOwner(held: Lock): Owner | null {
  if (!existsSync(ownerPath)) return null;
  const current = owner();
  if (current.instanceId !== held.instanceId || current.pid !== held.pid) throw new Error('Owner generation mismatch');
  return current;
}
// Called only inside lifecycle(). An absent owner is allowed for a crash before
// readiness; an existing owner must identify this exact lock generation and PID.
function reclaim(held: Lock): void {
  const current = matchingOwner(held);
  if (alive(held.pid)) throw new Error('Cannot prove service owner is dead; no recovery performed');
  if (current) unlinkSync(ownerPath);
  unlinkSync(lockPath);
}
async function service(record: Owner): Promise<unknown> {
  const response = await fetch(`${record.origin}/api/service`, { headers: { authorization: `Bearer ${record.token}` }, signal: AbortSignal.timeout(2000), redirect: 'error' });
  if (!response.ok) throw new Error('Service authentication failed');
  const value = object(await response.json() as unknown);
  if (value.startedAt !== record.startedAt) throw new Error('Service identity changed');
  return value;
}
function fromDelegation(file: string): ManagerConfig {
  const input = object(read(file)), base = dirname(file), projects: ProjectView[] = [], agents: AgentBinding[] = [];
  for (const id of ['character', 'edda']) {
    const source = object(input[id]), name = id === 'character' ? 'Character' : 'Edda';
    const project: ProjectView = { id, name, priority: id === 'character' ? 0 : 10, resources: [] };
    if (typeof source.servicesFile === 'string') {
      const services = object(read(resolve(base, source.servicesFile))), pg = object(services.postgres);
      if (pg.container && pg.testDatabase) project.resources.push({ id: 'postgres', name: '共用 PostgreSQL', kind: 'database',
        details: `${text(pg.host)}:${Number(pg.port)} / ${text(pg.testDatabase)}\n容器：${text(pg.container)}\n資料卷：${text(pg.dataVolume)}`,
        owner: text(pg.lifecycleOwner, 500), source: '交接服務登記；未進行即時健康探測' });
    }
    projects.push(project);
    agents.push({ id, name: `${name} ${id === 'character' ? 'Sol' : 'Flash'} 負責人`, role: 'manager', projectId: id,
      registryRoot: text(source.registryRoot, 4096), workspace: text(source.worktree, 4096), sessionId: text(source.sessionId),
      runId: text(source.runId), summaryFile: typeof source.statusFile === 'string' ? resolve(base, source.statusFile) : null });
  }
  return parseConfig({ version: 1, projects, agents, refreshMs: 3000 });
}
async function main(): Promise<void> {
  if (command === 'help') {
    console.log('Agent manager (Node24)\n  init --from-delegation INDEX --root PRIVATE_DIR\n  start|serve --root PRIVATE_DIR [--config FILE] [--port 4390] [--pi-root DIR]\n  status|url|stop|recover --root PRIVATE_DIR\nPrivate launch URL contains the gateway credential. Existing agents are never started or stopped by these commands.'); return;
  }
  if (command === 'init') {
    const input = flags.get('--from-delegation'); if (!input) throw new Error('init requires --from-delegation');
    await secureRoot(root, piRoot);
    const config = fromDelegation(resolve(input));
    writeFileSync(configFile, JSON.stringify(config, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
    console.log(JSON.stringify({ configFile, selectedAgents: config.agents.map((a) => a.id), agentsStarted: false })); return;
  }
  if (command === 'recover') {
    await secureRoot(root, piRoot);
    lifecycle(() => reclaim(lock()));
    console.log(JSON.stringify({ recovered: true, agentsChanged: false, operationsReplayed: false })); return;
  }
  if (['status', 'url', 'stop'].includes(command)) {
    const current = owner(), info = await service(current);
    if (command === 'url') console.log(`${current.origin}/#token=${current.token}`);
    else if (command === 'status') console.log(JSON.stringify({ ...object(info), origin: current.origin, pid: current.pid, instanceId: current.instanceId }));
    else {
      const response = await fetch(`${current.origin}/api/service/stop`, { method: 'POST', headers: { authorization: `Bearer ${current.token}`, 'content-type': 'application/json' }, body: '{}', signal: AbortSignal.timeout(5000) });
      if (!response.ok) throw new Error('Service stop failed');
      console.log(JSON.stringify(await response.json()));
    }
    return;
  }
  if (command !== 'start' && command !== 'serve') throw new Error('Unknown command');
  const config = loadConfig(configFile), configDigest = hash(JSON.stringify(config));
  await secureRoot(root, piRoot);
  if (command === 'start') {
    let recovered = false;
    const pending = lifecycle(() => {
      if (existsSync(lockPath)) {
        const held = lock(); matchingOwner(held);
        if (alive(held.pid)) return true;
        reclaim(held); recovered = true;
      } else if (existsSync(ownerPath)) throw new Error('Owner exists without a service lock; no recovery performed');
      return false;
    });
    const instanceId = randomUUID();
    let child: ReturnType<typeof spawn> | undefined, spawnError: Error | null = null;
    if (!pending) {
      const fd = openSync(join(root, 'service.log'), 'a', 0o600);
      try {
        child = spawn(process.execPath, [fileURLToPath(import.meta.url), 'serve', '--root', root, '--config', configFile, '--port', String(port), '--pi-root', piRoot, '--instance', instanceId],
          { cwd: root, windowsHide: true, detached: true, stdio: ['ignore', fd, fd] });
        child.on('error', (e) => { spawnError = e; }); child.unref();
      } finally { closeSync(fd); }
    }
    for (let i = 0; i < 80; i++) {
      if (spawnError) throw spawnError;
      const state = lifecycle(() => {
        if (!existsSync(lockPath)) return null;
        const held = lock(); return { current: matchingOwner(held), live: alive(held.pid) };
      });
      if (state?.current) {
        const current = state.current;
        if (!state.live) throw new Error('Service exited before readiness; inspect private service.log');
        await service(current);
        if (current.configDigest !== configDigest) throw new Error('Service is running with another configuration; stop it before changing selections');
        console.log(JSON.stringify({ ...(current.instanceId === instanceId ? { started: true } : { reused: true }), recovered,
          origin: current.origin, url: `${current.origin}/#token=${current.token}`, pid: current.pid })); return;
      }
      if (!state?.live && (pending || (child && (child.exitCode !== null || child.signalCode !== null)))) throw new Error('Service exited before readiness; inspect private service.log');
      await delay(250);
    }
    throw new Error('Startup is not confirmed; inspect private service.log and status before another start');
  }
  const instanceId = flags.get('--instance') || randomUUID();
  lifecycle(() => {
    if (existsSync(lockPath)) reclaim(lock());
    else if (existsSync(ownerPath)) throw new Error('Owner exists without a service lock; no recovery performed');
    writeFileSync(lockPath, JSON.stringify({ pid: process.pid, instanceId }), { flag: 'wx', mode: 0o600 });
  });
  let store: ManagerStore | undefined, manager: AgentManager | undefined, gateway: Awaited<ReturnType<typeof serve>> | undefined, closing = false;
  const shutdown = async (): Promise<void> => {
    if (closing) return; closing = true;
    await manager?.stop(); await gateway?.close(); store?.close();
    lifecycle(() => {
      if (!existsSync(lockPath)) return;
      const held = lock();
      if (held.instanceId !== instanceId || held.pid !== process.pid) return;
      if (matchingOwner(held)) unlinkSync(ownerPath);
      unlinkSync(lockPath);
    });
  };
  try {
    store = new ManagerStore(root); manager = new AgentManager(config, store, await RuntimeAdapter.create(store, piRoot));
    await manager.start();
    const token = randomBytes(32).toString('hex');
    gateway = await serve(manager, token, { port, onStop: () => { void shutdown(); } });
    const record: Owner = { version: 1, pid: process.pid, instanceId, origin: gateway.origin, token, configDigest, startedAt: manager.startedAt };
    lifecycle(() => {
      const held = lock();
      if (held.instanceId !== instanceId || held.pid !== process.pid) throw new Error('Owner generation mismatch');
      writeFileSync(ownerPath, JSON.stringify(record), { flag: 'wx', mode: 0o600 });
    });
    process.once('SIGINT', () => { void shutdown(); }); process.once('SIGTERM', () => { void shutdown(); });
    console.log(`Agent manager: ${gateway.origin}/#token=${token}`);
  } catch (error) { await shutdown(); throw error; }
}
void main().catch((error: unknown) => { console.error(error instanceof Error ? error.message : 'Manager operation failed'); process.exitCode = 1; });
