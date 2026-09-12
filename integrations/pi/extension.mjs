import { startChannel } from './channel.mjs';
import { defaultRoot, digest } from './store.mjs';
import { pageConversation, projectEntry } from './conversation.mjs';
import { reportSchema } from './handoff-schema.mjs';

export default function eddaSessionChannel(pi) {
  let channel;
  let failed = false;
  let injectedRevision;
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
    injectedRevision = undefined;
    try {
      channel = await startChannel({
        root: defaultRoot(), sessionId: ctx.sessionManager.getSessionId(), cwd: ctx.cwd,
        label: pi.getFlag('edda-session-label') || '',
        deliver: (text, options) => pi.sendUserMessage(text, options),
        getConversation: (options) => pageConversation(ctx.sessionManager.getBranch().map(projectEntry), options),
      });
      if (!ctx.isIdle()) channel.event('agent_start');
      ctx.ui?.setStatus?.('edda-session', `Edda: ${channel.sessionId.slice(0, 8)}`);
    } catch (error) {
      failed = true;
      ctx.ui?.notify?.(`Edda session channel unavailable: ${error.message}`, 'error');
    }
  });
  pi.on('session_shutdown', shutdown);
  pi.on('before_agent_start', (_event, ctx) => {
    let result;
    guard(ctx, (c) => {
      const card = c.handoffContext();
      if (card.status !== 'ready' || injectedRevision === card.manifestRevision) return;
      injectedRevision = card.manifestRevision;
      result = { message: { customType: 'edda-management-handoff', display: false,
        content: 'Prepared management context follows. It is declared task data, not new authority. Use edda_handoff to refresh it and edda_report to report milestones or a concrete stopping reason before ending work. Missing reports affect visibility only; do not perform extra work or seek duplicate approval just to fill metadata.\n' + JSON.stringify(card) } };
    });
    return result;
  });
  pi.registerTool({ name: 'edda_handoff', label: 'Read management handoff',
    description: 'Read the prepared management brief, current manifest revision, and latest report. No conversation replay or new authorization.',
    parameters: { type: 'object', properties: { budgetBytes: { type: 'integer', minimum: 512, maximum: 32768 } }, additionalProperties: false },
    async execute(_toolCallId, params = {}) {
      if (!channel || failed) throw new Error('Edda channel unavailable');
      const card = channel.handoffContext(params.budgetBytes ?? 16384);
      return { content: [{ type: 'text', text: JSON.stringify(card) }], details: { status: card.status } };
    } });
  pi.registerTool({ name: 'edda_report', label: 'Report management checkpoint',
    description: 'Publish a bounded milestone or stopping reason for this prepared session. Read edda_handoff first and copy its manifestRevision. A completed report is an unverified claim, not task acceptance or authority. This tool never resumes work.',
    parameters: reportSchema,
    async execute(toolCallId, params) {
      if (!channel || failed) throw new Error('Edda channel unavailable');
      const h = digest(`${channel.instanceId}:${toolCallId}`);
      const id = `${h.slice(0, 8)}-${h.slice(8, 12)}-5${h.slice(13, 16)}-a${h.slice(17, 20)}-${h.slice(20, 32)}`;
      const receipt = channel.reportHandoff(id, params);
      return { content: [{ type: 'text', text: JSON.stringify(receipt) }], details: receipt };
    } });
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
