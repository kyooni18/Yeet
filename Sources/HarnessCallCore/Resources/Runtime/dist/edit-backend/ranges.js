export class IntervalSet {
    #ranges = [];
    constructor(ranges = []) {
        for (const range of ranges)
            this.add(range.start, range.end);
    }
    add(start, end = start) {
        if (!Number.isInteger(start) || !Number.isInteger(end) || start < 1 || end < start) {
            throw new Error(`Invalid line range ${start}..${end}`);
        }
        const next = [];
        let pending = { start, end };
        let inserted = false;
        for (const current of this.#ranges) {
            if (current.end + 1 < pending.start) {
                next.push(current);
                continue;
            }
            if (pending.end + 1 < current.start) {
                if (!inserted) {
                    next.push(pending);
                    inserted = true;
                }
                next.push(current);
                continue;
            }
            pending = {
                start: Math.min(pending.start, current.start),
                end: Math.max(pending.end, current.end),
            };
        }
        if (!inserted)
            next.push(pending);
        this.#ranges = next;
    }
    covers(start, end = start) {
        return this.#ranges.some(range => range.start <= start && range.end >= end);
    }
    has(line) {
        return this.covers(line, line);
    }
    clone() {
        return new IntervalSet(this.#ranges);
    }
    toJSON() {
        return this.#ranges.map(range => ({ ...range }));
    }
    *values() {
        for (const range of this.#ranges) {
            for (let line = range.start; line <= range.end; line++)
                yield line;
        }
    }
}
//# sourceMappingURL=ranges.js.map