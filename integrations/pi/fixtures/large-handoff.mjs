// Pure test data: exercise valid individually bounded inputs whose combined
// context requires an explicit larger budget. No real authority/evidence.
const ref = (uriLength = 500, revisionLength = 200) => ({ uri: 'u'.repeat(uriLength), revision: 'r'.repeat(revisionLength) });
function fill(value, key, max = 1200) {
  const count = 8100 - Buffer.byteLength(JSON.stringify(value));
  if (count < 1 || count > max) throw new Error(`Large fixture field ${key} needs ${count} characters`);
  value[key] = 'x'.repeat(count);
  return value;
}
export function largeManifest() {
  return fill({ version: 1, runId: 'large-offline-fixture', role: 'controller', goal: '', planRef: ref(400, 100),
    doneWhen: Array(8).fill('d'.repeat(200)), scope: {
      allowed: Array(8).fill('a'.repeat(200)), excluded: Array(8).fill('e'.repeat(180)),
      reserved: Array(6).fill('r'.repeat(180)), authorityRefs: [ref(400, 100)],
    } }, 'goal');
}
export function largeReport(manifestRevision, completed) {
  const value = { manifestRevision, reportedState: completed ? 'completed' : 'waiting_decision',
    stage: 's'.repeat(200), summary: '', nextStep: 'n'.repeat(1200),
    evidence: Array.from({ length: completed ? 7 : 4 }, () => ref()),
    dependencies: completed ? [ref(200, 100)] : [],
    ...(completed ? {} : { decision: { question: 'q'.repeat(1000), requestedAction: 'a'.repeat(200),
      resource: 'r'.repeat(500), recommendation: 'p'.repeat(1000) } }),
  };
  return fill(value, 'summary');
}
