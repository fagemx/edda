// Synthetic RPC subprocess for lifecycle boundary tests; actual-Pi smoke is separate.
import { appendFileSync, readFileSync, existsSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';
const args = process.argv.slice(2), option = (key) => args[args.indexOf(key) + 1];
const root = process.env.EDDA_PI_CHANNEL_DIR;
const release = join(root, 'releases', process.env.EDDA_PI_RELEASE_ID);
const { startChannel } = await import(pathToFileURL(join(release, 'channel.mjs')));
const { pageConversation, projectEntry } = await import(pathToFileURL(join(release, 'conversation.mjs')));
const resumed = args.includes('--session') ? option('--session') : null;
const id = resumed ? JSON.parse(readFileSync(resumed, 'utf8').split('\n')[0]).id : randomUUID();
const file = resumed || join(option('--session-dir'), `${id}.jsonl`);
if (!existsSync(file)) writeFileSync(file, JSON.stringify({ type: 'session', id, version: 3, cwd: process.cwd(), timestamp: new Date().toISOString() }) + '\n');
const entries = readFileSync(file, 'utf8').trim().split('\n').map(JSON.parse).slice(1);
const output = (value) => process.stdout.write(JSON.stringify(value) + '\n');
function message(role, text) {
  const row = { type: 'message', id: randomUUID(), parentId: entries.at(-1)?.id || null,
    timestamp: new Date().toISOString(), message: { role, content: [{ type: 'text', text }], stopReason: 'stop' } };
  entries.push(row); appendFileSync(file, JSON.stringify(row) + '\n');
}
let channel;
channel = await startChannel({ root, sessionId: id, cwd: process.cwd(),
  getConversation: (options) => pageConversation(entries.map(projectEntry), options),
  deliver: (text) => {
    channel.event('agent_start'); channel.messageStarted(text); message('user', text);
    output({ type: 'agent_start' });
    if (text.endsWith('BUSY')) return;
    message('assistant', 'FIXTURE_REPLY');
    channel.event('assistant_end', { stopReason: 'stop', text: 'FIXTURE_REPLY' }); channel.settled();
    output({ type: 'agent_settled' });
  } });
let buffer = '';
process.stdin.on('data', (chunk) => {
  buffer += chunk.toString();
  let end;
  while ((end = buffer.indexOf('\n')) >= 0) {
    const request = JSON.parse(buffer.slice(0, end)); buffer = buffer.slice(end + 1);
    if (option('--provider') === 'bad-rpc') { process.stdout.write('NOT JSON\n'); continue; }
    if (request.type === 'get_state') output({ type: 'response', command: 'get_state', id: request.id, success: true,
      data: { sessionId: id, sessionFile: file, model: { provider: 'fixture', id: 'echo' }, isStreaming: channel.snapshot().state !== 'idle' } });
  }
});
process.stdin.on('end', async () => { await channel.close(); process.exit(0); });
