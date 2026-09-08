import type { CallRequest, Usage } from "./types.js";

export interface HarnessCapabilityDescriptor {
  id: string;
  name: string;
  description: string;
  defaultAttached: boolean;
}

export interface HarnessCapabilityModule extends HarnessCapabilityDescriptor {
  prepare(request: CallRequest): Promise<CallRequest> | CallRequest;
  prepareWithUsage?(request: CallRequest): Promise<{ request: CallRequest; auxiliaryUsage?: Usage }> | { request: CallRequest; auxiliaryUsage?: Usage };
}

/**
 * Applies harness-side request transforms selected by the embedding client.
 * This is deliberately separate from Skill/MCP/Worker activation: those add
 * model-visible tools, while these modules alter Yeet's request pipeline.
 */
export class HarnessCapabilityRegistry {
  readonly #modules = new Map<string, HarnessCapabilityModule>();

  constructor(modules: Iterable<HarnessCapabilityModule> = []) {
    for (const module of modules) this.register(module);
  }

  register(module: HarnessCapabilityModule): this {
    const id = module.id.trim();
    if (!id) throw new TypeError("Harness capability id must not be empty");
    if (this.#modules.has(id)) throw new TypeError(`Duplicate harness capability: ${id}`);
    this.#modules.set(id, { ...module, id });
    return this;
  }

  list(): HarnessCapabilityDescriptor[] {
    return [...this.#modules.values()].map(({ prepare: _prepare, ...descriptor }) => descriptor);
  }

  defaultAttached(): string[] {
    return [...this.#modules.values()]
      .filter((module) => module.defaultAttached)
      .map((module) => module.id);
  }

  async prepare(request: CallRequest): Promise<CallRequest> {
    return (await this.prepareWithUsage(request)).request;
  }

  async prepareWithUsage(request: CallRequest): Promise<{ request: CallRequest; auxiliaryUsage?: Usage }> {
    const ids = request.attachedCapabilities ?? this.defaultAttached();
    let prepared = request;
    let auxiliaryUsage: Usage | undefined;
    const seen = new Set<string>();
    for (const id of ids) {
      if (seen.has(id)) continue;
      seen.add(id);
      const module = this.#modules.get(id);
      if (!module) throw new Error(`Unknown attached harness capability: ${id}`);
      if (module.prepareWithUsage) {
        const result = await module.prepareWithUsage(prepared);
        prepared = result.request;
        auxiliaryUsage = mergeUsage(auxiliaryUsage, result.auxiliaryUsage);
      } else {
        prepared = await module.prepare(prepared);
      }
    }
    return { request: prepared, ...(auxiliaryUsage ? { auxiliaryUsage } : {}) };
  }
}

function mergeUsage(lhs: Usage | undefined, rhs: Usage | undefined): Usage | undefined {
  if (!lhs) return rhs;
  if (!rhs) return lhs;
  const add = (a: number | undefined, b: number | undefined): number | undefined =>
    a === undefined && b === undefined ? undefined : (a ?? 0) + (b ?? 0);
  const values: Array<[keyof Usage, number | undefined]> = [
    ["inputTokens", add(lhs.inputTokens, rhs.inputTokens)],
    ["outputTokens", add(lhs.outputTokens, rhs.outputTokens)],
    ["totalTokens", add(lhs.totalTokens, rhs.totalTokens)],
    ["cachedInputTokens", add(lhs.cachedInputTokens, rhs.cachedInputTokens)],
    ["cacheWriteInputTokens", add(lhs.cacheWriteInputTokens, rhs.cacheWriteInputTokens)],
    ["reasoningTokens", add(lhs.reasoningTokens, rhs.reasoningTokens)],
    ["modelCalls", add(lhs.modelCalls, rhs.modelCalls)],
  ];
  return Object.fromEntries(values.filter(([, value]) => value !== undefined)) as Usage;
}
