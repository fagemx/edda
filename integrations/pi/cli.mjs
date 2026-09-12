#!/usr/bin/env node
import { readFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { defaultRoot, recover, validateId } from './store.mjs';
import { listSessions, requestSession, getReceipt, inspectSession } from './client.mjs';
import { enroll, watch, checkpoint, reply } from './supervision.mjs';

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
  node integrations/pi/cli.mjs reply SESSION_ID --to CURSOR --message TEXT
  node integrations/pi/cli.mjs checkpoint SESSION_ID --cursor CURSOR --action observed|working|waiting_user|complete|paused --note TEXT

JSON stdout; diagnostics stderr. EDDA_PI_CHANNEL_DIR overrides the private root.
Supervision commands are tools for an authorized controller, not a decision engine.
No automatic resume, retries or remote network listener. Keep the message ID.
`;

async function main(args) {
  const [command, ...rest] = args;
  if (!command || command === '--help') { process.stdout.write(help); return; }
  const root = defaultRoot();
  const positional = [];
  const options = {};
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    if (!arg.startsWith('--')) { positional.push(arg); continue; }
    if (options[arg] !== undefined || rest[i + 1] === undefined || rest[i + 1].startsWith('--')) throw new Error(`Missing or duplicate option ${arg}`);
    options[arg] = rest[++i];
  }
  const allowed = {
    list: [], status: [], send: ['--message', '--message-file', '--id', '--sender', '--mode'],
    receipt: ['--id'], recover: ['--instance'],
    conversation: ['--after', '--limit'], enroll: ['--scope'], watch: [],
    reply: ['--to', '--message', '--message-file'], checkpoint: ['--cursor', '--action', '--note'],
  }[command];
  if (!allowed || Object.keys(options).some((key) => !allowed.includes(key))) throw new Error('Unknown command or option; use --help');
  if (positional.length !== (['list', 'watch'].includes(command) ? 0 : 1)) throw new Error('Use the exact session ID; see --help');
  const sessionId = positional[0];
  let result;
  if (command === 'list') result = await listSessions(root);
  if (command === 'status') result = await requestSession(root, sessionId, '/status');
  if (command === 'receipt') result = await getReceipt(root, sessionId, validateId(options['--id']));
  if (command === 'recover') result = recover(root, sessionId, options['--instance']);
  if (command === 'conversation') result = await inspectSession(root, sessionId, { after: options['--after'], limit: options['--limit'] });
  if (command === 'enroll') result = await enroll(root, sessionId, options['--scope']);
  if (command === 'watch') result = await watch(root);
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
  if (result?.status === 'unknown' || result?.status === 'failed') process.exitCode = 2;
}

main(process.argv.slice(2)).catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});
