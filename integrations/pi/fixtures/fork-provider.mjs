import { createAssistantMessageEventStream } from '@earendil-works/pi-ai/compat';
export default function (pi) {
  pi.registerProvider('edda-fork-fixture', {
    api: 'edda-fork-fixture-api', baseUrl: 'http://127.0.0.1:1', apiKey: 'offline',
    models: [{ id: 'echo', name: 'Fork fixture', reasoning: false, input: ['text'],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 200000, maxTokens: 4096 }],
    streamSimple(model, context) {
      const stream = createAssistantMessageEventStream();
      const texts = context.messages.filter((m) => m.role === 'user').map((m) => typeof m.content === 'string' ? m.content : m.content.filter((p) => p.type === 'text').map((p) => p.text).join('\n'));
      if (texts.at(-1) === 'HANG') return stream; // Deliberate offline timeout fixture.
      const message = { role: 'assistant', content: [{ type: 'text', text: JSON.stringify({ seed: texts.some((t) => t === 'SEED_86cb'), child: texts.some((t) => t === 'CHILD'), parentLater: texts.some((t) => t === 'PARENT_LATER') }) }],
        api: model.api, provider: model.provider, model: model.id, stopReason: 'stop', timestamp: Date.now(),
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
      if (texts.at(-1) === 'TOOLS') message.content = [{ type: 'text', text: JSON.stringify({ tools: (context.tools || []).map((t) => t.name).sort() }) }];
      setTimeout(() => { stream.push({ type: 'start', partial: message }); stream.push({ type: 'done', reason: 'stop', message }); stream.end(); }, 50);
      return stream;
    },
  });
}
