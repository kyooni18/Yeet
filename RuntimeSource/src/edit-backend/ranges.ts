import type { LineRange } from "./types.js";

export class IntervalSet {
  #ranges: LineRange[] = [];

  constructor(ranges: readonly LineRange[] = []) {
    for (const range of ranges) this.add(range.start, range.end);
  }

  add(start: number, end = start): void {
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 1 || end < start) {
      throw new Error(`Invalid line range ${start}..${end}`);
    }
    const next: LineRange[] = [];
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
    if (!inserted) next.push(pending);
    this.#ranges = next;
  }

  covers(start: number, end = start): boolean {
    return this.#ranges.some(range => range.start <= start && range.end >= end);
  }

  has(line: number): boolean {
    return this.covers(line, line);
  }

  clone(): IntervalSet {
    return new IntervalSet(this.#ranges);
  }

  toJSON(): LineRange[] {
    return this.#ranges.map(range => ({ ...range }));
  }

  *values(): Generator<number> {
    for (const range of this.#ranges) {
      for (let line = range.start; line <= range.end; line++) yield line;
    }
  }
}


