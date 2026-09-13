import type { AgentBinding, CandidateListView, CandidateView, DiscoveredRun, DiscoveryReport } from './contracts.js';
import { hash } from './config.js';

/** Stable opaque handle for one discovered run. Never exposes the registry path. */
export function candidateId(run: Pick<DiscoveredRun, 'registryRoot'> & Partial<Pick<DiscoveredRun, 'sessionId' | 'runId'>>): string {
  return hash(`${run.registryRoot}\0${run.sessionId ?? run.runId ?? ''}`).slice(0, 24);
}
/** A non-sensitive label for a configured root when its listing fails. */
export function rootLabel(registryRoot: string): string {
  return hash(registryRoot).slice(0, 12);
}
// Pi session identity when a run has one; otherwise the managed run id. A live
// session and its recorded managed run share this identity inside their registry
// root. A run is scoped to its root — the same key `config.json` and
// `candidateId` use — so the same session under another root is a different
// registration, not a duplicate to merge.
function identity(run: DiscoveredRun): string {
  return `${run.registryRoot}\0${run.sessionId ? `s:${run.sessionId}` : `r:${run.runId ?? candidateId(run)}`}`;
}
function merge(preferred: DiscoveredRun, other: DiscoveredRun): DiscoveredRun {
  return { ...preferred, sessionId: preferred.sessionId ?? other.sessionId, runId: preferred.runId ?? other.runId,
    instanceId: preferred.instanceId ?? other.instanceId, workspace: preferred.workspace ?? other.workspace,
    lastProgressAt: preferred.lastProgressAt ?? other.lastProgressAt };
}
// The same run can appear both as a live session and as recorded managed
// inventory. Keep one run per (registry root, session/run) identity, preferring
// the live registration so an operator never sees a stale duplicate. A session
// present in two configured roots stays two root-scoped candidates, because that
// is exactly the identity configuration and candidate ids already use.
export function dedupeRuns(report: DiscoveryReport): DiscoveredRun[] {
  const byIdentity = new Map<string, DiscoveredRun>();
  for (const run of report.runs) {
    const key = identity(run), existing = byIdentity.get(key);
    if (!existing) { byIdentity.set(key, run); continue; }
    byIdentity.set(key, run.live && !existing.live ? merge(run, existing) : merge(existing, run));
  }
  return [...byIdentity.values()];
}
// Public projection shared by the candidate list and by explicit registration.
// `configuredAgentId` is matched on the same `(registryRoot, sessionId)` identity
// `config.json` enforces, so a same-id run under another configured root (or a
// Codex agent whose workspace happens to equal a Pi registry root) is not
// mislabelled as an already-selected Pi run.
export function projectCandidates(report: DiscoveryReport, agents: AgentBinding[]): CandidateListView {
  const configured = new Map(agents.map((agent) => [`${agent.registryRoot}\0${agent.sessionId}`, agent.id]));
  const candidates: CandidateView[] = dedupeRuns(report)
    .sort((left, right) => (Number(right.live) - Number(left.live)) || (left.sessionId ?? left.runId ?? '').localeCompare(right.sessionId ?? right.runId ?? ''))
    .map((run) => ({
      id: candidateId(run), sessionId: run.sessionId, runId: run.runId, instanceId: run.instanceId, state: run.state, live: run.live,
      source: run.source, workspace: run.workspace, lastProgressAt: run.lastProgressAt, reason: run.reason,
      configuredAgentId: run.sessionId ? configured.get(`${run.registryRoot}\0${run.sessionId}`) ?? null : null,
    }));
  const issues = [...new Map(report.failures.map((failure) => [`${rootLabel(failure.registryRoot)}\0${failure.message}`,
    { label: rootLabel(failure.registryRoot), message: failure.message }])).values()];
  return { candidates, issues, generatedAt: new Date().toISOString() };
}
