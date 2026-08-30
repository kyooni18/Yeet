import { mkdir, readdir, readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join, relative, resolve, sep } from "node:path";

export interface SkillSummary {
  name: string;
  description: string;
  root: string;
  entrypoint: string;
}

export interface Skill extends SkillSummary {
  instructions: string;
  files: string[];
}

export interface SkillRegistryOptions {
  configDir?: string;
  roots?: string[];
}

interface Frontmatter {
  name: string;
  description: string;
}

function unquote(value: string): string {
  const trimmed = value.trim();
  if (trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"')) {
    try { return JSON.parse(trimmed) as string; } catch { return trimmed.slice(1, -1); }
  }
  if (trimmed.length >= 2 && trimmed.startsWith("'") && trimmed.endsWith("'")) {
    return trimmed.slice(1, -1).replace(/''/g, "'");
  }
  return trimmed;
}

function parseSkillMarkdown(source: string): { frontmatter: Frontmatter; instructions: string } {
  const normalized = source.replace(/\r\n/g, "\n");
  if (!normalized.startsWith("---\n")) throw new Error("SKILL.md must begin with YAML frontmatter");
  const end = normalized.indexOf("\n---\n", 4);
  if (end < 0) throw new Error("SKILL.md frontmatter is not terminated");

  const lines = normalized.slice(4, end).split("\n");
  const values: Record<string, string> = {};
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i] ?? "";
    const match = /^([A-Za-z0-9_-]+):(?:\s*(.*))?$/.exec(line);
    if (!match) continue;
    const key = match[1] ?? "";
    let value = match[2] ?? "";
    if (value === "|" || value === ">") {
      const folded = value === ">";
      const block: string[] = [];
      while (i + 1 < lines.length) {
        const next = lines[i + 1] ?? "";
        if (next && !/^\s+/.test(next)) break;
        i += 1;
        block.push(next.replace(/^\s{1,4}/, ""));
      }
      value = folded ? block.join(" ").trim() : block.join("\n").trim();
    }
    values[key] = unquote(value);
  }

  const name = values.name?.trim();
  const description = values.description?.trim();
  if (!name) throw new Error("SKILL.md frontmatter requires name");
  if (!description) throw new Error(`Skill ${name} frontmatter requires description`);
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)) {
    throw new Error(`Skill name must be lowercase kebab-case: ${name}`);
  }

  return {
    frontmatter: { name, description },
    instructions: normalized.slice(end + 5).replace(/^\n+/, ""),
  };
}

async function walkFiles(root: string, current = root): Promise<string[]> {
  const entries = await readdir(current, { withFileTypes: true });
  const files: string[] = [];
  for (const entry of entries) {
    if (entry.isSymbolicLink()) continue;
    const absolute = join(current, entry.name);
    if (entry.isDirectory()) files.push(...await walkFiles(root, absolute));
    else if (entry.isFile()) files.push(relative(root, absolute));
  }
  return files.sort();
}

function containedPath(root: string, requested: string): string {
  if (!requested || requested.includes("\0")) throw new Error("Skill file path is required");
  const base = resolve(root);
  const target = resolve(base, requested);
  if (target !== base && !target.startsWith(`${base}${sep}`)) {
    throw new Error(`Skill path escapes its root: ${requested}`);
  }
  return target;
}

export class SkillRegistry {
  readonly configDir: string;
  readonly roots: string[];

  constructor(options: SkillRegistryOptions = {}) {
    this.configDir = options.configDir ?? process.env.YEET_CONFIG_DIR ?? join(homedir(), ".yeet");
    this.roots = options.roots?.length ? options.roots.map((root) => resolve(root)) : [join(this.configDir, "skills")];
  }

  async ensure(): Promise<void> {
    await mkdir(this.roots[0]!, { recursive: true, mode: 0o700 });
  }

  async list(): Promise<SkillSummary[]> {
    await this.ensure();
    const byName = new Map<string, SkillSummary>();
    for (const root of this.roots) {
      let entries;
      try {
        entries = await readdir(root, { withFileTypes: true });
      } catch (error) {
        if ((error as { code?: string }).code === "ENOENT") continue;
        throw error;
      }
      for (const entry of entries) {
        if (!entry.isDirectory() || entry.isSymbolicLink()) continue;
        const skillRoot = join(root, entry.name);
        const entrypoint = join(skillRoot, "SKILL.md");
        try {
          const parsed = parseSkillMarkdown(await readFile(entrypoint, "utf8"));
          if (!byName.has(parsed.frontmatter.name)) {
            byName.set(parsed.frontmatter.name, {
              ...parsed.frontmatter,
              root: skillRoot,
              entrypoint,
            });
          }
        } catch (error) {
          if ((error as { code?: string }).code === "ENOENT") continue;
          throw new Error(`Failed to load skill at ${skillRoot}: ${(error as Error).message}`);
        }
      }
    }
    return [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
  }

  async load(name: string): Promise<Skill> {
    const summary = (await this.list()).find((skill) => skill.name === name);
    if (!summary) throw new Error(`Unknown skill: ${name}`);
    const parsed = parseSkillMarkdown(await readFile(summary.entrypoint, "utf8"));
    return {
      ...summary,
      instructions: parsed.instructions,
      files: await walkFiles(summary.root),
    };
  }

  async readSkillFile(name: string, path: string): Promise<string> {
    const skill = await this.load(name);
    const target = containedPath(skill.root, path);
    return readFile(target, "utf8");
  }
}
