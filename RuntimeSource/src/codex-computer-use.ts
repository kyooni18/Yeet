import { access, readFile, readdir } from "node:fs/promises";
import { homedir } from "node:os";
import { isAbsolute, join } from "node:path";

import type { McpCallToolResult, McpServerConfiguration, McpTool } from "./mcp.js";
import { McpManager } from "./mcp.js";

export const CODEX_COMPUTER_USE_RUNTIME = "codex-computer-use";
const COMPUTER_USE_PLUGIN = "unified-computer-use";

type RawPluginServer = {
  command?: unknown;
  args?: unknown;
  env?: unknown;
  enabled?: unknown;
  enabled_tools?: unknown;
};

type RawPluginConfig = {
  mcpServers?: Record<string, RawPluginServer>;
};

export interface CodexComputerUseDiscoveryOptions {
  codexHome?: string;
}

export interface CodexComputerUseInstallation {
  pluginVersion: string;
  pluginRoot: string;
  configuration: McpServerConfiguration;
}

function stringRecord(value: unknown): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>)
      .filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

function versionSort(a: string, b: string): number {
  return a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });
}

async function executable(path: string): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

export async function discoverCodexComputerUse(
  options: CodexComputerUseDiscoveryOptions = {},
): Promise<CodexComputerUseInstallation> {
  const codexHome = options.codexHome ?? process.env.CODEX_HOME ?? join(homedir(), ".codex");
  const pluginBase = join(codexHome, "plugins", "cache", "openai-bundled", COMPUTER_USE_PLUGIN);
  let versions: string[];
  try {
    versions = (await readdir(pluginBase, { withFileTypes: true }))
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort(versionSort)
      .reverse();
  } catch {
    throw new Error(`Codex ${COMPUTER_USE_PLUGIN} plugin is not installed under ${pluginBase}`);
  }

  for (const pluginVersion of versions) {
    const pluginRoot = join(pluginBase, pluginVersion);
    let parsed: RawPluginConfig;
    try {
      parsed = JSON.parse(await readFile(join(pluginRoot, ".mcp.json"), "utf8")) as RawPluginConfig;
    } catch {
      continue;
    }
    const candidates = Object.values(parsed.mcpServers ?? {});
    const raw = candidates.find((candidate) => {
      if (candidate.enabled === false) return false;
      const tools = stringArray(candidate.enabled_tools);
      return typeof candidate.command === "string" && (tools.length === 0 || tools.includes("js"));
    });
    if (!raw || typeof raw.command !== "string" || !raw.command.trim()) continue;
    const args = stringArray(raw.args);
    if (!(await executable(raw.command))) continue;
    if (args[0] && isAbsolute(args[0]) && !(await executable(args[0]))) {
      // Launcher scripts only need to be readable. Use path.isAbsolute so
      // native Windows drive/UNC paths receive the same stale-path check.
      continue;
    }
    const env = {
      ...stringRecord(raw.env),
      CODEX_HOME: codexHome,
      // Yeet exposes the native desktop surface only. Browser automation is a
      // separate capability and should not silently broaden this permission.
      CUA_REPL_ENABLED_SURFACES: "computer",
    };
    return {
      pluginVersion,
      pluginRoot,
      configuration: {
        name: CODEX_COMPUTER_USE_RUNTIME,
        transport: "stdio",
        command: raw.command,
        ...(args.length ? { args } : {}),
        env,
      },
    };
  }

  throw new Error(`No usable Codex ${COMPUTER_USE_PLUGIN} runtime was found in ${pluginBase}`);
}

export class CodexComputerUse {
  readonly #mcp: McpManager;
  readonly #options: CodexComputerUseDiscoveryOptions;
  #fingerprint: string | undefined;

  constructor(mcp: McpManager, options: CodexComputerUseDiscoveryOptions = {}) {
    this.#mcp = mcp;
    this.#options = options;
  }

  async listTools(signal?: AbortSignal): Promise<McpTool[]> {
    await this.#ensureRuntime();
    return (await this.#mcp.listTools(CODEX_COMPUTER_USE_RUNTIME, signal))
      .filter((tool) => tool.name === "js" || tool.name === "js_reset");
  }

  async call(
    tool: "js" | "js_reset",
    args: Record<string, unknown> = {},
    signal?: AbortSignal,
  ): Promise<McpCallToolResult> {
    if (tool !== "js" && tool !== "js_reset") throw new Error(`Unsupported Codex Computer Use tool: ${tool}`);
    try {
      await this.#ensureRuntime();
      return await this.#mcp.callTool(CODEX_COMPUTER_USE_RUNTIME, tool, args, signal);
    } catch (error) {
      // The bundled transport currently speaks MCP internally, but that is a
      // Codex implementation detail. Do not leak it into Yeet's user-facing
      // error classification for this first-party capability.
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(`Codex Computer Use failed: ${message}`);
    }
  }

  async #ensureRuntime(): Promise<void> {
    const installation = await discoverCodexComputerUse(this.#options);
    const fingerprint = JSON.stringify(installation.configuration);
    if (this.#fingerprint === fingerprint) return;
    await this.#mcp.setRuntimeServer(installation.configuration);
    this.#fingerprint = fingerprint;
  }
}
