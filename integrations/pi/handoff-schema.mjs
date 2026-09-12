export const problem = (message, status = 400) => Object.assign(new Error(message), { status });
export function fitContext(value, budget = 16384) {
  budget = Number(budget);
  if (!Number.isInteger(budget) || budget < 512 || budget > 32768) throw problem('Context budget must be 512..32768 bytes');
  const result = { ...value, serializedBytes: 0 };
  for (let i = 0; i < 4; i++) result.serializedBytes = Buffer.byteLength(JSON.stringify(result));
  if (result.serializedBytes > budget) return { sessionId: value.sessionId, instanceId: value.instanceId,
    status: 'needs_context', attention: 'context_over_budget', requiredBytes: result.serializedBytes, budgetBytes: budget };
  return result;
}
const text = (value, name, max = 1200) => {
  if (typeof value !== 'string' || !value.trim() || value.length > max) throw problem(`Invalid ${name}`);
  return value;
};
function object(value, keys, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw problem(`Invalid ${name}`);
  for (const key of Object.keys(value)) if (!keys.includes(key)) throw problem(`Unknown ${name} field: ${key}`);
}
function list(value, name, mapper, max = 10) {
  if (!Array.isArray(value) || value.length > max) throw problem(`Invalid ${name}`);
  return value.map((item) => mapper(item, name));
}
function reference(value, name) {
  object(value, ['uri', 'revision'], name);
  return { uri: text(value.uri, `${name}.uri`, 500), revision: text(value.revision, `${name}.revision`, 200) };
}
const refs = (value, name) => list(value, name, reference, 8);
const shortList = (value, name) => list(value, name, (v, n) => text(v, n, 300));

export function normalizeManifest(value) {
  object(value, ['version', 'runId', 'role', 'goal', 'planRef', 'doneWhen', 'scope'], 'manifest');
  if (value.version !== 1) throw problem('Unsupported manifest version');
  if (!['controller', 'worker'].includes(value.role)) throw problem('Invalid manifest role');
  object(value.scope, ['allowed', 'excluded', 'reserved', 'authorityRefs', 'taskPaths'], 'scope');
  const result = { version: 1, runId: text(value.runId, 'runId', 128), role: value.role,
    goal: text(value.goal, 'goal'), planRef: reference(value.planRef, 'planRef'),
    doneWhen: shortList(value.doneWhen, 'doneWhen'), scope: {
      allowed: shortList(value.scope.allowed, 'allowed'), excluded: shortList(value.scope.excluded, 'excluded'),
      reserved: shortList(value.scope.reserved, 'reserved'), authorityRefs: refs(value.scope.authorityRefs, 'authorityRefs'),
      ...(value.scope.taskPaths === undefined ? {} : { taskPaths: shortList(value.scope.taskPaths, 'taskPaths') }),
    } };
  if (!result.doneWhen.length || !result.scope.allowed.length || !result.scope.authorityRefs.length) throw problem('Manifest needs completion criteria, allowed scope and authority source references');
  if (Buffer.byteLength(JSON.stringify(result)) > 8192) throw problem('Manifest exceeds 8192 bytes; supply a bounded management brief', 413);
  return result;
}

export const reportStates = ['working', 'waiting_decision', 'waiting_dependency', 'completed', 'failed', 'paused', 'unknown'];
export function normalizeReport(value) {
  object(value, ['manifestRevision', 'reportedState', 'stage', 'summary', 'nextStep', 'evidence', 'dependencies', 'decision'], 'report');
  if (typeof value.manifestRevision !== 'string' || !/^[0-9a-f]{64}$/.test(value.manifestRevision)) throw problem('Invalid manifest revision');
  if (!reportStates.includes(value.reportedState)) throw problem('Invalid reportedState');
  const result = { manifestRevision: value.manifestRevision, reportedState: value.reportedState,
    stage: text(value.stage, 'stage', 200), summary: text(value.summary, 'summary'), nextStep: text(value.nextStep, 'nextStep'),
    evidence: refs(value.evidence, 'evidence'), dependencies: refs(value.dependencies, 'dependencies') };
  if (value.decision !== undefined) {
    object(value.decision, ['question', 'requestedAction', 'resource', 'recommendation'], 'decision');
    result.decision = { question: text(value.decision.question, 'decision.question'),
      requestedAction: text(value.decision.requestedAction, 'decision.requestedAction', 200),
      resource: text(value.decision.resource, 'decision.resource', 500),
      recommendation: text(value.decision.recommendation, 'decision.recommendation') };
  }
  if (result.reportedState === 'waiting_decision' && !result.decision) throw problem('waiting_decision needs a concrete decision request');
  if (result.reportedState !== 'waiting_decision' && result.decision) throw problem('decision is only valid for waiting_decision');
  if (result.reportedState === 'waiting_dependency' && !result.dependencies.length) throw problem('waiting_dependency needs dependencies');
  if (result.reportedState === 'completed' && !result.evidence.length) throw problem('completed claim needs evidence');
  if (Buffer.byteLength(JSON.stringify(result)) > 8192) throw problem('Report exceeds 8192 bytes', 413);
  return result;
}

// Pi accepts JSON schemas; runtime validation above also checks cross-field rules.
const string = { type: 'string', minLength: 1, maxLength: 1200 };
const refSchema = { type: 'object', additionalProperties: false, required: ['uri', 'revision'],
  properties: { uri: { ...string, maxLength: 500 }, revision: { ...string, maxLength: 200 } } };
export const reportSchema = { type: 'object', additionalProperties: false,
  required: ['manifestRevision', 'reportedState', 'stage', 'summary', 'nextStep', 'evidence', 'dependencies'],
  properties: {
    manifestRevision: { type: 'string', pattern: '^[0-9a-f]{64}$' }, reportedState: { type: 'string', enum: reportStates },
    stage: { ...string, maxLength: 200 }, summary: string, nextStep: string,
    evidence: { type: 'array', maxItems: 8, items: refSchema }, dependencies: { type: 'array', maxItems: 8, items: refSchema },
    decision: { type: 'object', additionalProperties: false, required: ['question', 'requestedAction', 'resource', 'recommendation'],
      properties: { question: string, requestedAction: { ...string, maxLength: 200 }, resource: { ...string, maxLength: 500 }, recommendation: string } },
  } };
