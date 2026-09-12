import { createAssistantMessageEventStream } from '@earendil-works/pi-ai/compat';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
const exec = promisify(execFile);
const textOf = (message) => typeof message?.content === 'string' ? message.content : (message?.content || []).filter((p) => p.type === 'text').map((p) => p.text).join('\n');
function payload(messages, marker) {
  const source = [...messages].reverse().map(textOf).find((text) => text?.includes(marker));
  return source ? JSON.parse(source.slice(source.indexOf(marker) + marker.length).trimStart().split('\n')[0]) : null;
}
export default function (pi) {
  pi.registerTool({ name: 'fixture_finish_task', label: 'Complete owned synthetic task', description: 'Test fixture only.',
    parameters: { type: 'object', properties: { project: { type: 'string' }, taskId: { type: 'string' } }, required: ['project', 'taskId'] },
    async execute(_id, args, _signal, _update, ctx) {
      const run = (...argv) => exec(process.env.EDDA_BIN, argv, { cwd: args.project, windowsHide: true, timeout: 15000 });
      const task = JSON.parse((await run('task', 'show', args.taskId, '--json')).stdout);
      if (task.status !== 'done') {
        if (task.status === 'ready') await run('task', 'start', args.taskId);
        const file = join(ctx.cwd, 'result.txt');
        await writeFile(file, 'FIXTURE_DONE\n', { flag: 'wx' });
        await run('task', 'done', args.taskId, '--receipt', `Synthetic fixture complete; evidence ${file}`);
      }
      return { content: [{ type: 'text', text: 'Synthetic task receipt recorded.' }] };
    } });
  pi.registerProvider('edda-supervisor-fixture', { api: 'edda-supervisor-fixture-api', baseUrl: 'http://127.0.0.1:1', apiKey: 'fixture',
    models: [{ id: 'echo', name: 'Supervisor fixture', reasoning: false, input: ['text'], contextWindow: 200000, maxTokens: 2048,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } }],
    streamSimple(model, context) {
      const stream = createAssistantMessageEventStream(), last = context.messages.at(-1);
      const packet = payload(context.messages, '[EDDA_SUPERVISOR_PACKET]');
      const assigned = payload(context.messages, '[EDDA_SUPERVISOR_TASK]');
      const continued = payload(context.messages, '[EDDA_SUPERVISOR_CONTINUE]');
      const message = { role: 'assistant', content: [{ type: 'text', text: 'Fixture complete' }], stopReason: 'stop',
        api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(),
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
      const call = (name, args) => { message.stopReason = 'toolUse'; message.content = [{ type: 'toolCall', id: `fixture-${Date.now()}`, name, arguments: args }]; };
      if (packet) {
        if (last?.role === 'toolResult' && last.toolName === 'edda_supervisor_decide') message.content = [{ type: 'text', text: 'Proposal recorded.' }];
        else if (last?.role === 'toolResult' && last.toolName === 'edda_supervisor_read') call('edda_supervisor_decide', {
          packetId: packet.id, taskId: packet.task.id, action: 'continue', reason: 'The existing operator instruction already authorizes the assigned fixture.' });
        else call('edda_supervisor_read', { taskId: packet.task.id });
      } else if (assigned) {
        if (last?.role === 'toolResult' && last.toolName === 'fixture_finish_task') message.content = [{ type: 'text', text: 'FIXTURE_TASK_DONE' }];
        else if (assigned.task.title.includes('ASK') && !continued) message.content = [{ type: 'text', text: 'Please confirm that the original assigned fixture work is approved before I continue.' }];
        else call('fixture_finish_task', { project: assigned.project, taskId: assigned.task.id });
      }
      setTimeout(() => { stream.push({ type: 'start', partial: message }); stream.push({ type: 'done', reason: message.stopReason, message }); stream.end(); }, 30);
      return stream;
    } });
}
