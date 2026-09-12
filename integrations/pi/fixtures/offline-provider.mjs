// Test fixture only: real Pi runtime, deterministic response, zero network/model spend.
import { createAssistantMessageEventStream } from '@earendil-works/pi-ai/compat';

export default function (pi) {
  pi.registerProvider('edda-offline-test', {
    api: 'edda-offline-test-api', baseUrl: 'http://127.0.0.1:1', apiKey: process.env.EDDA_PI_SMOKE_REJECT === '1' ? undefined : 'test-only',
    models: [{ id: 'echo', name: 'Offline channel test', reasoning: false, input: ['text'],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 200000, maxTokens: 4096 }],
    streamSimple(model, context) {
      const stream = createAssistantMessageEventStream();
      const user = [...context.messages].reverse().find((m) => m.role === 'user');
      const text = typeof user?.content === 'string' ? user.content : user?.content?.filter((p) => p.type === 'text').map((p) => p.text).join('\n');
      const answer = text?.endsWith('ASK_OFFLINE_PERMISSION') ? 'Please explicitly reply APPROVE_OFFLINE_TASK.' :
        text?.endsWith('APPROVE_OFFLINE_TASK') ? 'OFFLINE_TASK_STARTED: completed the synthetic operation.' : `OFFLINE_ACK: ${text}`;
      const message = { role: 'assistant', content: [{ type: 'text', text: answer }],
        api: model.api, provider: model.provider, model: model.id, stopReason: 'stop', timestamp: Date.now(),
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
      setTimeout(() => {
        stream.push({ type: 'start', partial: message });
        stream.push({ type: 'done', reason: 'stop', message });
        stream.end();
      }, 200);
      return stream;
    },
  });
}
