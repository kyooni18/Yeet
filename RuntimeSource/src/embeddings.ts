import { createHash } from "node:crypto";
import { providerFetch, readJson } from "./http.js";
import type { ProviderFetchLogger } from "./http.js";
import type { EmbeddingRequest, EmbeddingResult, FetchLike } from "./types.js";

/** Shared transport uses the same provider credentials, endpoint and logging as generation. */
export async function fetchEmbeddings(options: {
  provider: string; baseUrl: string; headers: Record<string, string>;
  request: EmbeddingRequest; format: "openai" | "gemini";
  fetch?: FetchLike | undefined; apiCallLogger?: ProviderFetchLogger | undefined;
}): Promise<EmbeddingResult> {
  const { request, format } = options;
  if (!Array.isArray(request.input) || request.input.length < 1 || request.input.length > 64
      || request.input.some(text => typeof text !== "string" || !text.trim() || text.length > 32000)) {
    throw new Error("Embedding input must contain 1..64 nonempty strings of at most 32000 characters");
  }
  const model = request.model.replace(/^models\//, "");
  const url = format === "gemini"
    ? `${options.baseUrl}/models/${encodeURIComponent(model)}:batchEmbedContents`
    : `${options.baseUrl}/embeddings`;
  const body = format === "gemini"
    ? { requests: request.input.map(text => ({ model: `models/${model}`, content: { parts: [{ text }] } })) }
    : { model: request.model, input: request.input, encoding_format: "float" };
  const response = await providerFetch(url, {
    method: "POST", headers: options.headers, body: JSON.stringify(body),
  }, {
    provider: options.provider, timeoutMs: 60000,
    ...(options.fetch ? { fetch: options.fetch } : {}),
    ...(options.apiCallLogger ? { apiCallLogger: options.apiCallLogger } : {}),
    ...(request.signal ? { signal: request.signal } : {}),
  });
  const raw = await readJson<any>(response);
  let vectors: unknown[];
  if (format === "gemini") vectors = raw.embeddings?.map((item: any) => item.values) ?? [];
  else {
    if (!Array.isArray(raw.data)) throw new Error("Embedding response has no data");
    const rows = [...raw.data].sort((a, b) => a.index - b.index);
    if (rows.some((row, index) => row.index !== index)) throw new Error("Embedding indices are missing or duplicated");
    vectors = rows.map(row => row.embedding);
  }
  const dimensions = Array.isArray(vectors[0]) ? vectors[0].length : 0;
  if (vectors.length !== request.input.length || dimensions < 1 || dimensions > 65536
      || vectors.some(vector => !Array.isArray(vector) || vector.length !== dimensions
        || vector.some(n => typeof n !== "number" || !Number.isFinite(n))
        || !vector.some(n => n !== 0))) throw new Error("Invalid embedding vectors or dimensions");
  return {
    model: request.model,
    resolvedModel: typeof raw.model === "string" ? raw.model : request.model,
    source: createHash("sha256").update(`${format}\0${options.baseUrl}`).digest("hex"),
    vectors: vectors as number[][],
  };
}
