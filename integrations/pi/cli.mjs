#!/usr/bin/env node
import { readFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { defaultRoot, recover, validateId } from './store.mjs';
import { listSessions, requestSession, getReceipt, inspectSession, prepareHandoff } from './client.mjs';
import { enroll, watch, checkpoint, reply, managementBrief } from './supervision.mjs';
import { composeHandoff } from './compose.mjs';
import { followDependencies, dependencyStatus, unfollowDependencies, doctor } from './dependency-client.mjs';
import { adoptSession } from './adoption.mjs';
import { listInbox, readInbox, acknowledgeInbox, recordAuthorization, revokeAuthorization, respondInbox } from './inbox-manager.mjs';
import { wakeCapability } from './inbox-store.mjs';
import { readBoundedFile } from './compose-sources.mjs';
import { installRuntime } from './managed-store.mjs';
import { launchManaged, managedStatus, stopManaged, resumeManaged } from './managed-client.mjs';

const help = `Edda Pi session channel (same-user, same-machine)
  node integrations/pi/cli.mjs list
  node integrations/pi/cli.mjs status SESSION_ID
  node integrations/pi/cli.mjs send SESSION_ID --message TEXT [--id UUID] [--sender codex] [--mode followUp|steer]
  node integrations/pi/cli.mjs send SESSION_ID --message-file PATH [--id UUID]
  node integrations/pi/cli.mjs receipt SESSION_ID --id UUID
  node integrations/pi/cli.mjs recover SESSION_ID --instance UUID
  node integrations/pi/cli.mjs conversation SESSION_ID [--after ENTRY_ID] [--limit 20]
  node integrations/pi/cli.mjs enroll SESSION_ID --scope TEXT
  node integrations/pi/cli.mjs watch
  node integrations/pi/cli.mjs watch --conversation
  node integrations/pi/cli.mjs prepare SESSION_ID --manifest FILE [--expected REVISION]
  node integrations/pi/cli.mjs brief SESSION_ID [--budget-bytes 16384]
  node integrations/pi/cli.mjs compose --project PATH --task ID [--context FILE] [--output NEW_FILE] [--edda-bin PATH]
  node integrations/pi/cli.mjs reply SESSION_ID --to CURSOR --message TEXT
  node integrations/pi/cli.mjs checkpoint SESSION_ID --cursor CURSOR --action observed|working|waiting_user|complete|paused --note TEXT
  node integrations/pi/cli.mjs checkpoint SESSION_ID --action paused --note TEXT
  node integrations/pi/cli.mjs doctor [SESSION_ID]
  node integrations/pi/cli.mjs adopt SESSION_ID_OR_PREFIX --task ID [--project PATH] [--include ID,ID] [--context FILE] [--scope TEXT] [--notify] [--max-notifications 10] [--preview] [--expected REVISION]
  node integrations/pi/cli.mjs follow SESSION_ID --project PATH --tasks ID,ID [--scope TEXT] [--notify] [--max-notifications 10]
  node integrations/pi/cli.mjs dependencies SESSION_ID
  node integrations/pi/cli.mjs check-dependencies SESSION_ID
  node integrations/pi/cli.mjs unfollow SESSION_ID
  node integrations/pi/cli.mjs inbox [--consumer codex] [--limit 20] [--after EVENT_ID]
  node integrations/pi/cli.mjs inbox-read EVENT_ID [--consumer codex] [--budget-bytes 16384]
  node integrations/pi/cli.mjs inbox-ack EVENT_ID [--consumer codex]
  node integrations/pi/cli.mjs authorization-record EVENT_ID --record FILE [--consumer codex]
  node integrations/pi/cli.mjs authorization-revoke RECORD_ID [--consumer codex]
  node integrations/pi/cli.mjs inbox-respond EVENT_ID --message TEXT [--authorization RECORD_ID] [--consumer codex]
  node integrations/pi/cli.mjs inbox-respond EVENT_ID --message-file PATH [--authorization RECORD_ID]
  node integrations/pi/cli.mjs inbox-wake
  node integrations/pi/cli.mjs runtime-install
  node integrations/pi/cli.mjs launch --project PATH [--pi-entry FILE] [--provider NAME] [--model NAME] [--thinking LEVEL] [--prompt-file FILE] [--run-id UUID] [--extension FILE] [--agent-dir PATH] [--no-tools]
  node integrations/pi/cli.mjs run-status RUN_ID
  node integrations/pi/cli.mjs run-stop RUN_ID [--abort]
  node integrations/pi/cli.mjs run-resume RUN_ID

JSON stdout; diagnostics stderr. EDDA_PI_CHANNEL_DIR overrides the private root.
Supervision commands are tools for an authorized controller, not a decision engine.
No automatic process restart or message retry. Opt-in dependency alerts may start a model turn.
The listener is local only. Keep the message ID.
`;

async function main(args) {
  const [command, ...rest] = args;
  if (!command || command === '--help') { process.stdout.write(help); return; }
  const root = defaultRoot();
  const positional = [];
  const options = {};
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    if (['--conversation', '--notify', '--preview', '--no-tools', '--abort'].includes(arg) && options[arg] === undefined) { options[arg] = true; continue; }
    if (!arg.startsWith('--')) { positional.push(arg); continue; }
    if (options[arg] !== undefined || rest[i + 1] === undefined || rest[i + 1].startsWith('--')) throw new Error(`Missing or duplicate option ${arg}`);
    options[arg] = rest[++i];
  }
  const allowed = {
    list: [], status: [], send: ['--message', '--message-file', '--id', '--sender', '--mode'],
    receipt: ['--id'], recover: ['--instance'],
    conversation: ['--after', '--limit'], enroll: ['--scope'], watch: ['--conversation'],
    prepare: ['--manifest', '--expected'], brief: ['--budget-bytes'],
    compose: ['--project', '--task', '--context', '--output', '--edda-bin'],
    doctor: [], follow: ['--project', '--tasks', '--scope', '--notify', '--max-notifications'],
    adopt: ['--task', '--project', '--include', '--context', '--scope', '--notify', '--max-notifications', '--preview', '--expected'],
    dependencies: [], 'check-dependencies': [], unfollow: [],
    inbox: ['--consumer', '--limit', '--after'], 'inbox-read': ['--consumer', '--budget-bytes'], 'inbox-ack': ['--consumer'],
    'authorization-record': ['--record', '--consumer'], 'authorization-revoke': ['--consumer'],
    'inbox-respond': ['--message', '--message-file', '--authorization', '--consumer'], 'inbox-wake': [],
    'runtime-install': [], launch: ['--project', '--pi-entry', '--provider', '--model', '--thinking', '--prompt-file', '--run-id', '--extension', '--agent-dir', '--no-tools'],
    'run-status': [], 'run-stop': ['--abort'], 'run-resume': [],
    reply: ['--to', '--message', '--message-file'], checkpoint: ['--cursor', '--action', '--note'],
  }[command];
  if (!allowed || Object.keys(options).some((key) => !allowed.includes(key))) throw new Error('Unknown command or option; use --help');
  const counts = command === 'doctor' ? [0, 1] : [['list', 'watch', 'compose', 'inbox', 'inbox-wake', 'runtime-install', 'launch'].includes(command) ? 0 : 1];
  if (!counts.includes(positional.length)) throw new Error('Use the exact session ID; see --help');
  const sessionId = positional[0];
  let result;
  if (command === 'runtime-install') result = { status: 'installed', release: installRuntime(root), settingsChanged: false };
  if (command === 'launch') {
    const runId = validateId(options['--run-id'] || randomUUID());
    process.stderr.write(`Run ID: ${runId}\n`);
    result = await launchManaged(root, { runId, project: options['--project'], piEntry: options['--pi-entry'],
      provider: options['--provider'], model: options['--model'], prompt: options['--prompt-file'] ? (await readBoundedFile(options['--prompt-file'])).text : undefined,
      extensions: options['--extension'] ? [options['--extension']] : [], agentDir: options['--agent-dir'], noTools: options['--no-tools'] === true, thinking: options['--thinking'] });
  }
  if (command === 'run-status') result = await managedStatus(root, sessionId);
  if (command === 'run-stop') result = await stopManaged(root, sessionId, { abort: options['--abort'] === true });
  if (command === 'run-resume') result = await resumeManaged(root, sessionId);
  if (command === 'inbox') result = listInbox(root, { consumer: options['--consumer'], limit: options['--limit'], after: options['--after'] });
  if (command === 'inbox-read') result = await readInbox(root, sessionId, { consumer: options['--consumer'], budget: options['--budget-bytes'] });
  if (command === 'inbox-ack') result = acknowledgeInbox(root, sessionId, options['--consumer']);
  if (command === 'inbox-wake') result = wakeCapability();
  if (command === 'authorization-record') result = await recordAuthorization(root, sessionId,
    JSON.parse((await readBoundedFile(options['--record'])).text), options['--consumer']);
  if (command === 'authorization-revoke') result = revokeAuthorization(root, sessionId, options['--consumer']);
  if (command === 'inbox-respond') {
    if (Boolean(options['--message']) === Boolean(options['--message-file'])) throw new Error('Supply exactly one of --message or --message-file');
    result = await respondInbox(root, sessionId, { message: options['--message'] || (await readBoundedFile(options['--message-file'])).text,
      authorizationId: options['--authorization'], consumer: options['--consumer'] });
  }
  if (command === 'doctor') result = await doctor(root, sessionId);
  if (command === 'adopt') result = await adoptSession(root, sessionId, { id: options['--task'], project: options['--project'],
    include: options['--include']?.split(',').map((s) => s.trim()), contextFile: options['--context'], scope: options['--scope'],
    notify: options['--notify'] === true, maxNotifications: Number(options['--max-notifications'] ?? 10),
    preview: options['--preview'] === true, expectedRevision: options['--expected'] });
  if (command === 'follow') result = await followDependencies(root, sessionId, { project: options['--project'],
    taskIds: options['--tasks']?.split(',').map((s) => s.trim()), scope: options['--scope'],
    notify: options['--notify'] === true, maxNotifications: Number(options['--max-notifications'] || 10) });
  if (command === 'dependencies' || command === 'check-dependencies') result = await dependencyStatus(root, sessionId, command === 'check-dependencies');
  if (command === 'unfollow') result = await unfollowDependencies(root, sessionId);
  if (command === 'compose') result = await composeHandoff({ project: options['--project'], id: options['--task'],
    contextFile: options['--context'], output: options['--output'], root,
    eddaCommand: options['--edda-bin'] ? { file: options['--edda-bin'], args: [] } : undefined });
  if (command === 'list') result = await listSessions(root);
  if (command === 'status') result = await requestSession(root, sessionId, '/status');
  if (command === 'receipt') result = await getReceipt(root, sessionId, validateId(options['--id']));
  if (command === 'recover') result = recover(root, sessionId, options['--instance']);
  if (command === 'conversation') result = await inspectSession(root, sessionId, { after: options['--after'], limit: options['--limit'] });
  if (command === 'enroll') result = await enroll(root, sessionId, options['--scope']);
  if (command === 'watch') result = await watch(root, { withConversation: options['--conversation'] === true });
  if (command === 'brief') result = await managementBrief(root, sessionId, options['--budget-bytes'] || 16384);
  if (command === 'prepare') {
    if (!options['--manifest']) throw new Error('prepare requires --manifest FILE');
    result = await prepareHandoff(root, sessionId, JSON.parse(readFileSync(options['--manifest'], 'utf8')), options['--expected'] || null);
  }
  if (command === 'checkpoint') result = await checkpoint(root, sessionId, { cursor: options['--cursor'], action: options['--action'], note: options['--note'] });
  if (command === 'reply') {
    if (Boolean(options['--message']) === Boolean(options['--message-file'])) throw new Error('Supply exactly one of --message or --message-file');
    result = await reply(root, sessionId, { to: options['--to'], message: options['--message'] || readFileSync(options['--message-file'], 'utf8') });
  }
  if (command === 'send') {
    if (Boolean(options['--message']) === Boolean(options['--message-file'])) throw new Error('Supply exactly one of --message or --message-file');
    const id = validateId(options['--id'] || randomUUID());
    const message = options['--message'] || readFileSync(options['--message-file'], 'utf8');
    // Print the ID BEFORE sending so a transport failure never loses retry identity.
    process.stderr.write(`Message ID: ${id}\n`);
    result = await requestSession(root, sessionId, '/messages', { id, message,
      sender: options['--sender'] || 'codex', mode: options['--mode'] || 'followUp' });
  }
  process.stdout.write(JSON.stringify(result, null, 2) + '\n');
  if (['unknown', 'failed', 'needs_context', 'not_prepared', 'stale_binding', 'unavailable', 'needs_reload', 'needs_enrollment', 'not_registered',
    'ambiguous_session', 'invalid_selector', 'offline', 'busy', 'source_changed', 'needs_expected_revision', 'adoption_incomplete', 'unsupported',
    'runner_unreachable', 'launch_pending', 'stop_pending', 'exited'].includes(result?.status)) process.exitCode = 2;
}

main(process.argv.slice(2)).catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});
