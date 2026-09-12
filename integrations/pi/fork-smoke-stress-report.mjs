import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
const [root, output] = process.argv.slice(2);
if (!root) throw new Error('Usage: node fork-smoke-stress-report.mjs ROOT [OUTPUT]');
const read = (name) => JSON.parse(readFileSync(join(root, name), 'utf8'));
const manifest = read('manifest.json'), results = read('results.json'), samples = read('resources.json');
const rows = results.map((r) => {
  const sum = (fn) => r.workers.reduce((n, w) => n + (fn(w) || 0), 0);
  const ids = new Set(r.workers.map((w) => w.pid));
  const resources = samples.filter((s) => s.at >= r.startedAt && s.at <= r.startedAt + r.elapsedMs && s.processes)
    .map((s) => s.processes.filter((p) => ids.has(p.Id)));
  return { label: r.label, inherited: r.inherited, dependency: r.dependency, passed: r.passed,
    elapsedSeconds: r.elapsedMs / 1000, integrationSeconds: r.integrationMs / 1000,
    workers: r.workers.length, workersPassed: r.workers.filter((w) => w.passed).length,
    firstSubmissionPassed: r.workers.filter((w) => w.submissions?.[0]?.accepted).length,
    submissions: sum((w) => w.submissions?.length), requests: sum((w) => w.metrics.requests), toolCalls: sum((w) => w.metrics.toolCalls),
    toolErrors: sum((w) => w.metrics.toolErrors), totalTokens: sum((w) => w.metrics.totalTokens), reportedUSD: sum((w) => w.metrics.reportedCost),
    input: sum((w) => w.metrics.input), output: sum((w) => w.metrics.output), cacheRead: sum((w) => w.metrics.cacheRead), cacheWrite: sum((w) => w.metrics.cacheWrite),
    scopeViolations: r.workers.filter((w) => w.extraChanges?.length).length,
    errors: r.workers.flatMap((w) => [...w.metrics.errors, ...(w.error ? [{ category: w.error }] : [])]),
    sampledPeakWorkerRssMiB: resources.length ? Math.max(...resources.map((ps) => ps.reduce((n, p) => n + p.WorkingSet64, 0))) / 2 ** 20 : null,
    sampledMaxConcurrentWorkers: resources.length ? Math.max(...resources.map((ps) => ps.length)) : null,
    dependencyReceipt: r.dependencyReceipt || null, validations: r.integrations.map((i) => ({ passed: i.passed, output: i.output })),
    sessions: r.workers.map((w) => w.sessionId) };
});
const summary = { version: 1, model: manifest.model, thinking: manifest.thinking, base: manifest.base,
  contractV1Digest: manifest.contractV1Digest, contractV2Digest: manifest.contractV2Digest, acceptanceV2Digest: manifest.acceptanceV2Digest,
  sourceDigest: manifest.sourceDigest, policy: manifest.policy, dependencyLimit: manifest.dependencyLimit,
  pins: manifest.pins.map((p) => ({ filename: p.path.split(/[\\/]/).at(-1), digest: p.digest })), rows,
  totalReportedUSD: rows.reduce((n, r) => n + r.reportedUSD, 0),
  limitations: ['Custom tool/validation workflow and fanout change together; no isolated causal speedup claim.',
    'Six workers produce two full deliveries; primary three workers produce one.',
    'Inherited preparation is reused from the primary experiment and reported there.',
    'RSS is sampled, not exact peak; generated code acceptance is not sandboxed.', manifest.dependencyLimit] };
if (output) writeFileSync(output, JSON.stringify(summary, null, 2) + '\n');
console.log(JSON.stringify(summary, null, 2));
