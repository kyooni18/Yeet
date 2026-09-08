import { createHash } from "node:crypto";

export interface TextEnvelope {
  text: string;
  bom: boolean;
  lineEnding: "\n" | "\r\n";
  endsWithNewline: boolean;
}

export function decodeText(raw: string): TextEnvelope {
  const bom = raw.startsWith("\uFEFF");
  const withoutBom = bom ? raw.slice(1) : raw;
  const crlf = (withoutBom.match(/\r\n/g) ?? []).length;
  const lf = (withoutBom.match(/(?<!\r)\n/g) ?? []).length;
  const lineEnding = crlf > lf ? "\r\n" : "\n";
  const text = withoutBom.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  return { text, bom, lineEnding, endsWithNewline: text.endsWith("\n") };
}

export function encodeText(text: string, envelope: Pick<TextEnvelope, "bom" | "lineEnding">): string {
  const body = envelope.lineEnding === "\r\n" ? text.replace(/\n/g, "\r\n") : text;
  return `${envelope.bom ? "\uFEFF" : ""}${body}`;
}

export function addressableLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  return lines;
}

export function payloadLines(text: string): string[] {
  const normalized = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const withoutFinalTerminator = normalized.endsWith("\n") ? normalized.slice(0, -1) : normalized;
  return withoutFinalTerminator.split("\n");
}

export function joinAddressableLines(lines: readonly string[], keepFinalNewline: boolean): string {
  if (lines.length === 0) return "";
  const body = lines.join("\n");
  return keepFinalNewline ? `${body}\n` : body;
}

export function formatNumbered(lines: readonly string[], startLine: number): string {
  return lines.map((line, index) => `${startLine + index}:${line}`).join("\n");
}

export function lineHash(line: string): string {
  return createHash("sha256").update(line, "utf8").digest("hex").slice(0, 4);
}

export function formatAnchored(lines: readonly string[], startLine: number): string {
  return lines
    .map((line, index) => `${startLine + index}:${lineHash(line)}|${line}`)
    .join("\n");
}


