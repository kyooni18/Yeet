import type { ChatMessage, TokenCounter } from "./types.js";

export class ApproximateTokenCounter implements TokenCounter {
  async countText(text: string): Promise<number> {
    return Math.ceil(new TextEncoder().encode(text).byteLength / 3.2);
  }

  async countMessages(messages: readonly ChatMessage[]): Promise<number> {
    let total = 0;
    for (const message of messages) {
      total += 6;
      total += await this.countText(message.content);
      if (message.name) total += await this.countText(message.name);
      if (message.toolCalls) total += await this.countText(JSON.stringify(message.toolCalls));
    }
    return total;
  }
}
