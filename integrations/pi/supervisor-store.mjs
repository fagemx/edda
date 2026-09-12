import { mkdirSync, existsSync, lstatSync, realpathSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { inside } from './managed-store.mjs';
import { privateRoot, validateId, digest, readJson } from './store.mjs';
import { taskId } from './compose-sources.mjs';

export function supervisorDir(root, id, create = false) {
  const base = join(resolve(root), 'supervisors'), dir = join(base, validateId(id));
  if (create) privateRoot(root);
  for (const path of [base, dir]) {
    if (create) mkdirSync(path, { recursive: true, mode: 0o700 });
    if (existsSync(path) && (!lstatSync(path).isDirectory() || lstatSync(path).isSymbolicLink())) throw new Error('Supervisor directory must not be a link');
  }
  return dir;
}
function text(value, max, name) {
  if (typeof value !== 'string' || !value.trim() || value.length > max) throw new Error(`Invalid ${name}`);
  return value;
}
function keys(value, allowed, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).some((k) => !allowed.includes(k))) throw new Error(`Invalid ${name} fields`);
}
function profile(value) {
  keys(value, ['piEntry', 'provider', 'model', 'thinking', 'agentDir', 'extensions'], 'Pi profile');
  const result = { provider: text(value.provider, 100, 'provider'), model: text(value.model, 200, 'model') };
  if (value.thinking !== undefined) {
    if (!['off', 'minimal', 'low', 'medium', 'high', 'xhigh'].includes(value.thinking)) throw new Error('Invalid thinking level');
    result.thinking = value.thinking;
  }
  for (const name of ['piEntry', 'agentDir']) if (value[name]) result[name] = realpathSync(resolve(value[name]));
  if (value.extensions) {
    if (!Array.isArray(value.extensions) || value.extensions.length > 6) throw new Error('Too many profile extensions');
    result.extensions = value.extensions.map((p) => realpathSync(resolve(p)));
  }
  return result;
}
export function normalizeSupervisor(value) {
  keys(value, ['version', 'id', 'project', 'authority', 'tasks', 'worker', 'manager', 'maxDecisions', 'maxResponses', 'pollMs'], 'supervisor');
  if (value.version !== 1) throw new Error('Unsupported supervisor version');
  const id = validateId(value.id || randomUUID()), project = realpathSync(resolve(value.project));
  keys(value.authority, ['instruction', 'source'], 'authority');
  keys(value.authority.source, ['uri', 'revision'], 'authority source');
  const authority = { instruction: text(value.authority.instruction, 6000, 'operator instruction'), source: {
    uri: text(value.authority.source.uri, 500, 'authority URI'), revision: text(value.authority.source.revision, 200, 'authority revision') } };
  if (Buffer.byteLength(authority.instruction) > 6000) throw new Error('Operator instruction exceeds 6000 UTF-8 bytes');
  if (!Array.isArray(value.tasks) || value.tasks.length < 1 || value.tasks.length > 2) throw new Error('Select one or two existing tasks');
  const tasks = value.tasks.map((t) => {
    keys(t, ['id', 'cwd', 'runId'], 'task binding');
    const cwd = realpathSync(resolve(t.cwd));
    if (!lstatSync(cwd).isDirectory()) throw new Error('Worker cwd must be a directory');
    return { id: taskId(t.id), cwd, ...(t.runId ? { runId: validateId(t.runId) } : {}) };
  });
  if (new Set(tasks.map((t) => t.id)).size !== tasks.length || new Set(tasks.map((t) => t.cwd)).size !== tasks.length) throw new Error('Tasks and worker directories must be distinct');
  const gitPath = (cwd, arg) => {
    try { return realpathSync(execFileSync('git', ['-C', cwd, 'rev-parse', '--path-format=absolute', arg], { windowsHide: true, timeout: 5000, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim()); }
    catch { return null; }
  };
  const common = gitPath(project, '--git-common-dir');
  if (common) {
    const gitDirs = [];
    for (const task of tasks) {
      if (gitPath(task.cwd, '--git-common-dir') !== common) throw new Error('Worker worktree must belong to the selected project repository');
      gitDirs.push(gitPath(task.cwd, '--absolute-git-dir'));
    }
    if (new Set(gitDirs).size !== tasks.length) throw new Error('Parallel repository workers need distinct worktrees');
  } else if (tasks.some((t) => !inside(project, t.cwd))) throw new Error('Non-Git worker directories must stay within the selected project');
  const maxDecisions = value.maxDecisions ?? 10, maxResponses = value.maxResponses ?? 10, pollMs = value.pollMs ?? 2000;
  if (![maxDecisions, maxResponses].every((n) => Number.isInteger(n) && n >= 1 && n <= 50)) throw new Error('Manager decision/response limits must be 1..50');
  if (!Number.isInteger(pollMs) || pollMs < 1000 || pollMs > 60000) throw new Error('pollMs must be 1000..60000');
  const result = { version: 1, id, project, authority, tasks, worker: profile(value.worker), manager: profile(value.manager || value.worker), maxDecisions, maxResponses, pollMs };
  if (Buffer.byteLength(JSON.stringify(result)) > 24000) throw new Error('Supervisor configuration exceeds 24 KiB');
  return result;
}
export function loadSupervisor(root, id) {
  const dir = supervisorDir(root, id), config = readJson(join(dir, 'config.json'));
  if (!config || config.id !== id || digest(JSON.stringify(config)) !== readJson(join(dir, 'identity.json'))?.digest) throw new Error('Supervisor config identity mismatch');
  return { dir, config };
}
export function pauseToken(dir) { return readJson(join(dir, 'pause.json'))?.nonce || null; }
