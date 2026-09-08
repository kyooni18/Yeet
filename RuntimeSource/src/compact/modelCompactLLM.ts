import { COMPACT_SYSTEM_PROMPT, parseCheckpoint, serializeCompactInput } from "./prompt.js";
import type { CompactLLM } from "./types.js";
import type { Usage } from "../types.js";

/** A completed, paid generation whose output cannot serve as a checkpoint. */
export class InvalidCheckpointError extends Error {
  constructor(readonly usage: Usage | undefined, cause: unknown) {
    super("Compact model returned an unusable checkpoint", { cause });
  }
}

export interface YeetModelClient {
  complete(input: {
    model: string;
    messages: Array<{ role: "system" | "user"; content: string }>;
    responseFormat?: "json";
  }): Promise<{ text: string; usage?: Usage }>;
}

export class ModelCompactLLM implements CompactLLM {
  constructor(private readonly client: YeetModelClient, private readonly model: string) {}

  async createCheckpoint(input: Parameters<CompactLLM["createCheckpoint"]>[0]) {
    const result = await this.client.complete({
      model: this.model,
      messages: [
        { role: "system", content: COMPACT_SYSTEM_PROMPT },
        { role: "user", content: serializeCompactInput(input) },
      ],
      responseFormat: "json",
    });
    let checkpoint;
    try {
      checkpoint = parseCheckpoint(result.text);
    } catch (error) {
      throw new InvalidCheckpointError(result.usage, error);
    }
    return {
      checkpoint,
      ...(result.usage ? { usage: result.usage } : {}),
    };
  }
}
