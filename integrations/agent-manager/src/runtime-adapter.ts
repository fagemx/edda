import type { AgentBinding, DiscoveryReport, PiAdapter, SendRequest, OperationView } from './contracts.js';
import { ChannelAdapter } from './pi-adapter.js';
import { CodexAdapter } from './codex-adapter.js';
import type { ManagerStore } from './store.js';

/** Explicit transport routing. Codex recorded evidence never acquires Pi send authority. */
export class RuntimeAdapter implements PiAdapter {
  constructor(private pi: PiAdapter, private codex: PiAdapter) {}
  static async create(store: ManagerStore, piRoot?: string): Promise<RuntimeAdapter> {
    return new RuntimeAdapter(await ChannelAdapter.create(piRoot), new CodexAdapter(store));
  }
  private for(binding: AgentBinding): PiAdapter { return binding.transport === 'codex' ? this.codex : this.pi; }
  validateMessage(request: SendRequest): void { this.pi.validateMessage?.(request); }
  defaultRegistryRoot(): string | null { return this.pi.defaultRegistryRoot?.() ?? null; }
  async discover(registryRoots: string[]): Promise<DiscoveryReport> { return this.pi.discover?.(registryRoots) ?? { runs: [], failures: [] }; }
  observe(binding: AgentBinding) { return this.for(binding).observe(binding); }
  conversation(binding: AgentBinding, after?: string) { return this.for(binding).conversation(binding, after); }
  send(binding: AgentBinding, request: SendRequest) { return this.for(binding).send(binding, request); }
  receipt(binding: AgentBinding, operation: OperationView) { return this.for(binding).receipt(binding, operation); }
}
