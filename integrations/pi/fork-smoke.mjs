// Explicit live-model experiment; never run by npm test. Preserves all owned artifacts.
import { execFileSync, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdirSync, readFileSync, writeFileSync, copyFileSync, readdirSync, realpathSync } from 'node:fs';
import { join, resolve, relative, isAbsolute, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { totalmem, freemem, platform, cpus } from 'node:os';
import { ExperimentPi, checkpoint, forkCheckpoint, sessionSDK, save } from './fork-smoke-runtime.mjs';
import { privateRoot, digest } from './store.mjs';

const [entryArg, rootArg, repetitionsArg = '3'] = process.argv.slice(2);
if (!entryArg || !rootArg || !/^[1-3]$/.test(repetitionsArg)) throw new Error('Usage: node fork-smoke.mjs PI_ENTRY NEW_PRIVATE_ROOT [1..3 repetitions]');
const entry = resolve(entryArg), root = resolve(rootArg), repetitions = Number(repetitionsArg);
mkdirSync(root); privateRoot(root); // Existing experiment must never be overwritten/replayed.
const here = fileURLToPath(new URL('.', import.meta.url));
const fixture = join(here, 'fixtures', 'fork-workload');
// This installed Pi/model normalizes requested low to high; pin the observed level.
const provider = 'openrouter', model = 'deepseek/deepseek-v4.1-flash', thinking = 'high';
const modules = ['cards', 'attention', 'overview'], active = new Set(), results = [], samples = [];
const execAsync = promisify(execFile);
const git = (cwd, args) => execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', windowsHide: true, timeout: 30000, stdio: ['ignore', 'pipe', 'pipe'] }).trimEnd();
const repo = join(root, 'baseline'); mkdirSync(repo);
const contract = readFileSync(join(fixture, 'CONTRACT.md'), 'utf8');
writeFileSync(join(repo, 'CONTRACT.md'), contract);
copyFileSync(join(fixture, 'acceptance.mjs'), join(repo, 'acceptance.mjs'));
writeFileSync(join(repo, 'AGENTS.md'), 'This is an isolated experiment. Follow CONTRACT.md and your current assigned module. No commits, external access, dependency installation or child agents. The controller runs tests.\n');
for (const name of modules) writeFileSync(join(repo, `${name}.mjs`), '// Implement the assigned CONTRACT.md export.\n');
mkdirSync(join(repo, 'background'));
for (const name of ['supervisor-store.mjs', 'supervisor-engine.mjs', 'supervisor-policy.mjs']) {
  // Exact source references are context only, never editable task output.
  copyFileSync(join(here, name), join(repo, 'background', name));
}
git(repo, ['init', '-b', 'codex/fork-baseline']);
git(repo, ['config', 'user.name', 'Edda fork experiment']); git(repo, ['config', 'user.email', 'experiment@localhost']);
git(repo, ['add', '.']); git(repo, ['commit', '-m', 'test: freeze cross-project card experiment workload']);
const base = git(repo, ['rev-parse', 'HEAD']);
const pinPaths = [fileURLToPath(import.meta.url), join(here, 'fork-smoke-runtime.mjs'), entry,
  join(dirname(dirname(entry)), 'core', 'session-manager.js'), join(dirname(dirname(dirname(entry))), 'package.json')];
const pins = pinPaths.map((path) => ({ path, digest: digest(readFileSync(path)) }));
const verifyPins = () => { for (const pin of pins) if (digest(readFileSync(pin.path)) !== pin.digest) throw new Error(`Execution pin changed: ${pin.path}`); };
const manifest = { version: 1, createdAt: new Date().toISOString(), entry, root, provider, model, thinking,
  repetitions, base, contractDigest: digest(contract), acceptanceDigest: digest(readFileSync(join(repo, 'acceptance.mjs'))),
  tools: ['read', 'write', 'edit'], timeoutMs: 240000, repairs: 2, autoRetry: false, autoCompaction: false,
  machine: { platform: platform(), totalMemoryBytes: totalmem(), initialFreeMemoryBytes: freemem(), logicalCPUs: cpus().length },
  design: 'Matched three-session serial/parallel x native inherited/fresh brief; additional single-session practical baseline',
  source: readdirSync(join(repo, 'background')), pins,
  piVersion: JSON.parse(readFileSync(pinPaths.at(-1), 'utf8')).version };
save(join(root, 'manifest.json'), manifest);
const { SessionManager } = await sessionSDK(entry);
let sampling = false;
const sampler = setInterval(async () => {
  const ids = [...active].filter((p) => p.child.exitCode === null && p.child.signalCode === null).map((p) => p.child.pid);
  if (sampling || !ids.length || process.platform !== 'win32') return;
  sampling = true;
  try {
    const { stdout } = await execAsync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
      `Get-Process -Id ${ids.join(',')} -ErrorAction SilentlyContinue | Select-Object Id,WorkingSet64,CPU | ConvertTo-Json -Compress`], { windowsHide: true, timeout: 5000 });
    const values = stdout.trim() ? JSON.parse(stdout) : [];
    samples.push({ at: Date.now(), processes: Array.isArray(values) ? values : [values] });
    save(join(root, 'resources.json'), samples);
  } catch { samples.push({ at: Date.now(), unavailable: true }); }
  finally { sampling = false; }
}, 3000);
function worktree(label) {
  const cwd = join(root, label); git(repo, ['worktree', 'add', '-b', `codex/${label}`, cwd, base]); return cwd;
}
function validate(cwd, bundle) {
  const startedAt = Date.now();
  try {
    const output = execFileSync(process.execPath, [join(repo, 'acceptance.mjs'), cwd, bundle], { encoding: 'utf8', windowsHide: true, timeout: 15000, stdio: ['ignore', 'pipe', 'pipe'] });
    return { passed: true, elapsedMs: Date.now() - startedAt, output: output.trim() };
  } catch (error) { return { passed: false, elapsedMs: Date.now() - startedAt, output: String(error.stderr || error.message).slice(0, 5000) }; }
}
function scope(cwd, assigned, pi) {
  const allowed = assigned.map((n) => `${n}.mjs`);
  const changed = git(cwd, ['status', '--porcelain', '-uall']).split('\n').filter(Boolean).map((s) => s.slice(3).replace(/^"|"$/g, ''));
  const extra = changed.filter((p) => !allowed.includes(p));
  const outsideWrites = pi.metrics.writes.filter((w) => {
    if (typeof w.path !== 'string') return true;
    const rel = relative(cwd, resolve(cwd, w.path)).replaceAll('\\', '/');
    return isAbsolute(rel) || !allowed.includes(rel);
  });
  return { changed, extra, outsideWrites, passed: !extra.length && !outsideWrites.length };
}
const brief = (assigned) => `Implement ${assigned.map((n) => n + '.mjs').join(', ')} in your CURRENT working directory only. Read CONTRACT.md here; it is the frozen specification. The current assignment overrides any broader inherited planning role. Other files, including background sources and acceptance.mjs, are read-only. Use read/write/edit tools to finish now; the controller executes acceptance and will supply failures if repairs are needed. No additional approval is required. Do not commit, spawn, or access outside your current worktree. Report completion briefly.`;
let parent, source;
try {
  verifyPins();
  parent = new ExperimentPi({ entry, cwd: repo, dir: join(root, 'parent'), provider, model, thinking }); active.add(parent);
  await parent.ready();
  const prepStart = Date.now();
  console.log(JSON.stringify({ phase: 'planning_parent', root }));
  const sourceText = ['CONTRACT.md', ...manifest.source.map((name) => `background/${name}`)]
    .map((name) => `FILE ${name}\n${readFileSync(join(repo, name), 'utf8')}\nEND FILE ${name}`).join('\n\n');
  // Supply exact source bytes, avoiding a confound where a seed's failed reads
  // silently produce a much shorter inherited context than the manifest declares.
  await parent.prompt('You are a read-only planning context seed. The complete frozen source is supplied below; do not call tools. Explain only the product intent, scope boundaries and the three module responsibilities in under 250 words. Do not provide solution code/algorithms. Implementation is delegated later; stop after the short context summary.\n\n' + sourceText);
  if (parent.metrics.writes.length || git(repo, ['status', '--porcelain'])) throw new Error('Planning parent changed the frozen workload');
  source = await checkpoint(parent, SessionManager, join(root, 'checkpoint.jsonl'));
  save(join(root, 'preparation.json'), { elapsedMs: Date.now() - prepStart, source, metrics: parent.metrics });
  console.log(JSON.stringify({ phase: 'checkpoint_ready', entries: source.entries, bytes: source.bytes, metrics: parent.metrics }));

  async function runArm(label, parallel, inherited, single = false) {
    const startedAt = Date.now(), armDir = join(root, label); mkdirSync(armDir);
    const groups = single ? [modules] : modules.map((n) => [n]);
    const workers = [];
    console.log(JSON.stringify({ phase: 'arm_started', label, parallel, inherited, single }));
    async function worker(assigned, index) {
      verifyPins();
      const scheduledAt = Date.now(), cwd = worktree(`${label}-w${index}`), dir = join(armDir, `w${index}`);
      mkdirSync(dir);
      const seeded = inherited ? forkCheckpoint(SessionManager, source, cwd, join(dir, 'sessions')) : {};
      const prompt = brief(assigned);
      save(join(dir, 'assignment.json'), { assigned, cwd, prompt, promptBytes: Buffer.byteLength(prompt), source: inherited ? source : null, base, seeded });
      const pi = new ExperimentPi({ entry, cwd, dir: join(dir, 'sessions'), sessionFile: seeded.sessionFile, provider, model, thinking }); active.add(pi);
      const receipt = { index, assigned, cwd, scheduledAt, queueMs: scheduledAt - startedAt, repairs: [] };
      workers.push(receipt);
      try {
        const state = await pi.ready();
        if (state.model?.provider !== provider || state.model?.id !== model || state.thinkingLevel !== thinking) throw new Error('Observed model/thinking differs from frozen profile');
        if (inherited && state.sessionId !== seeded.sessionId) throw new Error('Child runtime did not open seeded session');
        if (inherited) {
          const opened = SessionManager.open(state.sessionFile);
          if (opened.getLeafId() !== source.leaf || realpathSync(opened.getHeader().cwd) !== realpathSync(cwd)) throw new Error('Child context/cwd mismatch at runtime');
        }
        receipt.sessionId = state.sessionId; receipt.sessionFile = state.sessionFile; receipt.pid = pi.child.pid;
        receipt.startupMs = Date.now() - scheduledAt;
        const work = await pi.prompt(prompt); receipt.workMs = work.elapsedMs;
        receipt.initial = validate(cwd, single ? 'all' : assigned[0]);
        receipt.initialScope = scope(cwd, assigned, pi);
        receipt.initialPassed = receipt.initial.passed && receipt.initialScope.passed && !pi.metrics.errors.length;
        writeFileSync(join(dir, 'initial.diff'), git(cwd, ['diff', '--no-ext-diff']));
        let last = receipt.initial, currentScope = receipt.initialScope;
        // Recoverable tool errors (e.g. read(directory)) are telemetry, not a
        // reason to suppress acceptance-driven repair after the turn settled.
        for (let attempt = 1; attempt <= 2 && !last.passed && currentScope.passed && !pi.metrics.errors.length; attempt++) {
          const repaired = await pi.prompt(`Acceptance failed. Fix ONLY ${assigned.map((n) => n + '.mjs').join(', ')} to satisfy unchanged CONTRACT.md. Failure:\n${last.output}`);
          last = validate(cwd, single ? 'all' : assigned[0]);
          currentScope = scope(cwd, assigned, pi);
          receipt.repairs.push({ attempt, elapsedMs: repaired.elapsedMs, validation: last, scope: currentScope });
        }
        receipt.final = last; receipt.finalScope = scope(cwd, assigned, pi);
        receipt.passed = last.passed && receipt.finalScope.passed && !pi.metrics.errors.length;
      } catch (error) { receipt.passed = false; receipt.error = error.message; }
      finally {
        receipt.metrics = pi.metrics; receipt.elapsedMs = Date.now() - scheduledAt;
        writeFileSync(join(dir, 'final.diff'), git(cwd, ['diff', '--no-ext-diff']));
        save(join(dir, 'receipt.json'), receipt);
        await pi.stop(); active.delete(pi);
      }
    }
    if (parallel) await Promise.all(groups.map(worker));
    else for (const [i, assigned] of groups.entries()) await worker(assigned, i);
    const integrationStart = Date.now(), integrated = worktree(`${label}-integrated`);
    for (const worker of workers) for (const name of worker.assigned) copyFileSync(join(worker.cwd, `${name}.mjs`), join(integrated, `${name}.mjs`));
    const integration = validate(integrated, 'all');
    const parentState = await parent.rpc('get_state');
    const parentUnchanged = parentState.sessionId === source.sessionId && digest(readFileSync(parentState.sessionFile)) === source.digest;
    const result = { label, parallel, inherited, single, startedAt, elapsedMs: Date.now() - startedAt,
      integrationMs: Date.now() - integrationStart, integration, parentUnchanged, workers,
      passed: integration.passed && parentUnchanged && workers.every((w) => w.passed), integrated };
    results.push(result); save(join(armDir, 'result.json'), result); save(join(root, 'results.json'), results);
    console.log(JSON.stringify({ phase: 'arm_finished', label, passed: result.passed, elapsedMs: result.elapsedMs,
      workers: workers.map((w) => ({ assigned: w.assigned, passed: w.passed, initialPassed: w.initialPassed, repairs: w.repairs.length, error: w.error, workMs: w.workMs, metrics: w.metrics })) }));
  }
  const arms = [[false, true], [true, false], [true, true], [false, false]];
  for (let rep = 0; rep < repetitions; rep++) {
    const order = rep % 2 ? [...arms].reverse() : arms;
    for (const [parallel, inherited] of order) await runArm(`r${rep + 1}-${parallel ? 'parallel' : 'serial'}-${inherited ? 'fork' : 'brief'}`, parallel, inherited);
  }
  await runArm('practical-single-fork', false, true, true);
  await runArm('practical-single-brief', false, false, true);
  console.log(JSON.stringify({ phase: 'complete', root, arms: results.length, passed: results.filter((r) => r.passed).length }));
} finally {
  clearInterval(sampler);
  for (const pi of active) await pi.stop();
  save(join(root, 'results.json'), results);
}
