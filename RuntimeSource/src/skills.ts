import { execFile } from "node:child_process";
import { cp, mkdir, mkdtemp, readdir, readFile, rm, stat } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { basename, extname, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";
import { defaultConfigDirectory } from "./platform.js";

const execFileAsync = promisify(execFile);

export interface SkillSummary {
  name: string;
  description: string;
  root: string;
  entrypoint: string;
  shortDescription?: string;
  allowImplicitInvocation?: boolean;
  source: "project" | "user" | "codex" | "custom";
}

export interface Skill extends SkillSummary {
  instructions: string;
  files: string[];
}

export interface SkillRegistryOptions {
  configDir?: string;
  roots?: string[];
  projectRoot?: string;
  includeCodexSkills?: boolean;
}

export interface SkillInstallResult {
  installed: string[];
  destinationRoot: string;
}

interface Frontmatter {
  name: string;
  description: string;
  shortDescription?: string;
  allowImplicitInvocation?: boolean;
}

interface ParsedSkillMarkdown {
  frontmatter: Frontmatter;
  instructions: string;
  legacy: boolean;
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

function sanitizeName(value: string): string {
  return value.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

function inferLegacyDescription(source: string, name: string): string {
  for (const line of source.replace(/\r\n/g, "\n").split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const heading = /^#{1,6}\s+(.+)$/.exec(trimmed);
    if (heading?.[1]) return heading[1].trim();
    if (!trimmed.startsWith("<!--")) return trimmed.slice(0, 240);
  }
  return `Instructions for ${name}`;
}

function parseFrontmatterValues(lines: string[]): Record<string, string> {
  const values: Record<string, string> = {};
  const stack: Array<{ indent: number; key: string }> = [];

  for (let index = 0; index < lines.length; index += 1) {
    const raw = lines[index] ?? "";
    if (!raw.trim() || raw.trimStart().startsWith("#")) continue;
    const indent = raw.length - raw.trimStart().length;
    while (stack.length && indent <= stack[stack.length - 1]!.indent) stack.pop();
    const match = /^\s*([A-Za-z0-9_-]+):(?:\s*(.*))?$/.exec(raw);
    if (!match) continue;
    const key = match[1] ?? "";
    let value = match[2] ?? "";
    const path = [...stack.map((item) => item.key), key].join(".");

    if (!value.trim()) {
      stack.push({ indent, key });
      continue;
    }
    if (value === "|" || value === ">") {
      const folded = value === ">";
      const block: string[] = [];
      while (index + 1 < lines.length) {
        const next = lines[index + 1] ?? "";
        const nextIndent = next.length - next.trimStart().length;
        if (next.trim() && nextIndent <= indent) break;
        index += 1;
        block.push(next.slice(Math.min(next.length, indent + 2)));
      }
      value = folded ? block.join(" ").trim() : block.join("\n").trim();
    }
    values[path] = unquote(value.replace(/\s+#.*$/, ""));
  }
  return values;
}

function parseSkillMarkdown(source: string, fallbackName?: string): ParsedSkillMarkdown {
  const normalized = source.replace(/\r\n/g, "\n");
  if (!normalized.startsWith("---\n")) {
    const name = sanitizeName(fallbackName ?? "");
    if (!name) throw new Error("SKILL.md must begin with YAML frontmatter");
    return {
      frontmatter: { name, description: inferLegacyDescription(normalized, name) },
      instructions: normalized,
      legacy: true,
    };
  }

  const end = normalized.indexOf("\n---\n", 4);
  if (end < 0) throw new Error("SKILL.md frontmatter is not terminated");
  const values = parseFrontmatterValues(normalized.slice(4, end).split("\n"));
  const name = sanitizeName(values.name || fallbackName || "");
  const description = values.description?.trim();
  if (!name) throw new Error("SKILL.md frontmatter requires name");
  if (!description) throw new Error(`Skill ${name} frontmatter requires description`);
  if (name.length > 64) throw new Error(`Skill name exceeds 64 characters: ${name}`);

  const implicit = values["policy.allow_implicit_invocation"];
  return {
    frontmatter: {
      name,
      description,
      ...(values["metadata.short-description"] ? { shortDescription: values["metadata.short-description"] } : {}),
      ...(implicit !== undefined ? { allowImplicitInvocation: !/^(false|no|0)$/i.test(implicit) } : {}),
    },
    instructions: normalized.slice(end + 5).replace(/^\n+/, ""),
    legacy: false,
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

async function entrypointFor(skillRoot: string): Promise<string | undefined> {
  for (const name of ["SKILL.md", "skill.md"]) {
    const candidate = join(skillRoot, name);
    try {
      if ((await stat(candidate)).isFile()) return candidate;
    } catch (error) {
      if ((error as { code?: string }).code !== "ENOENT") throw error;
    }
  }
  return undefined;
}

async function openAiMetadata(skillRoot: string): Promise<{ shortDescription?: string; allowImplicitInvocation?: boolean }> {
  const metadataPath = join(skillRoot, "agents", "openai.yaml");
  try {
    const values = parseFrontmatterValues(
      (await readFile(metadataPath, "utf8")).replace(/\r\n/g, "\n").split("\n"),
    );
    const implicit = values["policy.allow_implicit_invocation"];
    return {
      ...(values["interface.short_description"] ? { shortDescription: values["interface.short_description"] } : {}),
      ...(implicit !== undefined
        ? { allowImplicitInvocation: !/^(false|no|0)$/i.test(implicit) }
        : {}),
    };
  } catch (error) {
    if ((error as { code?: string }).code === "ENOENT") return {};
    throw error;
  }
}

async function discoverSkillRoots(root: string, depth = 0): Promise<string[]> {
  if (await entrypointFor(root)) return [root];
  if (depth >= 2) return [];
  let entries;
  try { entries = await readdir(root, { withFileTypes: true }); }
  catch (error) {
    if ((error as { code?: string }).code === "ENOENT") return [];
    throw error;
  }
  const roots: string[] = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.isSymbolicLink() || entry.name.startsWith(".")) continue;
    roots.push(...await discoverSkillRoots(join(root, entry.name), depth + 1));
  }
  return roots;
}

export class SkillRegistry {
  readonly configDir: string;
  readonly roots: string[];
  readonly userRoot: string;

  constructor(options: SkillRegistryOptions = {}) {
    this.configDir = options.configDir ?? defaultConfigDirectory();
    this.userRoot = join(this.configDir, "skills");
    if (options.roots?.length) {
      this.roots = options.roots.map((root) => resolve(root));
    } else {
      const projectRoot = resolve(options.projectRoot ?? process.cwd());
      this.roots = [join(projectRoot, ".yeet", "skills"), this.userRoot];
      const includeCodexSkills = options.includeCodexSkills
        ?? /^(1|true|yes)$/i.test(process.env.YEET_CODEX_SKILLS ?? "");
      if (includeCodexSkills) this.roots.push(join(homedir(), ".codex", "skills"));
    }
  }

  async ensure(): Promise<void> {
    await mkdir(this.userRoot, { recursive: true, mode: 0o700 });
  }

  async list(): Promise<SkillSummary[]> {
    await this.ensure();
    const byName = new Map<string, SkillSummary>();
    for (const [rootIndex, root] of this.roots.entries()) {
      let entries;
      try { entries = await readdir(root, { withFileTypes: true }); }
      catch (error) {
        if ((error as { code?: string }).code === "ENOENT") continue;
        throw error;
      }
      for (const entry of entries) {
        if (!entry.isDirectory() || entry.isSymbolicLink()) continue;
        const skillRoot = join(root, entry.name);
        const entrypoint = await entrypointFor(skillRoot);
        if (!entrypoint) continue;
        try {
          const parsed = parseSkillMarkdown(await readFile(entrypoint, "utf8"), entry.name);
          const agentMetadata = await openAiMetadata(skillRoot);
          if (!byName.has(parsed.frontmatter.name)) {
            byName.set(parsed.frontmatter.name, {
              ...parsed.frontmatter,
              ...agentMetadata,
              root: skillRoot,
              entrypoint,
              source: rootIndex === 0 ? "project" : root === this.userRoot ? "user" : root === join(homedir(), ".codex", "skills") ? "codex" : "custom",
            });
          }
        } catch (error) {
          throw new Error(`Failed to load skill at ${skillRoot}: ${(error as Error).message}`);
        }
      }
    }
    return [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
  }

  async load(name: string): Promise<Skill> {
    const summary = (await this.list()).find((skill) => skill.name === name);
    if (!summary) throw new Error(`Unknown skill: ${name}`);
    const parsed = parseSkillMarkdown(await readFile(summary.entrypoint, "utf8"), basename(summary.root));
    return { ...summary, instructions: parsed.instructions, files: await walkFiles(summary.root) };
  }

  async readSkillFile(name: string, path: string): Promise<string> {
    const skill = await this.load(name);
    return readFile(containedPath(skill.root, path), "utf8");
  }

  async validate(source: string): Promise<SkillSummary[]> {
    const root = resolve(source);
    const candidates = await discoverSkillRoots(root);
    if (!candidates.length) throw new Error(`No SKILL.md or skill.md found under ${source}`);
    const results: SkillSummary[] = [];
    for (const candidate of candidates) {
      const entrypoint = await entrypointFor(candidate);
      if (!entrypoint) continue;
      const parsed = parseSkillMarkdown(await readFile(entrypoint, "utf8"), basename(candidate));
      results.push({
        ...parsed.frontmatter,
        ...await openAiMetadata(candidate),
        root: candidate,
        entrypoint,
        source: "custom",
      });
    }
    return results;
  }

  async install(source: string): Promise<SkillInstallResult> {
    await this.ensure();
    const temp = await mkdtemp(join(tmpdir(), "yeet-skill-install-"));
    let prepared = source;
    try {
      if (/^https?:\/\//i.test(source) || source.startsWith("git@")) {
        prepared = join(temp, "repo");
        await execFileAsync("git", ["clone", "--depth", "1", source, prepared]);
      } else if (extname(source).toLowerCase() === ".zip") {
        prepared = join(temp, "archive");
        await mkdir(prepared, { recursive: true });
        await execFileAsync("unzip", ["-q", resolve(source), "-d", prepared]);
      } else {
        prepared = resolve(source);
      }

      const candidates = await discoverSkillRoots(prepared);
      if (!candidates.length) throw new Error(`No skills found in ${source}`);
      const installed: string[] = [];
      for (const candidate of candidates) {
        const entrypoint = await entrypointFor(candidate);
        if (!entrypoint) continue;
        const parsed = parseSkillMarkdown(await readFile(entrypoint, "utf8"), basename(candidate));
        const destination = join(this.userRoot, parsed.frontmatter.name);
        try {
          await stat(destination);
          throw new Error(`Skill already installed: ${parsed.frontmatter.name}`);
        } catch (error) {
          if ((error as { code?: string }).code !== "ENOENT") throw error;
        }
        await cp(candidate, destination, { recursive: true, errorOnExist: true, force: false });
        installed.push(parsed.frontmatter.name);
      }
      return { installed, destinationRoot: this.userRoot };
    } finally {
      await rm(temp, { recursive: true, force: true });
    }
  }

  async remove(name: string): Promise<boolean> {
    const normalized = sanitizeName(name);
    if (!normalized) throw new Error("Skill name is required");
    const target = containedPath(this.userRoot, normalized);
    try {
      if (!(await stat(target)).isDirectory()) return false;
    } catch (error) {
      if ((error as { code?: string }).code === "ENOENT") return false;
      throw error;
    }
    await rm(target, { recursive: true, force: false });
    return true;
  }
}
