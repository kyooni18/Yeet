const SECTION = /^\[([^\]@#]+)(?:[@#]([^\]]+))?\]$/;
const PUT_RANGE = /^PUT\s+(\d+)\.=\s*(\d+)\s*:\s*$/i;
const PUT_BLOCK = /^PUT\s+(\d+)\*\s*:\s*$/i;
const PUT_BEFORE = /^PUT\s+<(\d+)\s*:\s*$/i;
const PUT_AFTER = /^PUT\s+>(\d+)\s*:\s*$/i;
const PUT_AFTER_BLOCK = /^PUT\s+>(\d+)\*\s*:\s*$/i;
const PUT_HEAD = /^PUT\s+<1\s*:\s*$/i;
const PUT_TAIL = /^PUT\s+>\$\s*:\s*$/i;
const CUT_RANGE = /^CUT\s+(\d+)\.=\s*(\d+)\s*$/i;
const CUT_BLOCK = /^CUT\s+(\d+)\*\s*$/i;
const MOVE = /^MV\s+(.+)$/i;
function unquote(value) {
    const trimmed = value.trim();
    if ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'"))) {
        return trimmed.slice(1, -1);
    }
    return trimmed;
}
function consumeBody(lines, start) {
    const body = [];
    let index = start;
    while (index < lines.length) {
        const line = lines[index];
        if (line.trim() === "" || SECTION.test(line) || /^(?:PUT|CUT|REM|MV)\b/i.test(line))
            break;
        if (!line.startsWith("+"))
            throw new Error(`Hashline body row must start with '+': ${line}`);
        body.push(line.slice(1));
        index++;
    }
    if (body.length === 0)
        throw new Error("PUT requires at least one '+' body row.");
    return { text: body.join("\n"), next: index };
}
export class HashlineDialect {
    id = "hashline";
    parse(input) {
        const lines = input.replace(/\r\n/g, "\n").split("\n");
        const changes = [];
        let current;
        let index = 0;
        const pushCurrent = () => {
            if (current)
                changes.push(current);
            current = undefined;
        };
        while (index < lines.length) {
            const raw = lines[index];
            const line = raw.trimEnd();
            if (line.trim() === "") {
                index++;
                continue;
            }
            const section = SECTION.exec(line);
            if (section) {
                pushCurrent();
                current = {
                    path: section[1].trim(),
                    ...(section[2]?.trim() ? { snapshot: section[2].trim() } : {}),
                    edits: [],
                };
                index++;
                continue;
            }
            if (!current)
                throw new Error("Hashline payload must begin with [path@snapshot].");
            let match;
            if ((match = PUT_RANGE.exec(line))) {
                const body = consumeBody(lines, index + 1);
                current.edits.push({ kind: "replace", range: { start: Number(match[1]), end: Number(match[2]) }, text: body.text });
                index = body.next;
                continue;
            }
            if ((match = PUT_AFTER_BLOCK.exec(line))) {
                const body = consumeBody(lines, index + 1);
                current.edits.push({ kind: "insertAfterBlock", line: Number(match[1]), text: body.text });
                index = body.next;
                continue;
            }
            if ((match = PUT_BLOCK.exec(line))) {
                const body = consumeBody(lines, index + 1);
                current.edits.push({ kind: "replaceBlock", line: Number(match[1]), text: body.text });
                index = body.next;
                continue;
            }
            if (PUT_TAIL.test(line)) {
                const body = consumeBody(lines, index + 1);
                current.edits.push({ kind: "insert", at: { kind: "end" }, text: body.text });
                index = body.next;
                continue;
            }
            if (PUT_HEAD.test(line)) {
                const body = consumeBody(lines, index + 1);
                current.edits.push({ kind: "insert", at: { kind: "start" }, text: body.text });
                index = body.next;
                continue;
            }
            if ((match = PUT_BEFORE.exec(line))) {
                const body = consumeBody(lines, index + 1);
                current.edits.push({ kind: "insert", at: { kind: "before", line: Number(match[1]) }, text: body.text });
                index = body.next;
                continue;
            }
            if ((match = PUT_AFTER.exec(line))) {
                const body = consumeBody(lines, index + 1);
                current.edits.push({ kind: "insert", at: { kind: "after", line: Number(match[1]) }, text: body.text });
                index = body.next;
                continue;
            }
            if ((match = CUT_RANGE.exec(line))) {
                current.edits.push({ kind: "delete", range: { start: Number(match[1]), end: Number(match[2]) } });
                index++;
                continue;
            }
            if ((match = CUT_BLOCK.exec(line))) {
                current.edits.push({ kind: "deleteBlock", line: Number(match[1]) });
                index++;
                continue;
            }
            if (/^REM\s*$/i.test(line)) {
                current.fileOp = { kind: "delete" };
                index++;
                continue;
            }
            if ((match = MOVE.exec(line))) {
                current.fileOp = { kind: "move", destination: unquote(match[1]) };
                index++;
                continue;
            }
            throw new Error(`Unknown hashline operation: ${line}`);
        }
        pushCurrent();
        if (changes.length === 0)
            throw new Error("Hashline payload contained no file sections.");
        return { changes };
    }
}
//# sourceMappingURL=hashline.js.map