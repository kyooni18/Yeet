import path from "node:path";
import { lstat, realpath } from "node:fs/promises";

function isInside(root: string, candidate: string): boolean {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

async function existingAncestor(candidate: string): Promise<string> {
  let cursor = candidate;
  while (true) {
    try {
      await lstat(cursor);
      return cursor;
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code !== "ENOENT") throw error;
      const parent = path.dirname(cursor);
      if (parent === cursor) throw error;
      cursor = parent;
    }
  }
}

export class PathGuard {
  readonly root: string;
  readonly realRoot: string;

  private constructor(root: string, realRoot: string) {
    this.root = root;
    this.realRoot = realRoot;
  }

  static async create(root: string): Promise<PathGuard> {
    const absolute = path.resolve(root);
    const resolved = await realpath(absolute);
    return new PathGuard(absolute, resolved);
  }

  async resolve(input: string, options: { allowMissing?: boolean } = {}): Promise<string> {
    const lexical = path.resolve(this.root, input);
    if (!isInside(this.root, lexical)) throw new Error(`Path escapes workspace root: ${input}`);

    try {
      const stat = await lstat(lexical);
      if (stat.isSymbolicLink()) throw new Error(`Refusing to edit symbolic link: ${input}`);
      const canonical = await realpath(lexical);
      if (!isInside(this.realRoot, canonical)) throw new Error(`Resolved path escapes workspace root: ${input}`);
      return canonical;
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code !== "ENOENT" || !options.allowMissing) throw error;
      const ancestor = await existingAncestor(path.dirname(lexical));
      const realAncestor = await realpath(ancestor);
      if (!isInside(this.realRoot, realAncestor)) throw new Error(`Parent path escapes workspace root: ${input}`);
      return lexical;
    }
  }

  display(canonical: string): string {
    const relative = path.relative(this.realRoot, canonical);
    return relative === "" ? "." : relative.split(path.sep).join("/");
  }
}


