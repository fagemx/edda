import { startChannel } from './channel.mjs';
import { defaultRoot } from './store.mjs';

export default function eddaSessionChannel(pi) {
  let channel;
  let failed = false;
  pi.registerFlag('edda-session-label', { description: 'Display label for the local Edda session channel', type: 'string', default: '' });

  async function shutdown() {
    const prior = channel;
    channel = undefined;
    if (prior) await prior.close();
  }
  function guard(ctx, action) {
    if (!channel || failed) return;
    try { action(channel); }
    catch {
      failed = true;
      ctx.ui?.notify?.('Edda session channel storage failed; inspect the channel before sending more messages.', 'error');
      // Stop accepting messages when receipt persistence is unavailable.
      void shutdown().catch(() => {});
    }
  }
  pi.on('session_start', async (_event, ctx) => {
    await shutdown();
    failed = false;
    try {
      channel = await startChannel({
        root: defaultRoot(), sessionId: ctx.sessionManager.getSessionId(), cwd: ctx.cwd,
        label: pi.getFlag('edda-session-label') || '',
        deliver: (text, options) => pi.sendUserMessage(text, options),
      });
      if (!ctx.isIdle()) channel.event('agent_start');
      ctx.ui?.setStatus?.('edda-session', `Edda: ${channel.sessionId.slice(0, 8)}`);
    } catch (error) {
      failed = true;
      ctx.ui?.notify?.(`Edda session channel unavailable: ${error.message}`, 'error');
    }
  });
  pi.on('session_shutdown', shutdown);
  for (const name of ['agent_start', 'turn_start', 'tool_execution_start', 'tool_execution_update',
    'tool_execution_end', 'ui_prompt_start', 'ui_prompt_end']) {
    pi.on(name, (event, ctx) => guard(ctx, (c) => c.event(name, event)));
  }
  pi.on('message_start', (event, ctx) => guard(ctx, (c) => {
    if (event.message.role !== 'user') return;
    const content = event.message.content;
    const text = typeof content === 'string' ? content : content.filter((p) => p.type === 'text').map((p) => p.text).join('\n');
    c.messageStarted(text);
  }));
  pi.on('message_end', (event, ctx) => guard(ctx, (c) => {
    if (event.message.role === 'assistant') c.event('assistant_end', { stopReason: event.message.stopReason });
  }));
  pi.on('agent_settled', (_event, ctx) => guard(ctx, (c) => c.settled()));
  pi.registerCommand('edda-session', {
    description: 'Show this Pi session channel identity and state',
    handler: async (_args, ctx) => {
      ctx.ui.notify(channel ? JSON.stringify(channel.snapshot(), null, 2) : 'Edda session channel is unavailable', channel ? 'info' : 'error');
    },
  });
}
