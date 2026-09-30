export interface SSEMessage {
  event?: string;
  data: string;
  id?: string;
}

export async function* parseSSE(response: Response): AsyncGenerator<SSEMessage> {
  if (!response.body) return;

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let event: string | undefined;
  let id: string | undefined;
  let dataLines: string[] = [];

  const flush = (): SSEMessage | undefined => {
    if (dataLines.length === 0) {
      event = undefined;
      id = undefined;
      return undefined;
    }
    const message: SSEMessage = {
      data: dataLines.join("\n"),
      ...(event !== undefined ? { event } : {}),
      ...(id !== undefined ? { id } : {}),
    };
    event = undefined;
    id = undefined;
    dataLines = [];
    return message;
  };

  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });

      while (true) {
        const newline = buffer.search(/[\r\n]/);
        if (newline < 0) break;
        // Keep a trailing CR until the next chunk tells us whether it is CRLF.
        if (buffer[newline] === "\r" && newline + 1 === buffer.length) break;
        const line = buffer.slice(0, newline);
        const separator = buffer[newline] === "\r" && buffer[newline + 1] === "\n" ? 2 : 1;
        buffer = buffer.slice(newline + separator);

        if (line === "") {
          const message = flush();
          if (message) yield message;
          continue;
        }
        if (line.startsWith(":")) continue;

        const colon = line.indexOf(":");
        const field = colon < 0 ? line : line.slice(0, colon);
        let valueText = colon < 0 ? "" : line.slice(colon + 1);
        if (valueText.startsWith(" ")) valueText = valueText.slice(1);

        if (field === "event") event = valueText;
        else if (field === "data") dataLines.push(valueText);
        else if (field === "id") id = valueText;
      }
    }

    buffer += decoder.decode();
    if (buffer.length > 0) {
      const line = buffer.replace(/\r$/, "");
      if (line.startsWith("data:")) dataLines.push(line.slice(5).replace(/^ /, ""));
    }
    const message = flush();
    if (message) yield message;
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
