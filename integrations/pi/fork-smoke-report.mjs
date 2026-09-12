// Publishable aggregate; deliberately excludes raw conversations/tool arguments.
import { readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
const [rootArg, output] = process.argv.slice(2);
if (!rootArg) throw new Error('Usage: node fork-smoke-report.mjs EXPERIMENT_ROOT [OUTPUT_JSON]');
const root = resolve(rootArg), read = (name) => JSON.parse(readFileSync(join(root, name), 'utf8'));
const manifest = read('manifest.json'), results = read('results.json'), preparation = read('preparation.json'), resources = read('resources.json');
const rows = results.map((arm) => {
  const workers = arm.workers, workerIds = new Set(workers.map((w) => w.pid));
  const samples = resources.filter((s) => s.at >= arm.startedAt && s.at <= arm.startedAt + arm.elapsedMs && s.processes)
    .map((s) => s.processes.filter((p) => workerIds.has(p.Id)));
  const sum = (fn) => workers.reduce((n, w) => n + (fn(w) || 0), 0);
  const activeTimes = workers.map((w) => (w.workMs || 0) + w.repairs.reduce((n, r) => n + r.elapsedMs, 0));
  const peakRss = samples.length ? Math.max(...samples.map((ps) => ps.reduce((n, p) => n + p.WorkingSet64, 0))) : null;
  const cpu = samples.length ? [...workerIds].reduce((n, id) => n + Math.max(0, ...samples.flatMap((ps) => ps.filter((p) => p.Id === id).map((p) => p.CPU || 0))), 0) : null;
  return { label: arm.label, parallel: arm.parallel, inherited: arm.inherited, single: arm.single, passed: arm.passed,
    elapsedSeconds: arm.elapsedMs / 1000, integrationSeconds: arm.integrationMs / 1000,
    activeSumSeconds: activeTimes.reduce((a, b) => a + b, 0) / 1000,
    activeCriticalSeconds: (arm.parallel ? Math.max(...activeTimes) : activeTimes.reduce((a, b) => a + b, 0)) / 1000,
    initialPassed: workers.filter((w) => w.initialPassed).length, workers: workers.length,
    repairs: sum((w) => w.repairs.length), repairSeconds: sum((w) => w.repairs.reduce((n, r) => n + r.elapsedMs, 0)) / 1000,
    requests: sum((w) => w.metrics.requests), toolCalls: sum((w) => w.metrics.toolCalls), toolErrors: sum((w) => w.metrics.toolErrors),
    totalTokens: sum((w) => w.metrics.totalTokens), input: sum((w) => w.metrics.input), output: sum((w) => w.metrics.output),
    cacheRead: sum((w) => w.metrics.cacheRead), cacheWrite: sum((w) => w.metrics.cacheWrite), reportedUSD: sum((w) => w.metrics.reportedCost),
    errors: workers.flatMap((w) => [...w.metrics.errors, ...(w.error ? [{ category: w.error }] : [])]),
    scopeViolations: workers.filter((w) => w.finalScope && !w.finalScope.passed).length,
    parentUnchanged: arm.parentUnchanged, sampledPeakWorkerRssMiB: peakRss === null ? null : peakRss / 2 ** 20,
    sampledWorkerCpuSeconds: cpu, resourceSamples: samples.length,
    validation: arm.integration.output, sessions: workers.map((w) => w.sessionId).filter(Boolean) };
});
const median = (xs) => { const a = [...xs].sort((x, y) => x - y); return a.length % 2 ? a[(a.length - 1) / 2] : (a[a.length / 2 - 1] + a[a.length / 2]) / 2; };
const groups = [];
for (const key of ['serial-fork', 'parallel-fork', 'serial-brief', 'parallel-brief']) {
  const group = rows.filter((r) => !r.single && r.label.endsWith(key));
  if (!group.length) continue;
  groups.push({ condition: key, n: group.length, passed: group.filter((r) => r.passed).length,
    medianSeconds: median(group.map((r) => r.elapsedSeconds)), minSeconds: Math.min(...group.map((r) => r.elapsedSeconds)), maxSeconds: Math.max(...group.map((r) => r.elapsedSeconds)),
    medianReportedUSD: median(group.map((r) => r.reportedUSD)), totalRepairs: group.reduce((n, r) => n + r.repairs, 0) });
}
const report = { version: 1, model: manifest.model, provider: manifest.provider, thinking: manifest.thinking, piVersion: manifest.piVersion,
  workloadCommit: manifest.base, contractDigest: manifest.contractDigest, acceptanceDigest: manifest.acceptanceDigest,
  pins: manifest.pins.map((p) => ({ filename: p.path.split(/[\\/]/).at(-1), digest: p.digest })),
  preparation: { seconds: preparation.elapsedMs / 1000, checkpointDigest: preparation.source.digest, checkpointEntries: preparation.source.entries,
    checkpointBytes: preparation.source.bytes, metrics: preparation.metrics }, machine: manifest.machine, groups, rows,
  totalReportedUSD: preparation.metrics.reportedCost + rows.reduce((n, r) => n + r.reportedUSD, 0),
  limitations: ['Small pure-JavaScript prototype; not hundreds of turns or a production delivery benchmark.',
    'Provider cache/load and run order uncontrolled beyond varied sequential arm ordering; no statistical significance claim.',
    'Worker processes sampled every ~3s on Windows; sampled RSS/CPU omit between-sample peaks, OS cache and controller/provider resources.',
    'Read/write/edit tools only; controller runs acceptance. Git worktrees isolate ordinary writes but are not a security sandbox.',
    'Single-session practical baselines always ran after primary matrix; secondary observations only.',
    'Preparation is reported separately; arm elapsed includes worktree/start/stop/integration but excludes harness development and human experiment design.'] };
if (output) writeFileSync(output, JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
