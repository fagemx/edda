// Test fixture only: real Pi runtime, deterministic response, zero network/model spend.
import { createAssistantMessageEventStream } from '@earendil-works/pi-ai/compat';

export default function (pi) {
  pi.registerProvider('edda-offline-test', {
    api: 'edda-offline-test-api', baseUrl: 'http://127.0.0.1:1', apiKey: process.env.EDDA_PI_SMOKE_REJECT === '1' ? undefined : 'test-only',
    models: [{ id: 'echo', name: 'Offline channel test', reasoning: false, input: ['text'],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 200000, maxTokens: 4096 }],
    streamSimple(model, context) {
      const stream = createAssistantMessageEventStream();
      const messageText = (m) => typeof m?.content === 'string' ? m.content : m?.content?.filter((p) => p.type === 'text').map((p) => p.text).join('\n');
      // Pi can render an injected custom context message as a user message.
      // The fixture must select the actual channel envelope, not that context.
      const user = [...context.messages].reverse().find((m) => m.role === 'user' && messageText(m)?.startsWith('[Edda message '));
      const text = messageText(user);
      const answer = text?.endsWith('ASK_OFFLINE_PERMISSION') ? 'Please explicitly reply APPROVE_OFFLINE_TASK.' :
        text?.endsWith('APPROVE_OFFLINE_TASK') ? 'OFFLINE_TASK_STARTED: completed the synthetic operation.' : `OFFLINE_ACK: ${text}`;
      const message = { role: 'assistant', content: [{ type: 'text', text: answer }],
        api: model.api, provider: model.provider, model: model.id, stopReason: 'stop', timestamp: Date.now(),
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
      if (process.env.EDDA_PI_SMOKE_HANDOFF === '1') {
        const last = context.messages.at(-1);
        if (last?.role === 'toolResult' && last.toolName === 'edda_report') {
          message.content = [{ type: 'text', text: `OFFLINE_ACK: ${text}; handoff report recorded, not independently verified.` }];
        } else {
          let name = 'edda_handoff';
          let args = {};
          if (last?.role === 'toolResult' && last.toolName === 'edda_handoff') {
            const card = JSON.parse(last.content.filter((p) => p.type === 'text').map((p) => p.text).join('\n'));
            const completed = text?.endsWith('CHANNEL_SMOKE_TWO');
            name = 'edda_report';
            args = { manifestRevision: card.manifestRevision, reportedState: completed ? 'completed' : 'waiting_decision',
              stage: 'offline-demo', summary: completed ? 'Synthetic operation complete' : 'Waiting for a fixture decision',
              nextStep: completed ? 'Independent verification' : 'Read the requested decision', dependencies: [],
              evidence: completed ? [{ uri: 'fixture://result', revision: 'v1' }] : [],
              ...(completed ? {} : { decision: { question: 'May the synthetic fixture continue?', requestedAction: 'fixture_continue',
                resource: 'offline-only', recommendation: 'Continue under the fixture scope' } }) };
          }
          message.content = [{ type: 'toolCall', id: `handoff-${Date.now()}`, name, arguments: args }];
          message.stopReason = 'toolUse';
        }
      }
      setTimeout(() => {
        stream.push({ type: 'start', partial: message });
        stream.push({ type: 'done', reason: message.stopReason, message });
        stream.end();
      }, 200);
      return stream;
    },
  });
}
