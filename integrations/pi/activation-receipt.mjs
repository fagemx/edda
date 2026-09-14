// Read-only activation receipt: one bounded measurement of the four revisions
// that can drift apart independently — the repository merge revision, the
// installed shipping `edda` binary, the installed Pi package content, and the
// running agent-manager service — plus a fail-closed coherence verdict.
//
// This module is strictly read-only: it never writes, creates, renames or
// deletes any file or directory, and never starts, stops or restarts a service
// or agent process. Its only external reads are bounded one-shot `git` /
// `edda --version` probes and the authenticated `GET /api/service` health
// check. It never prints or returns the agent-manager owner token.
import { lstatSync, readdirSync, readFileSync, realpathSync, statSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { defaultRoot, digest, readRecord } from './store.mjs';

export const RECEIPT_VERSION = 1;
const MAX_SOURCE_FILE_BYTES = 1024 * 1024;
const HEX64 = /^[0-9a-f]{64}$/;
const RUN_ID = /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const BARE_VERSION = /^(\S+)\s+\(([0-9a-f]{7,40})(-dirty)?\s+(\d{4}-\d\d-\d\d)\)$/;
// The shipped binary prints `edda 0.6.1 (<rev>[-dirty] <date>)`; a bare
// `<version> (<rev>[-dirty] <date>)` line is accepted too so the parser is not
// coupled to the leading program name.
const NAMED_VERSION = /^(\S+)\s+(\S+)\s+\(([0-9a-f]{7,40})(-dirty)?\s+(\d{4}-\d\d-\d\d)\)$/;

function shortError(error) {
  const message = error && error.message ? error.message : String(error);
  return message.replace(/\s+/g, ' ').trim().slice(0, 200) || 'unavailable';
}
const strOrNull = (value) => (typeof value === 'string' && value.length ? value : null);

function parseVersionLine(line) {
  const named = NAMED_VERSION.exec(line);
  if (named) return { version: named[2], revision: named[3], dirtyBuild: Boolean(named[4]), builtAt: named[5] };
  const bare = BARE_VERSION.exec(line);
  if (bare) return { version: bare[1], revision: bare[2], dirtyBuild: Boolean(bare[3]), builtAt: bare[4] };
  return null;
}

function pidAlive(pid) {
  if (!Number.isSafeInteger(pid) || pid <= 0) return false;
  try { process.kill(pid, 0); return true; }
  catch (error) { return error.code !== 'ESRCH'; }
}

// Same directory, allowing for Windows case and for junctions/symlinks.
function samePath(left, right) {
  const normalize = (value) => {
    let resolved = resolve(value);
    try { resolved = realpathSync.native(resolved); } catch { /* keep the resolved path */ }
    return process.platform === 'win32' ? resolved.toLowerCase() : resolved;
  };
  return normalize(left) === normalize(right);
}

// Read-only content identity of a runtime source directory. File selection and
// digest rules are intentionally identical to managed-store.installRuntime()
// so `releaseIdentity(dir).id === installRuntime(root, dir).id`; unlike the
// installer this function writes nothing and creates no registry.
export function releaseIdentity(source) {
  const dir = resolve(source);
  const names = readdirSync(dir).filter((name) => name === 'package.json' || name === 'getting-started.md' || name.endsWith('.ps1') ||
    (name.endsWith('.mjs') && !name.endsWith('.test.mjs') && !name.includes('smoke'))).sort();
  const files = {}, contents = new Map();
  for (const name of names) {
    const path = join(dir, name), stat = lstatSync(path);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > MAX_SOURCE_FILE_BYTES) throw new Error('Invalid runtime source file');
    const content = readFileSync(path);
    files[name] = digest(content);
    contents.set(name, content);
  }
  if (!files['managed-runner.mjs'] || !files['extension.mjs'] || !files['package.json']) throw new Error('Incomplete runtime source');
  return { id: digest(JSON.stringify(files)), version: JSON.parse(contents.get('package.json').toString()).version, files };
}

// Pure fail-closed evaluation over already-collected legs. Exported so the
// decision logic is provable without a live environment.
//
// `observed` alone is not enough to call a leg consistent: a leg that was read
// but is missing the input its comparison needs (a checkout without
// `integrations/pi`, a manager root without `release.json`) cannot be verified,
// and must never let the verdict reach `coherent`.
export function evaluateCoherence(legs = {}) {
  const repository = legs.repository || {}, edda = legs.edda || {}, pi = legs.pi || {}, manager = legs.manager || {};
  const findings = [];
  const requiredLegs = 4;
  const repositoryObserved = Boolean(repository.observed && repository.headRevision);
  const eddaObserved = Boolean(edda.observed && edda.revision);
  const piObserved = Boolean(pi.observed && pi.installedReleaseId && pi.repoReleaseId);
  const managerObserved = Boolean(manager.observed);
  if (repositoryObserved && eddaObserved && !repository.headRevision.startsWith(edda.revision)) {
    findings.push({ leg: 'edda', code: 'edda_revision_mismatch',
      detail: `installed edda ${edda.revision} does not match repository ${repository.headRevision}` });
  }
  if (piObserved && pi.installedReleaseId !== pi.repoReleaseId) {
    findings.push({ leg: 'pi', code: 'pi_content_drift',
      detail: `installed ${pi.installedReleaseId} does not match repository ${pi.repoReleaseId}` });
  }
  if (managerObserved) {
    // A running manager with no release metadata is the exact gap this receipt
    // exists to expose: its configured revision cannot be verified.
    if (!manager.configured) {
      findings.push({ leg: 'manager', code: 'manager_configured_revision_unknown',
        detail: 'agent-manager root has no release.json; the configured revision cannot be verified' });
    } else if (repository.headRevision && manager.configured.headSha !== repository.headRevision) {
      findings.push({ leg: 'manager', code: 'manager_configured_revision_mismatch',
        detail: `manager configured ${manager.configured.headSha ?? 'unknown'} does not match repository ${repository.headRevision}` });
    }
    if (manager.health !== 'ok') findings.push({ leg: 'manager', code: 'manager_not_healthy', detail: manager.health || 'unknown' });
    if (!manager.running) findings.push({ leg: 'manager', code: 'manager_absent_owner', detail: 'no running agent-manager owner record' });
  }
  const observedLegs = [repositoryObserved, eddaObserved, piObserved, managerObserved].filter(Boolean).length;
  const status = observedLegs === 0 ? 'unknown'
    : findings.length > 0 ? 'drift'
      : observedLegs < requiredLegs ? 'partial'
        : 'coherent';
  return { status, findings, observedLegs, requiredLegs };
}

function defaultRunGit(timeoutMs) {
  return (args) => execFileSync('git', args, { encoding: 'utf8', timeout: timeoutMs, windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: MAX_SOURCE_FILE_BYTES,
    // `git status` may otherwise rewrite the index stat cache; optional locks off
    // keeps the probe to read-only file access.
    env: { ...process.env, GIT_OPTIONAL_LOCKS: '0' } }).trim();
}

function defaultRunVersion(timeoutMs) {
  return (binary) => {
    const options = { encoding: 'utf8', timeout: timeoutMs, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: MAX_SOURCE_FILE_BYTES };
    // A `.mjs`/`.cjs`/`.js` command is run through the current Node so the
    // receipt is testable without a fabricated native binary everywhere.
    if (typeof binary === 'string' && /\.(?:mjs|cjs|js)$/i.test(binary)) {
      return execFileSync(process.execPath, [binary, '--version'], options).trim();
    }
    return execFileSync(binary, ['--version'], options).trim();
  };
}

function repositoryLeg(repo, runGit) {
  const root = typeof repo === 'string' && repo ? resolve(repo) : process.cwd();
  const leg = { observed: false, root, headRevision: null, headRef: null, dirty: null, piPackagingRevision: null, error: null };
  try {
    const head = String(runGit(['-C', root, 'rev-parse', 'HEAD']) ?? '').trim();
    if (!/^[0-9a-f]{40}$/.test(head)) throw new Error('Not a git checkout');
    leg.headRevision = head;
    leg.observed = true;
    try { leg.headRef = String(runGit(['-C', root, 'symbolic-ref', '-q', 'HEAD']) ?? '').trim() || null; }
    catch { leg.headRef = null; }
    try { leg.dirty = String(runGit(['-C', root, 'status', '--porcelain', '-uno']) ?? '').trim().length > 0; }
    catch { leg.dirty = null; }
    try {
      const tree = String(runGit(['-C', root, 'rev-parse', 'HEAD:integrations/pi']) ?? '').trim();
      leg.piPackagingRevision = /^[0-9a-f]{40}$/.test(tree) ? tree : null;
    } catch { leg.piPackagingRevision = null; }
  } catch (error) { leg.error = shortError(error); }
  return leg;
}

function eddaLeg(binary, runVersion) {
  const leg = { observed: false, binary, version: null, revision: null, dirtyBuild: null, builtAt: null, raw: null, error: null };
  try {
    const stdout = String(runVersion(binary) ?? '');
    const line = stdout.split(/\r?\n/).map((value) => value.trim()).find((value) => value.length > 0) || '';
    const parsed = parseVersionLine(line);
    if (!parsed) throw new Error('Unexpected edda --version output');
    Object.assign(leg, parsed, { raw: line, observed: true });
  } catch (error) { leg.error = shortError(error); }
  return leg;
}

// Where the *installed* client lives. The running module is the installed
// package when the receipt is invoked as `edda-pi`, but it is the source tree
// when invoked as `node integrations/pi/activation-receipt.mjs`. Measuring the
// source tree as "installed" would make the Pi leg compare a directory to
// itself, so resolve the global install explicitly and report the source used.
// `env`/`execPath` are injectable so the candidate list is testable.
export function clientRootCandidates(env = process.env, execPath = process.execPath) {
  const packagePath = '@edda/pi-session-channel';
  const execDir = dirname(execPath);
  const candidates = [
    [join(execDir, 'node_modules', packagePath), 'global'],
    [join(execDir, '..', 'lib', 'node_modules', packagePath), 'global'],
  ];
  // npm's global prefix without spawning npm: the configured prefix when present,
  // then the platform defaults. A standard Windows install is %APPDATA%\npm, which
  // is neither beside node.exe nor under ../lib (GH #1220).
  if (env.npm_config_prefix) candidates.push([join(env.npm_config_prefix, 'node_modules', packagePath), 'global']);
  if (env.APPDATA) candidates.push([join(env.APPDATA, 'npm', 'node_modules', packagePath), 'global']);
  if (env.HOME) candidates.push([join(env.HOME, '.npm-global', 'lib', 'node_modules', packagePath), 'global']);
  return candidates;
}

export function resolveClientRoot(explicit, env = process.env, execPath = process.execPath) {
  if (typeof explicit === 'string' && explicit) return { root: resolve(explicit), source: 'explicit' };
  if (typeof env.EDDA_PI_PACKAGE_ROOT === 'string' && env.EDDA_PI_PACKAGE_ROOT) {
    return { root: resolve(env.EDDA_PI_PACKAGE_ROOT), source: 'env' };
  }
  for (const [candidate, source] of clientRootCandidates(env, execPath)) {
    try {
      const info = lstatSync(candidate);
      if (info.isDirectory() && !info.isSymbolicLink()) return { root: resolve(candidate), source };
    } catch { /* absent candidate is not an error */ }
  }
  return { root: dirname(fileURLToPath(import.meta.url)), source: 'module' };
}

function piLeg(registryRoot, repo, clientRoot) {
  const moduleDir = dirname(fileURLToPath(import.meta.url));
  const client = resolveClientRoot(clientRoot);
  const root = resolve(registryRoot);
  const leg = { observed: false, clientPath: join(client.root, 'cli.mjs'), clientSource: client.source,
    modulePath: join(moduleDir, 'activation-receipt.mjs'), clientVersion: null,
    installedReleaseId: null, installedReleaseVersion: null, repoReleaseId: null, repoReleaseVersion: null,
    registryRoot: root, pinnedReleaseIds: [], relevantReleaseIds: [], error: null };
  const errors = [];
  const repoDir = join(resolve(repo), 'integrations', 'pi');
  // When the resolved "installed" root is the checkout itself, the Pi leg would
  // compare a directory with itself and could report false coherence. Refuse
  // that instead: the leg is unobserved and the receipt fails closed as partial.
  const selfComparison = samePath(client.root, repoDir);
  try {
    const meta = readRecord(join(client.root, 'package.json'));
    if (meta.value && typeof meta.value.version === 'string') leg.clientVersion = meta.value.version;
  } catch (error) { errors.push(shortError(error)); }
  try {
    const identity = releaseIdentity(client.root);
    if (!selfComparison) {
      leg.installedReleaseId = identity.id;
      leg.installedReleaseVersion = identity.version;
      leg.observed = true;
    }
  } catch (error) { errors.push(shortError(error)); }
  try {
    const info = lstatSync(repoDir);
    if (info.isDirectory() && !info.isSymbolicLink()) {
      const identity = releaseIdentity(repoDir);
      leg.repoReleaseId = identity.id;
      leg.repoReleaseVersion = identity.version;
    }
  } catch (error) { if (error.code !== 'ENOENT') errors.push(shortError(error)); }

  // Pinned releases: newest first. Never create the releases directory.
  try {
    const releases = join(root, 'releases'), info = lstatSync(releases);
    if (info.isDirectory() && !info.isSymbolicLink()) {
      const pinned = [];
      for (const name of readdirSync(releases)) {
        if (!HEX64.test(name)) continue;
        const dir = join(releases, name);
        let stat;
        try { stat = lstatSync(dir); } catch { continue; }
        if (!stat.isDirectory() || stat.isSymbolicLink()) continue;
        const record = readRecord(join(dir, 'release.json'));
        const manifestVersion = record.value && (typeof record.value.version === 'string' || typeof record.value.version === 'number')
          ? String(record.value.version) : null;
        // The runtime release manifest stores its own format version, not the
        // package semver; prefer the snapshotted package.json so the version
        // reported here means the same thing as `installedReleaseVersion`.
        let version = manifestVersion;
        const snapshot = readRecord(join(dir, 'package.json'));
        if (snapshot.value && typeof snapshot.value.version === 'string' && snapshot.value.version) version = snapshot.value.version;
        pinned.push({ id: name, version, path: dir, mtimeMs: stat.mtimeMs });
      }
      pinned.sort((left, right) => (right.mtimeMs - left.mtimeMs) || left.id.localeCompare(right.id));
      leg.pinnedReleaseIds = pinned.map(({ id, version, path }) => ({ id, version, path }));
    }
  } catch (error) { if (error.code !== 'ENOENT') errors.push(shortError(error)); }

  // Releases referenced by recorded runs, read per config record.
  const relevant = [], seen = new Set();
  try {
    const managed = join(root, 'managed'), info = lstatSync(managed);
    if (info.isDirectory() && !info.isSymbolicLink()) {
      for (const name of readdirSync(managed).sort()) {
        if (!RUN_ID.test(name)) continue;
        const record = readRecord(join(managed, name, 'config.json'));
        const release = record.value && typeof record.value.release === 'object' ? record.value.release : null;
        const id = release && typeof release.id === 'string' ? release.id : null;
        if (HEX64.test(id) && !seen.has(id)) { seen.add(id); relevant.push(id); }
      }
    }
  } catch (error) { if (error.code !== 'ENOENT') errors.push(shortError(error)); }
  leg.relevantReleaseIds = relevant;
  if (selfComparison) leg.error = 'installed client package was not resolved independently of the checkout; pass --client-root or set EDDA_PI_PACKAGE_ROOT';
  else if (errors.length) leg.error = errors[0];
  return leg;
}

async function managerLeg(managerRoot, timeoutMs, fetchImpl) {
  const root = typeof managerRoot === 'string' && managerRoot ? resolve(managerRoot) : resolve(join(homedir(), '.edda-agent-manager'));
  const leg = { observed: false, root, configured: null, configuredAt: null, running: null, pidAlive: null,
    health: 'absent', service: null, error: null };
  let present = false;
  try {
    const info = lstatSync(root);
    present = info.isDirectory() && !info.isSymbolicLink();
    if (!present) leg.error = 'Manager root is not a directory';
  } catch (error) { leg.error = error.code === 'ENOENT' ? 'Manager root absent' : shortError(error); }
  if (!present) { leg.health = 'absent'; return leg; }
  leg.observed = true;

  const releasePath = join(root, 'release.json'), release = readRecord(releasePath);
  if (release.value && typeof release.value === 'object' && !Array.isArray(release.value)) {
    leg.configured = { mergeCommit: strOrNull(release.value.mergeCommit), headSha: strOrNull(release.value.headSha),
      sourceWorktree: strOrNull(release.value.sourceWorktree), entrypoint: strOrNull(release.value.entrypoint) };
    if (typeof release.value.configuredAt === 'string' && release.value.configuredAt) leg.configuredAt = release.value.configuredAt;
    else { try { leg.configuredAt = statSync(releasePath).mtime.toISOString(); } catch { leg.configuredAt = null; } }
  } else if (release.error) leg.error = 'Unreadable release.json';

  const owner = readRecord(join(root, 'owner.json'));
  if (owner.value == null && owner.error == null) { leg.health = 'absent'; return leg; }
  const record = owner.value;
  const usable = record && typeof record === 'object' && Number.isSafeInteger(record.pid) && record.pid > 0 &&
    typeof record.origin === 'string' && /^https?:\/\//.test(record.origin) && typeof record.token === 'string' &&
    record.token.length > 0 && typeof record.instanceId === 'string' && typeof record.startedAt === 'string';
  if (!usable) { leg.health = 'invalid'; leg.error = leg.error || 'Invalid owner record'; return leg; }
  leg.running = { pid: record.pid, instanceId: record.instanceId, origin: record.origin, startedAt: record.startedAt };
  leg.pidAlive = pidAlive(record.pid);
  if (typeof fetchImpl !== 'function') { leg.health = 'unreachable'; leg.error = leg.error || 'No fetch available'; return leg; }
  try {
    const signal = typeof AbortSignal !== 'undefined' && typeof AbortSignal.timeout === 'function' ? AbortSignal.timeout(timeoutMs) : undefined;
    const response = await fetchImpl(`${record.origin.replace(/\/$/, '')}/api/service`, { method: 'GET',
      headers: { Authorization: `Bearer ${record.token}` }, signal });
    if (response && typeof response.status === 'number') {
      if (response.status === 200) {
        let body = null;
        try { body = await response.json(); } catch { body = null; }
        // A 200 with a partial body is not proof the service is healthy: the
        // endpoint's contract is {version, startedAt, agents}.
        const usableBody = body && typeof body === 'object' && typeof body.startedAt === 'string' &&
          (typeof body.agents === 'number' || typeof body.agents === 'string') && body.version !== undefined;
        if (usableBody) {
          leg.health = 'ok';
          leg.service = { version: body.version, startedAt: body.startedAt, agents: body.agents };
        } else { leg.health = 'invalid'; leg.error = leg.error || 'Incomplete service response'; }
      } else if (response.status === 401 || response.status === 403) leg.health = 'unauthenticated';
      else { leg.health = 'unreachable'; leg.error = leg.error || `Service status ${response.status}`; }
    } else if (response && typeof response === 'object') {
      // Injectable stub that returns the service body directly; it must still
      // satisfy the endpoint contract to count as healthy.
      const stubUsable = typeof response.startedAt === 'string' &&
        (typeof response.agents === 'number' || typeof response.agents === 'string') && response.version !== undefined;
      if (stubUsable) {
        leg.health = 'ok';
        leg.service = { version: response.version, startedAt: response.startedAt, agents: response.agents };
      } else { leg.health = 'invalid'; leg.error = leg.error || 'Incomplete service response'; }
    } else { leg.health = 'invalid'; leg.error = leg.error || 'Unusable service response'; }
  } catch (error) { leg.health = 'unreachable'; leg.error = leg.error || shortError(error); }
  return leg;
}

// Compose the receipt from real, read-only probes. An absent or broken
// environment leg is recorded as unobserved with a short error, never thrown.
export async function activationReceipt(options = {}) {
  const repo = options.repo ?? process.cwd();
  const root = options.root ?? defaultRoot();
  const managerRoot = options.managerRoot ?? join(homedir(), '.edda-agent-manager');
  const eddaBin = options.eddaBin ?? process.env.EDDA_BIN ?? 'edda';
  const timeoutMs = Number.isFinite(options.timeoutMs) && options.timeoutMs > 0 ? options.timeoutMs : 2000;
  const fetchImpl = options.fetch ?? globalThis.fetch;
  const runGit = options.runGit ?? defaultRunGit(timeoutMs);
  const runVersion = options.runVersion ?? defaultRunVersion(timeoutMs);

  const repository = repositoryLeg(repo, runGit);
  const edda = eddaLeg(eddaBin, runVersion);
  const pi = piLeg(root, repo, options.clientRoot);
  const manager = await managerLeg(managerRoot, timeoutMs, fetchImpl);
  const legs = { repository, edda, pi, manager };
  return { receiptVersion: RECEIPT_VERSION, observedAt: new Date().toISOString(), ...legs, coherence: evaluateCoherence(legs) };
}

const VALUE_FLAGS = new Set(['--repo', '--registry-root', '--manager-root', '--edda-bin', '--client-root', '--timeout']);
function parseArgs(argv) {
  const options = { json: false, check: false };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === '--json') options.json = true;
    else if (flag === '--check') options.check = true;
    else if (VALUE_FLAGS.has(flag)) {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith('--')) throw new Error(`Missing value for ${flag}`);
      index += 1;
      if (flag === '--repo') options.repo = value;
      else if (flag === '--registry-root') options.registryRoot = value;
      else if (flag === '--manager-root') options.managerRoot = value;
      else if (flag === '--edda-bin') options.eddaBin = value;
      else if (flag === '--client-root') options.clientRoot = value;
      else {
        const timeout = Number(value);
        if (!Number.isFinite(timeout) || timeout <= 0) throw new Error(`Invalid --timeout value: ${value}`);
        options.timeout = timeout;
      }
    } else throw new Error(`Unknown option ${flag}`);
  }
  return options;
}

const or = (value, fallback) => (value === null || value === undefined || value === '' ? fallback : value);
function formatReceipt(receipt) {
  const { repository, edda, pi, manager, coherence } = receipt;
  const lines = [
    `activation receipt v${receipt.receiptVersion}  ${receipt.observedAt}`,
    `coherence: ${coherence.status} (${coherence.observedLegs}/${coherence.requiredLegs} legs observed)`,
    '',
    `repository  ${repository.root}`,
    `  head      ${or(repository.headRevision, 'unobserved')} ${repository.headRef ? `(${repository.headRef})` : ''}`.trimEnd(),
    `  dirty     ${repository.dirty === null ? 'unknown' : repository.dirty}`,
    `  pi-tree   ${or(repository.piPackagingRevision, 'unobserved')}`,
    `edda        ${edda.binary}`,
    `  version   ${edda.observed ? `${or(edda.version, 'unknown')} (${or(edda.revision, 'unknown')}${edda.dirtyBuild ? '-dirty' : ''} ${or(edda.builtAt, 'unknown')})` : `unobserved${edda.error ? ` (${edda.error})` : ''}`}`,
    `pi          ${pi.clientPath}`,
    `  client    ${or(pi.clientVersion, 'unknown')} (${or(pi.clientSource, 'unknown')})`,
    `  installed ${or(pi.installedReleaseId, 'unobserved')}${pi.installedReleaseVersion ? ` (${pi.installedReleaseVersion})` : ''}`,
    `  repo      ${or(pi.repoReleaseId, 'unobserved')}${pi.repoReleaseVersion ? ` (${pi.repoReleaseVersion})` : ''}`,
    `  registry  ${pi.registryRoot}`,
    `  releases  ${pi.pinnedReleaseIds.length} pinned, ${pi.relevantReleaseIds.length} referenced by runs`,
    `manager     ${manager.root}`,
    `  configured ${manager.configured ? or(manager.configured.headSha, 'unknown') : 'absent'}${manager.configuredAt ? ` at ${manager.configuredAt}` : ''}`,
    `  running   ${manager.running ? `pid ${manager.running.pid} instance ${manager.running.instanceId} ${manager.running.origin}` : 'absent'}`,
    `  pidAlive  ${manager.pidAlive === null ? 'unknown' : manager.pidAlive}`,
    `  health    ${manager.health}${manager.service ? ` (service v${or(manager.service.version, '?')}, agents ${or(manager.service.agents, '?')})` : ''}`,
  ];
  lines.push('', 'findings');
  if (!coherence.findings.length) lines.push('  (none)');
  else for (const finding of coherence.findings) lines.push(`  - ${finding.leg}/${finding.code}: ${finding.detail}`);
  return lines.join('\n');
}

async function main(argv) {
  let options;
  try { options = parseArgs(argv); }
  catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 2; return; }
  const receipt = await activationReceipt({ repo: options.repo, root: options.registryRoot,
    managerRoot: options.managerRoot, eddaBin: options.eddaBin, clientRoot: options.clientRoot, timeoutMs: options.timeout });
  process.stdout.write(options.json ? `${JSON.stringify(receipt, null, 2)}\n` : `${formatReceipt(receipt)}\n`);
  if (options.check) process.exitCode = receipt.coherence.status === 'coherent' ? 0 : 2;
}

const invoked = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : '';
const isMain = invoked && (process.platform === 'win32' ? invoked.toLowerCase() === import.meta.url.toLowerCase() : invoked === import.meta.url);
if (isMain) {
  main(process.argv.slice(2)).catch((error) => { process.stderr.write(`${shortError(error)}\n`); process.exitCode = 2; });
}
