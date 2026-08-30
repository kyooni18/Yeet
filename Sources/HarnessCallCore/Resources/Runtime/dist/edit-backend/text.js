export function decodeText(raw) {
    const bom = raw.startsWith("\uFEFF");
    const withoutBom = bom ? raw.slice(1) : raw;
    const crlf = (withoutBom.match(/\r\n/g) ?? []).length;
    const lf = (withoutBom.match(/(?<!\r)\n/g) ?? []).length;
    const lineEnding = crlf > lf ? "\r\n" : "\n";
    const text = withoutBom.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
    return { text, bom, lineEnding, endsWithNewline: text.endsWith("\n") };
}
export function encodeText(text, envelope) {
    const body = envelope.lineEnding === "\r\n" ? text.replace(/\n/g, "\r\n") : text;
    return `${envelope.bom ? "\uFEFF" : ""}${body}`;
}
export function addressableLines(text) {
    if (text === "")
        return [];
    const lines = text.split("\n");
    if (lines.at(-1) === "")
        lines.pop();
    return lines;
}
export function payloadLines(text) {
    const normalized = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
    const withoutFinalTerminator = normalized.endsWith("\n") ? normalized.slice(0, -1) : normalized;
    return withoutFinalTerminator.split("\n");
}
export function joinAddressableLines(lines, keepFinalNewline) {
    if (lines.length === 0)
        return "";
    const body = lines.join("\n");
    return keepFinalNewline ? `${body}\n` : body;
}
export function formatNumbered(lines, startLine) {
    return lines.map((line, index) => `${startLine + index}:${line}`).join("\n");
}
//# sourceMappingURL=text.js.map