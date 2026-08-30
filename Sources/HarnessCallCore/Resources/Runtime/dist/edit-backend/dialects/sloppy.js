import { addressableLines } from "../text.js";
const SECTION = /^\[([^\]@#]+)(?:[@#]([^\]]+))?\]$/;
function findUniqueRange(text, search) {
    const lines = addressableLines(text);
    const needle = search.replace(/\r\n/g, "\n").split("\n");
    const hits = [];
    for (let index = 0; index + needle.length <= lines.length; index++) {
        let ok = true;
        for (let j = 0; j < needle.length; j++) {
            if (lines[index + j] !== needle[j]) {
                ok = false;
                break;
            }
        }
        if (ok)
            hits.push(index);
    }
    if (hits.length !== 1)
        throw new Error(`Sloppy SEARCH block matched ${hits.length} locations; expected exactly one.`);
    return { start: hits[0] + 1, end: hits[0] + needle.length };
}
export class SloppyDialect {
    id = "sloppy";
    parse(input, context) {
        const lines = input.replace(/\r\n/g, "\n").split("\n");
        const changes = [];
        let index = 0;
        let current;
        while (index < lines.length) {
            const section = SECTION.exec(lines[index]);
            if (section) {
                if (current)
                    changes.push(current);
                current = {
                    path: section[1].trim(),
                    ...(section[2]?.trim() ? { snapshot: section[2].trim() } : {}),
                    edits: [],
                };
                index++;
                continue;
            }
            if (lines[index] === "<<<<<<< SEARCH") {
                if (!current?.snapshot)
                    throw new Error("Sloppy SEARCH/REPLACE requires [path@snapshot].");
                index++;
                const search = [];
                while (index < lines.length && lines[index] !== "=======")
                    search.push(lines[index++]);
                if (lines[index] !== "=======")
                    throw new Error("Missing ======= in SEARCH/REPLACE block.");
                index++;
                const replacement = [];
                while (index < lines.length && lines[index] !== ">>>>>>> REPLACE")
                    replacement.push(lines[index++]);
                if (lines[index] !== ">>>>>>> REPLACE")
                    throw new Error("Missing >>>>>>> REPLACE marker.");
                index++;
                const range = findUniqueRange(context.getSnapshotText(current.snapshot), search.join("\n"));
                current.edits.push({ kind: "replace", range, text: replacement.join("\n") });
                continue;
            }
            if (lines[index].trim() === "") {
                index++;
                continue;
            }
            throw new Error(`Unexpected sloppy edit line: ${lines[index]}`);
        }
        if (current)
            changes.push(current);
        if (changes.length === 0)
            throw new Error("Sloppy payload contained no file sections.");
        return { changes };
    }
}
//# sourceMappingURL=sloppy.js.map