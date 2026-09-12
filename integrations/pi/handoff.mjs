import { join } from 'node:path';
import { digest, readJson, writeJson, validateId } from './store.mjs';
import { normalizeManifest, normalizeReport, problem, fitContext } from './handoff-schema.mjs';

const now = () => new Date().toISOString();
const notice = 'Observational handoff only: manifest scope and reports are declared data with references, not verified authority or task acceptance. No transcripts or private reasoning are loaded.';

export function createHandoff(dir, sessionId, instanceId) {
  const path = join(dir, 'handoff.json');
  let data = readJson(path);
  if (data && (data.version !== 1 || data.sessionId !== sessionId)) throw problem('Unsupported handoff storage identity/version');
  if (data) {
    validateId(data.instanceId);
    if (digest(JSON.stringify(normalizeManifest(data.manifest))) !== data.manifestRevision ||
      !Number.isSafeInteger(data.workEpoch) || data.workEpoch < 0 ||
      !Number.isSafeInteger(data.settledEpoch) || data.settledEpoch < 0 || data.settledEpoch > data.workEpoch ||
      !data.reportIds || typeof data.reportIds !== 'object' || Array.isArray(data.reportIds)) throw problem('Invalid persisted handoff state');
    if (data.latestReport) {
      const { reportId, instanceId: reportInstance, workEpoch, recordedAt, ...value } = data.latestReport;
      normalizeReport(value);
      if (reportInstance !== data.instanceId || value.manifestRevision !== data.manifestRevision || workEpoch > data.workEpoch) throw problem('Invalid persisted handoff report binding');
    }
  }
  const current = () => data?.instanceId === instanceId;
  const commit = (next) => { writeJson(path, next); data = next; };
  return {
    prepare(value, expectedRevision, runtime) {
      if (runtime.state !== 'idle') throw problem('Handoff preparation requires an idle instance', 409);
      const manifest = normalizeManifest(value);
      const revision = digest(JSON.stringify(manifest));
      if (current() && data.manifestRevision === revision) return;
      if (data && expectedRevision !== data.manifestRevision) throw problem('Expected manifest revision does not match; inspect before updating or rebinding', 409);
      if (!data && expectedRevision !== null) throw problem('First manifest requires expectedRevision=null', 409);
      commit({ version: 1, sessionId, instanceId, manifestRevision: revision, manifest,
        workEpoch: 0, settledEpoch: 0, latestReport: null, reportIds: data?.reportIds || {}, preparedAt: now() });
    },
    event(name) {
      if (!current()) return;
      if (name === 'agent_start') commit({ ...data, workEpoch: data.workEpoch + 1 });
      if (name === 'agent_settled') commit({ ...data, settledEpoch: data.workEpoch });
    },
    report(id, value) {
      id = validateId(id);
      const report = normalizeReport(value);
      if (!current()) throw problem('No handoff bound to this instance; prepare or explicitly rebind first', 409);
      if (report.manifestRevision !== data.manifestRevision) throw problem('Stale manifest revision; read edda_handoff again', 409);
      const fingerprint = digest(JSON.stringify(report));
      const previous = data.reportIds[id];
      if (previous) {
        if (previous.fingerprint !== fingerprint) throw problem('Report ID conflict', 409);
        if (previous.receipt.instanceId !== instanceId || previous.receipt.workEpoch !== data.workEpoch) throw problem('Report ID belongs to a previous instance/work epoch', 409);
        return previous.receipt;
      }
      if (!data.workEpoch || data.settledEpoch === data.workEpoch) throw problem('Reports must belong to an active work epoch', 409);
      if (Object.keys(data.reportIds).length >= 1000) throw problem('Report capacity reached; preserve this session and start a new one', 409);
      const recordedAt = now();
      const receipt = { reportId: id, sessionId, instanceId, manifestRevision: data.manifestRevision,
        workEpoch: data.workEpoch, recordedAt, status: 'recorded', acceptance: 'unverified' };
      commit({ ...data, latestReport: { ...report, reportId: id, instanceId, workEpoch: data.workEpoch, recordedAt },
        reportIds: { ...data.reportIds, [id]: { fingerprint, receipt } } });
      return receipt;
    },
    context(runtime, budget = 16384) {
      const base = { version: 1, sessionId, instanceId, role: 'supervisor', authority: 'declared_context_only', acceptance: 'unverified', notice };
      if (!data) return fitContext({ ...base, status: 'not_prepared', attention: 'needs_manifest' }, budget);
      if (!current()) return fitContext({ ...base, status: 'stale_binding', attention: 'needs_rebind', manifestRevision: data.manifestRevision,
        previousInstanceId: data.instanceId }, budget);
      const report = data.latestReport?.workEpoch === data.workEpoch ? data.latestReport : null;
      let attention = 'not_started';
      if (data.workEpoch) {
        if (runtime.state !== 'idle' && runtime.state !== 'stopped') attention = runtime.state === 'waiting_user' ? 'waiting_user' : 'running';
        else if (!report || report.reportedState === 'working') attention = 'missing_report';
        else attention = { waiting_decision: 'decision_required', waiting_dependency: 'dependency_wait',
          completed: 'completion_pending', failed: 'failure_reported', paused: 'paused_reported', unknown: 'missing_report' }[report.reportedState];
      }
      const view = { ...base, status: 'ready', attention, manifestRevision: data.manifestRevision, manifest: data.manifest,
        runtime: { state: runtime.state, observedAt: runtime.heartbeatAt, lastEvent: runtime.lastEvent },
        workEpoch: data.workEpoch, report };
      return fitContext(view, budget);
    },
  };
}
