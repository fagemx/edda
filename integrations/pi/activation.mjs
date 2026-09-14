// Read-only entry/discovery. Managed config/state remain the lifecycle records.
import { readdirSync, lstatSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defaultRoot, digest, readRecord, validateId } from './store.mjs';
import { findPiEntry, managedDir, verifyRelease, continuityMode } from './managed-store.mjs';
export { RECEIPT_VERSION, releaseIdentity, activationReceipt, evaluateCoherence } from './activation-receipt.mjs';

const packageRoot = dirname(fileURLToPath(import.meta.url));

// Digest-verified installed runtime releases, bounded and in descending id order
// (the id is a content digest, not a timestamp). Read-only: `runtime-info` must
// not create anything (the activation test asserts the root stays empty), so a
// missing releases directory is simply empty.
function verifiedReleases(root, limit = 5) {
  const base = join(resolve(root), 'releases');
  let names;
  try { names = readdirSync(base).filter((name) => /^[0-9a-f]{64}$/.test(name)); }
  catch (error) { if (error.code === 'ENOENT') return { releases: [], hasMore: false }; throw error; }
  const releases = names.sort((a, b) => (a < b ? 1 : -1)).slice(0, limit).map((id) => {
    const path = join(base, id);
    let version = null, verified = false;
    try {
      verifyRelease({ id, path });
      const pkg = readRecord(join(path, 'package.json')).value;
      version = typeof pkg?.version === 'string' ? pkg.version : null;
      verified = true;
    } catch { /* an unverified release keeps its id and paths so drift stays visible */ }
    return { id, version, channel: join(path, 'channel.mjs'), extension: join(path, 'extension.mjs'), verified };
  });
  return { releases, hasMore: names.length > limit };
}

export function runtimeInfo(root = defaultRoot()) {
  let pi = null;
  try { pi = findPiEntry(); } catch { /* Missing optional runtime is a diagnostic, not activation. */ }
  const pkg = JSON.parse(readFileSync(join(packageRoot, 'package.json'), 'utf8'));
  const extension = join(packageRoot, 'extension.mjs'), channel = join(packageRoot, 'channel.mjs');
  const installed = verifiedReleases(root);
  return { status: pi ? 'available' : 'needs_pi', version: pkg.version, nodeVersion: process.versions.node,
    cli: join(packageRoot, 'cli.mjs'), guide: join(packageRoot, 'getting-started.md'), registryRoot: resolve(root), pi,
    extension: { path: extension, version: pkg.version, digest: digest(readFileSync(extension)) },
    // The module a live session reports as `integration.modulePath` is the channel
    // module, so drift is checked against `channel`, not `extension`.
    channel: { path: channel, version: pkg.version, digest: digest(readFileSync(channel)) },
    handOpened: { argv: ['pi', '-e', extension], load: `pi -e "${extension}"`, ownerRef: 'EDDA_OWNER_REF=<owner-reference>',
      notice: 'A session Edda did not launch must load this installed extension (handOpened.argv) and declare EDDA_OWNER_REF. A live session reports its loaded channel as integration.modulePath in edda-pi list; if it matches neither runtime-info.channel.path nor a verified runtime-info.releases[].channel, it loaded a stale copy and owner-mailbox pickup fails silently.' },
    releases: installed.releases, releasesHasMore: installed.hasMore,
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
    let config = null, configError = null, configOk = false;
    try {
      const dir = managedDir(root, runId);
      const configRecord = readRecord(join(dir, 'config.json'));
      configError = configRecord.error;
      config = configRecord.value;
      configOk = Boolean(config) && !configError && config.version === 1 && config.runId === runId && config.root === root &&
        typeof config.project === 'string' && config.project.length <= 4096;
      if (!configOk) throw new Error('Invalid run records');
      const stateRecord = readRecord(join(dir, 'state.json')), state = stateRecord.value;
      if (state && state.runId !== runId) throw new Error('Invalid run records');
      // Never spread records: prompts, tokens and raw provider errors are not discovery data.
      // A valid config.json keeps project/releaseId even when state.json is unreadable.
      return { runId, project: config.project, sessionId: state?.sessionId ? validateId(state.sessionId) : null,
        recordedPhase: phases.has(state?.phase) ? state.phase : 'unknown', observedLive: null,
        updatedAt: time(state?.updatedAt), releaseId: /^[a-f0-9]{64}$/.test(config.release?.id) ? config.release.id : null,
        continuity: continuityMode(state?.owner ?? config.owner), error: stateRecord.error ? { ...stateRecord.error } : null };
    } catch {
      // Preserve any validated config identity even when the row degrades, and
      // attribute the failure to the record that actually failed.
      return { runId, project: configOk ? config.project : null, sessionId: null, recordedPhase: 'unknown', observedLive: null, updatedAt: null,
        releaseId: configOk && /^[a-f0-9]{64}$/.test(config.release?.id) ? config.release.id : null,
        continuity: continuityMode(configOk ? config.owner : null),
        error: configError ? { ...configError }
          : configOk ? { code: 'record_invalid', record: 'state.json', message: 'State record invalid; run identity preserved without recovery.' }
          : { code: 'record_invalid', record: 'config.json', message: 'Run records unavailable or invalid; preserved without recovery.' } };
    }
  });
  return { status: 'recorded_runs', registryRoot: root, runs, hasMore: selected.length > page.length,
    nextAfter: selected.length > page.length ? page.at(-1) : null,
    notice: 'Recorded inventory, not live health or completion. Use run-status RUN_ID. Only this registry is searched; no automatic restart or repair.' };
}
