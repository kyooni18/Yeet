function stripPrefix(file) {
    const trimmed = file.trim().split(/\s+/)[0];
    if (trimmed === "/dev/null")
        return trimmed;
    return trimmed.replace(/^[ab]\//, "");
}
export class ApplyPatchDialect {
    id = "apply_patch";
    parse(input, context) {
        const lines = input.replace(/\r\n/g, "\n").split("\n");
        const changes = [];
        let index = 0;
        while (index < lines.length) {
            if (!lines[index].startsWith("--- ")) {
                index++;
                continue;
            }
            const oldPath = stripPrefix(lines[index].slice(4));
            const plus = lines[index + 1];
            if (!plus?.startsWith("+++ "))
                throw new Error("Malformed unified diff: missing +++ header.");
            const newPath = stripPrefix(plus.slice(4));
            index += 2;
            const path = newPath === "/dev/null" ? oldPath : newPath;
            const edits = [];
            while (index < lines.length && !lines[index].startsWith("--- ")) {
                const header = /^@@\s+-([0-9]+)(?:,([0-9]+))?\s+\+([0-9]+)(?:,([0-9]+))?\s+@@/.exec(lines[index]);
                if (!header) {
                    index++;
                    continue;
                }
                const oldStart = Number(header[1]);
                const oldCount = header[2] === undefined ? 1 : Number(header[2]);
                index++;
                const hunk = { oldStart, oldCount, oldLines: [], newLines: [] };
                while (index < lines.length && !lines[index].startsWith("@@ ") && !lines[index].startsWith("--- ")) {
                    const row = lines[index];
                    if (row.startsWith(" ")) {
                        hunk.oldLines.push(row.slice(1));
                        hunk.newLines.push(row.slice(1));
                    }
                    else if (row.startsWith("-"))
                        hunk.oldLines.push(row.slice(1));
                    else if (row.startsWith("+"))
                        hunk.newLines.push(row.slice(1));
                    else if (!row.startsWith("\\ No newline"))
                        break;
                    index++;
                }
                if (oldCount === 0) {
                    edits.push({
                        kind: "insert",
                        at: oldStart === 0 ? { kind: "start" } : { kind: "after", line: oldStart },
                        text: hunk.newLines.join("\n"),
                    });
                }
                else {
                    edits.push({
                        kind: "replace",
                        range: { start: oldStart, end: oldStart + oldCount - 1 },
                        text: hunk.newLines.join("\n"),
                    });
                }
            }
            if (oldPath === "/dev/null") {
                const text = edits.length === 1 && edits[0]?.kind === "insert" ? edits[0].text : "";
                changes.push({ path, fileOp: { kind: "create", text } });
            }
            else if (newPath === "/dev/null") {
                changes.push({ path, ...(context.snapshots[path] ? { snapshot: context.snapshots[path] } : {}), fileOp: { kind: "delete" } });
            }
            else {
                changes.push({ path, ...(context.snapshots[path] ? { snapshot: context.snapshots[path] } : {}), edits });
            }
        }
        if (changes.length === 0)
            throw new Error("No unified diff file headers were found.");
        return { changes };
    }
}
//# sourceMappingURL=apply-patch.js.map