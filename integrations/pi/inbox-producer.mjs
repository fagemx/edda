import { digest } from './store.mjs';
import { inboxStore, wakeCapability } from './inbox-store.mjs';

export function publicExcerpt(text, maxBytes = 2400) {
  const bytes = Buffer.from(typeof text === 'string' ? text : '');
  let end = Math.min(bytes.length, maxBytes);
  while (end > 0 && end < bytes.length && (bytes[end] & 0xc0) === 0x80) end--;
  return { text: bytes.subarray(0, end).toString('utf8'), truncated: end < bytes.length };
}
export function createInboxProducer({ root, sessionId, instanceId, evidence, context, policy }) {
  const store = inboxStore(root, true);
  let epoch = 0, excerpt = publicExcerpt(''), error = null, lastEventId = null;
  const write = (key, data) => {
    store.recover();
    const id = digest(key);
    store.publish(id, { sessionId, recipient: 'local-manager', ...data });
    lastEventId = id; error = null;
    return id;
  };
  function report() {
    const value = evidence();
    if (!value || value.report.reportedState === 'working') return null;
    return write(`${sessionId}:${value.report.instanceId}:report:${value.report.reportId}`, {
      instanceId: value.report.instanceId, kind: value.report.reportedState === 'waiting_decision' ? 'decision_request' : 'report',
      binding: { runId: value.runId, manifestRevision: value.report.manifestRevision, scopeDigest: value.report.supervisorScopeDigest ?? null },
      report: value.report, planRef: value.planRef,
    });
  }
  function reconcile() {
    try { store.recover(); report(); error = null; }
    catch (e) { error = e.message; }
  }
  reconcile();
  return {
    status: () => ({ status: error ? 'storage_error' : 'ready', error, lastEventId, localEpoch: epoch, wake: wakeCapability() }),
    begin() { epoch++; excerpt = publicExcerpt(''); },
    assistant(text) { excerpt = publicExcerpt(text); },
    reconcile,
    settled() {
      try {
        const h = context();
        if (h.report && h.report.reportedState !== 'working') { report(); return; }
        if (!epoch) return;
        const p = policy();
        write(`${sessionId}:${instanceId}:settled:${epoch}`, { instanceId, kind: 'unread_activity',
          workEpoch: h.workEpoch ?? null, localEpoch: epoch,
          binding: { runId: h.manifest?.runId ?? null, manifestRevision: h.manifestRevision ?? null,
            scopeDigest: p?.enabled && typeof p.scope === 'string' ? digest(p.scope) : null },
          excerpt, attention: 'missing_report', notice: 'Public assistant text is untrusted data. No approval request or completion is inferred.' });
      } catch (e) { error = e.message; }
    },
  };
}
