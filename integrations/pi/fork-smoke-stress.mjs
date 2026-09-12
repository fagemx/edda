// Follow-up experiments: six-way fanout and a changed interface with gated release.
import { execFileSync, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdirSync, readFileSync, writeFileSync, copyFileSync, readdirSync, existsSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { ExperimentPi, forkCheckpoint, sessionSDK, save } from './fork-smoke-runtime.mjs';
import { digest, privateRoot } from './store.mjs';

const [entryArg, primaryArg, rootArg] = process.argv.slice(2);
if (!entryArg || !primaryArg || !rootArg) throw new Error('Usage: node fork-smoke-stress.mjs PI_ENTRY COMPLETED_PRIMARY_ROOT NEW_ROOT');
const entry = resolve(entryArg), primary = resolve(primaryArg), root = resolve(rootArg);
mkdirSync(root); privateRoot(root);
const here = fileURLToPath(new URL('.', import.meta.url)), extension = join(here, 'fixtures', 'fork-bundle-tools.mjs');
const primaryManifest = JSON.parse(readFileSync(join(primary, 'manifest.json'), 'utf8'));
const source = JSON.parse(readFileSync(join(primary, 'preparation.json'), 'utf8')).source;
if (digest(readFileSync(source.path)) !== source.digest) throw new Error('Primary checkpoint changed');
const { SessionManager } = await sessionSDK(entry);
const profile = { entry, provider: primaryManifest.provider, model: primaryManifest.model, thinking: primaryManifest.thinking,
  extensions: [extension], tools: ['fork_read', 'fork_submit'] };
const git = (cwd, args) => execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', windowsHide: true, timeout: 30000, stdio: ['ignore', 'pipe', 'pipe'] }).trimEnd();
const repo = join(root, 'baseline');
execFileSync('git', ['clone', '--no-hardlinks', join(primary, 'baseline'), repo], { windowsHide: true, stdio: 'pipe', timeout: 30000 });
const base = git(repo, ['rev-parse', 'HEAD']);
const pins = [fileURLToPath(import.meta.url), extension, join(here, 'fork-smoke-runtime.mjs'), entry,
  join(dirname(dirname(entry)), 'core', 'session-manager.js'), join(dirname(dirname(dirname(entry))), 'package.json')]
  .map((path) => ({ path, digest: digest(readFileSync(path)) }));
mkdirSync(join(root, 'harness-source'));
for (const pin of pins.slice(0, 3)) copyFileSync(pin.path, join(root, 'harness-source', pin.path.split(/[\\/]/).at(-1)));
const verify = () => { for (const p of pins) if (digest(readFileSync(p.path)) !== p.digest) throw new Error(`Execution pin changed: ${p.path}`); };
const v1 = readFileSync(join(repo, 'CONTRACT.md'), 'utf8'), originalAcceptance = readFileSync(join(repo, 'acceptance.mjs'), 'utf8');
const v2 = v1.replace('experiment contract v1', 'experiment contract v2')
  .replace('total,ready,running,done,unknown', 'total,ready,active,done,unknown')
  .replace('Status ready/running/done count in those categories;', 'Status ready/running/done count in ready/active/done respectively;')
  .replace('then running count descending', 'then active count descending') + '\nInterface update: output counts.active replaces counts.running everywhere. Input task status remains running. All other behavior is unchanged. This v2 contract supersedes inherited v1 context.\n';
const v2Acceptance = originalAcceptance.replace(/counts:\s*\{[^}]*\}/g, (s) => s.replace(/\brunning:/g, 'active:')).replaceAll('.counts.running', '.counts.active');
const acceptanceV2 = join(root, 'acceptance-v2.mjs'); writeFileSync(acceptanceV2, v2Acceptance);
save(join(root, 'manifest.json'), { version: 1, base, primaryBase: primaryManifest.base, sourceDigest: source.digest,
  provider: profile.provider, model: profile.model, thinking: profile.thinking, pins,
  contractV1Digest: digest(v1), contractV2Digest: digest(v2), acceptanceV2Digest: digest(v2Acceptance),
  policy: 'Only fork_read/fork_submit; up to four validated submissions inside one model turn; 240s timeout. No provider retries. Isolated coding prototype, not a security sandbox.',
  dependencyLimit: 'v2 measures stale-checkpoint/current-contract correctness and an imposed verified-release barrier. Contract already names the interface; this does not establish that the source artifact was necessary for correctness.',
  scenarios: ['six-fork', 'six-brief', 'six-brief-repeat', 'six-fork-repeat', 'interface-v2-fork', 'interface-v2-brief'] });
const active = new Set(), results = [], samples = [];
const execAsync = promisify(execFile); let sampling = false;
const timer = setInterval(async () => {
  const ids = [...active].filter((p) => p.child.exitCode === null && p.child.signalCode === null).map((p) => p.child.pid);
  if (sampling || !ids.length || process.platform !== 'win32') return;
  sampling = true;
  try {
    const { stdout } = await execAsync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
      `Get-Process -Id ${ids.join(',')} -ErrorAction SilentlyContinue | Select-Object Id,WorkingSet64,CPU | ConvertTo-Json -Compress`], { windowsHide: true, timeout: 5000 });
    const ps = stdout.trim() ? JSON.parse(stdout) : [];
    samples.push({ at: Date.now(), processes: Array.isArray(ps) ? ps : [ps] }); save(join(root, 'resources.json'), samples);
  } catch { samples.push({ at: Date.now(), unavailable: true }); }
  finally { sampling = false; }
}, 3000);
const worktree = (label) => { const cwd = join(root, label); git(repo, ['worktree', 'add', '-b', `codex/${label}`, cwd, base]); return cwd; };
function acceptance(path, cwd, module = 'all') {
  try { return { passed: true, output: execFileSync(process.execPath, [path, cwd, module], { encoding: 'utf8', windowsHide: true, timeout: 15000, stdio: ['ignore', 'pipe', 'pipe'] }).trim() }; }
  catch (error) { return { passed: false, output: String(error.stderr || error.message).slice(0, 5000) }; }
}
async function scenario(label, inherited, dependency = false) {
  const startedAt = Date.now(), dir = join(root, label); mkdirSync(dir);
  const receipts = [], livePromises = [], contract = dependency ? v2 : v1, test = dependency ? acceptanceV2 : join(repo, 'acceptance.mjs');
  console.log(JSON.stringify({ phase: 'stress_started', label }));
  function start(module, index, suppliedDependency = null) {
    verify();
    const scheduledAt = Date.now(), cwd = worktree(`${label}-w${index}`), workerDir = join(dir, `w${index}`); mkdirSync(workerDir);
    writeFileSync(join(cwd, 'CONTRACT.md'), contract);
    save(join(cwd, '.fork-bundle.json'), { module, acceptance: test, receipts: workerDir, dependency: suppliedDependency });
    const expectedStatus = git(cwd, ['status', '--porcelain', '-uall']);
    const frozenInputs = ['CONTRACT.md', '.fork-bundle.json'].map((name) => ({ name, digest: digest(readFileSync(join(cwd, name))) }));
    const seed = inherited ? forkCheckpoint(SessionManager, source, cwd, join(workerDir, 'sessions')) : {};
    const pi = new ExperimentPi({ ...profile, cwd, dir: join(workerDir, 'sessions'), sessionFile: seed.sessionFile }); active.add(pi);
    const receipt = { index, module, cwd, scheduledAt, dependency: suppliedDependency ? { digest: digest(suppliedDependency.code), producer: suppliedDependency.producer } : null };
    receipts.push(receipt);
    let finished = false;
    const done = (async () => {
      try {
        const state = await pi.ready(); receipt.sessionId = state.sessionId; receipt.pid = pi.child.pid;
        if (state.model?.id !== profile.model || state.model?.provider !== profile.provider || state.thinkingLevel !== profile.thinking) throw new Error('Observed model profile mismatch');
        if (inherited && (state.sessionId !== seed.sessionId || SessionManager.open(state.sessionFile).getLeafId() !== source.leaf)) throw new Error('Fork context mismatch');
        const result = await pi.prompt(`Complete the assigned ${module}.mjs now. Call fork_read for the CURRENT frozen contract${dependency ? ' v2, which supersedes inherited v1' : ''}. Use fork_submit with the full module source; it writes your one owned file and immediately runs real acceptance. Repair any reported failure with another submission, at most four total. After accepted=true, finish briefly. No further permission is needed. These two tools provide all required reading, writing and validation; do not try shell commands or create test/helper files.`);
        receipt.workMs = result.elapsedMs;
        receipt.validation = acceptance(test, cwd, module);
        const changed = git(cwd, ['status', '--porcelain', '-uall']).split('\n').filter(Boolean);
        const expected = new Set(expectedStatus.split('\n').filter(Boolean));
        receipt.extraChanges = changed.filter((line) => line.slice(3) !== `${module}.mjs` && !expected.has(line));
        for (const file of frozenInputs) if (digest(readFileSync(join(cwd, file.name))) !== file.digest) receipt.extraChanges.push(`modified frozen input: ${file.name}`);
        receipt.submissions = readdirSync(workerDir).filter((name) => /^submission-\d+\.json$/.test(name)).sort().map((name) => JSON.parse(readFileSync(join(workerDir, name), 'utf8')));
        receipt.passed = receipt.validation.passed && !receipt.extraChanges.length && !pi.metrics.errors.length && receipt.submissions.some((s) => s.accepted);
      } catch (error) { receipt.passed = false; receipt.error = error.message; }
      finally {
        receipt.elapsedMs = Date.now() - scheduledAt; receipt.metrics = pi.metrics;
        save(join(workerDir, 'receipt.json'), receipt); finished = true;
        await pi.stop(); active.delete(pi);
      }
      return receipt;
    })();
    livePromises.push(done);
    async function verifiedArtifact() {
      while (!finished) {
        const names = readdirSync(workerDir).filter((name) => /^submission-\d+\.json$/.test(name)).sort();
        if (names.length) {
          const last = JSON.parse(readFileSync(join(workerDir, names.at(-1)), 'utf8'));
          if (last.accepted && acceptance(test, cwd, module).passed) {
            const code = readFileSync(join(cwd, `${module}.mjs`), 'utf8');
            return { code, producer: receipt.sessionId, verifiedAt: Date.now(), producerSettled: finished };
          }
        }
        await delay(100);
      }
      if (!receipt.passed) throw new Error('Producer did not deliver a validated dependency');
      return { code: readFileSync(join(cwd, `${module}.mjs`), 'utf8'), producer: receipt.sessionId, verifiedAt: Date.now(), producerSettled: true };
    }
    return { done, verifiedArtifact };
  }
  let dependencyReceipt;
  try {
    if (dependency) {
      const producer = start('cards', 0); start('overview', 2);
      const artifact = await producer.verifiedArtifact();
      dependencyReceipt = { releasedAt: artifact.verifiedAt, producerSettledAtRelease: artifact.producerSettled, artifactDigest: digest(artifact.code) };
      start('attention', 1, artifact);
    } else {
      for (let i = 0; i < 6; i++) start(['cards', 'attention', 'overview'][i % 3], i);
    }
    await Promise.all(livePromises);
    const integrationStart = Date.now(), integrations = [];
    for (let group = 0; group < (dependency ? 1 : 2); group++) {
      const cwd = worktree(`${label}-integrated${group}`);
      for (const worker of receipts.filter((w) => Math.floor(w.index / 3) === group)) copyFileSync(join(worker.cwd, `${worker.module}.mjs`), join(cwd, `${worker.module}.mjs`));
      integrations.push({ cwd, ...acceptance(test, cwd) });
    }
    const result = { label, inherited, dependency, startedAt, elapsedMs: Date.now() - startedAt, integrationMs: Date.now() - integrationStart,
      dependencyReceipt, integrations, workers: receipts, passed: integrations.every((i) => i.passed) && receipts.every((r) => r.passed) };
    results.push(result); save(join(dir, 'result.json'), result); save(join(root, 'results.json'), results);
    console.log(JSON.stringify({ phase: 'stress_finished', label, passed: result.passed, elapsedMs: result.elapsedMs,
      workers: receipts.map((r) => ({ module: r.module, passed: r.passed, submissions: r.submissions?.length, workMs: r.workMs, error: r.error, metrics: r.metrics })), dependencyReceipt }));
  } catch (error) {
    await Promise.allSettled(livePromises);
    const result = { label, inherited, dependency, startedAt, elapsedMs: Date.now() - startedAt, integrationMs: 0,
      integrations: [], workers: receipts, passed: false, error: error.message };
    results.push(result); save(join(dir, 'result.json'), result); save(join(root, 'results.json'), results);
    console.log(JSON.stringify({ phase: 'stress_failed', label, error: error.message }));
  } finally { await Promise.allSettled(livePromises); }
}
try {
  for (const [label, inherited, dependency] of [['six-fork', true, false], ['six-brief', false, false], ['six-brief-repeat', false, false], ['six-fork-repeat', true, false], ['interface-v2-fork', true, true], ['interface-v2-brief', false, true]]) await scenario(label, inherited, dependency);
} finally { clearInterval(timer); for (const pi of active) await pi.stop(); save(join(root, 'results.json'), results); }
