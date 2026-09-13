// Read-only entry/discovery. Managed config/state remain the lifecycle records.
import { readdirSync, lstatSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defaultRoot, readJson, validateId } from './store.mjs';
import { findPiEntry, managedDir } from './managed-store.mjs';

const packageRoot = dirname(fileURLToPath(import.meta.url));
export function runtimeInfo(root = defaultRoot()) {
  let pi = null;
  try { pi = findPiEntry(); } catch { /* Missing optional runtime is a diagnostic, not activation. */ }
  const pkg = JSON.parse(readFileSync(join(packageRoot, 'package.json'), 'utf8'));
  return { status: pi ? 'available' : 'needs_pi', version: pkg.version, nodeVersion: process.versions.node,
    cli: join(packageRoot, 'cli.mjs'), guide: join(packageRoot, 'getting-started.md'), registryRoot: resolve(root), pi,
    capabilities: { managedLaunch: true, sameSessionResume: true, explicitMessages: true, persistedManagedConversation: true,
      selectedTaskNotifications: true, managedFork: false, automaticProcessRestart: false, automaticOwnerWake: false },
    notice: 'Installed client capabilities only. Inspect run-status for the runtime actually loaded by a run. No session was started or upgraded.',
    nextStep: pi ? 'Read the installed guide; runs discovers existing managed runs without starting them.' : 'Install Pi or set EDDA_PI_ENTRY to its dist/bundle/cli.js, then run runtime-info again.' };
}

const phases = new Set(['launch_requested', 'starting', 'ready', 'stopping', 'stopped', 'failed', 'exited']);
const time = (v) => typeof v === 'string' && /^\d{4}-\d\d-\d\dT/.test(v) && Number.isFinite(Date.parse(v)) ? v : null;
export function listManagedRuns(root, { limit = 50, after } = {}) {
  limit = Number(limit);
  if (!Number.isInteger(limit) || limit < 1 || limit > 100) throw new Error('Run limit must be 1..100');
  if (after !== undefined) after = validateId(after);
  root = resolve(root);
  const base = join(root, 'managed');
  let names;
  try {
    const info = lstatSync(base);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error('Managed directory must not be a link');
    names = readdirSync(base).filter((name) => /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(name)).sort();
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    names = [];
  }
  const selected = names.filter((name) => !after || name > after), page = selected.slice(0, limit);
  const runs = page.map((runId) => {
    try {
      const dir = managedDir(root, runId), config = readJson(join(dir, 'config.json')), state = readJson(join(dir, 'state.json'));
      if (!config || config.version !== 1 || config.runId !== runId || config.root !== root || typeof config.project !== 'string' || config.project.length > 4096 ||
          (state && state.runId !== runId)) throw new Error('Invalid run records');
      // Never spread records: prompts, tokens and raw provider errors are not discovery data.
      return { runId, project: config.project, sessionId: state?.sessionId ? validateId(state.sessionId) : null,
        recordedPhase: phases.has(state?.phase) ? state.phase : 'unknown', observedLive: null,
        updatedAt: time(state?.updatedAt), releaseId: /^[a-f0-9]{64}$/.test(config.release?.id) ? config.release.id : null,
        error: null };
    } catch {
      return { runId, project: null, sessionId: null, recordedPhase: 'unknown', observedLive: null,
        updatedAt: null, releaseId: null, error: 'Run records unavailable or invalid; preserved without recovery.' };
    }
  });
  return { status: 'recorded_runs', registryRoot: root, runs, hasMore: selected.length > page.length,
    nextAfter: selected.length > page.length ? page.at(-1) : null,
    notice: 'Recorded inventory, not live health or completion. Use run-status RUN_ID. Only this registry is searched; no automatic restart or repair.' };
}
