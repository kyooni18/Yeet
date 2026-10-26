import { access, readFile } from "node:fs/promises";
import { join } from "node:path";
import { defaultConfigDirectory } from "./platform.js";

import type { McpCallToolResult, McpServerConfiguration, McpTool } from "./mcp.js";
import { McpManager } from "./mcp.js";

export const OPEN_COMPUTER_USE_RUNTIME = "open-computer-use";

const DEFAULT_COMPUTER_USE_TIMEOUT_MS = 30_000;
const MAX_SAFE_TIMER_TIMEOUT_MS = 2_147_483_647;

function computerUseTimeoutMs(args: Record<string, unknown>): number {
  const requested = args.timeout_ms;
  if (typeof requested !== "number" || !Number.isSafeInteger(requested) || requested < 1) {
    return DEFAULT_COMPUTER_USE_TIMEOUT_MS;
  }
  return Math.min(requested, MAX_SAFE_TIMER_TIMEOUT_MS);
}

function signalReason(signal: AbortSignal): unknown {
  return signal.reason ?? new Error("Open Computer Use cancelled");
}

async function withComputerUseTimeout<T>(
  timeoutMs: number,
  parent: AbortSignal | undefined,
  operation: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  if (parent?.aborted) throw signalReason(parent);
  const controller = new AbortController();
  const onParentAbort = () => controller.abort(signalReason(parent!));
  parent?.addEventListener("abort", onParentAbort, { once: true });
  const timer = setTimeout(
    () => controller.abort(new Error(`Open Computer Use timed out after ${timeoutMs}ms`)),
    timeoutMs,
  );
  let onAbort: (() => void) | undefined;
  const aborted = new Promise<never>((_, reject) => {
    onAbort = () => reject(signalReason(controller.signal));
    controller.signal.addEventListener("abort", onAbort, { once: true });
  });
  try {
    return await Promise.race([operation(controller.signal), aborted]);
  } finally {
    clearTimeout(timer);
    parent?.removeEventListener("abort", onParentAbort);
    if (onAbort) controller.signal.removeEventListener("abort", onAbort);
  }
}

export interface OpenComputerUseDiscoveryOptions {
  configDir?: string;
}

export interface OpenComputerUseInstallation {
  version: string;
  packageRoot: string;
  configuration: McpServerConfiguration;
}

/** Use only the Yeet-managed npm package, never Codex's private plugin cache. */
export async function discoverOpenComputerUse(
  options: OpenComputerUseDiscoveryOptions = {},
): Promise<OpenComputerUseInstallation> {
  const packageRoot = join(options.configDir ?? defaultConfigDirectory(),
    "computer-use", "node_modules", "open-computer-use");
  const adapter = join(packageRoot, "scripts", "node-repl", "open-computer-use-repl.mjs");
  const launcher = join(packageRoot, "bin", "open-computer-use");
  try {
    const manifest = JSON.parse(await readFile(join(packageRoot, "package.json"), "utf8"));
    if (manifest.name !== "open-computer-use" || typeof manifest.version !== "string") {
      throw new Error("invalid package manifest");
    }
    await Promise.all([access(adapter), access(launcher),
      access(join(packageRoot, "scripts", "node-repl", "open-computer-use-kernel.mjs"))]);
    return {
      version: manifest.version,
      packageRoot,
      configuration: {
        name: OPEN_COMPUTER_USE_RUNTIME,
        transport: "stdio",
        command: process.execPath,
        // The upstream JS adapter speaks MCP. Its native child is launched via
        // the npm CLI so platform selection works without shell/PATH shims.
        args: [adapter, "--", process.execPath, launcher, "mcp"],
      },
    };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`Open Computer Use is unavailable at ${packageRoot}. Run \`yeet install computer-use\`. ${message}`);
  }
}

export class OpenComputerUse {
  readonly #mcp: McpManager;
  readonly #options: OpenComputerUseDiscoveryOptions;
  #fingerprint: string | undefined;

  constructor(mcp: McpManager, options: OpenComputerUseDiscoveryOptions = {}) {
    this.#mcp = mcp;
    this.#options = options;
  }

  async listTools(signal?: AbortSignal): Promise<McpTool[]> {
    await this.#ensureRuntime();
    return (await this.#mcp.listTools(OPEN_COMPUTER_USE_RUNTIME, signal))
      .filter((tool) => tool.name === "js" || tool.name === "js_reset");
  }

  async call(
    tool: "js" | "js_reset",
    args: Record<string, unknown> = {},
    signal?: AbortSignal,
  ): Promise<McpCallToolResult> {
    if (tool !== "js" && tool !== "js_reset") throw new Error(`Unsupported Open Computer Use tool: ${tool}`);
    let runtimeReady = false;
    try {
      await this.#ensureRuntime();
      runtimeReady = true;
      const timeoutMs = computerUseTimeoutMs(args);
      return await withComputerUseTimeout(
        timeoutMs,
        signal,
        (requestSignal) => this.#mcp.callTool(OPEN_COMPUTER_USE_RUNTIME, tool, args, requestSignal),
      );
    } catch (error) {
      if (runtimeReady) {
        try {
          await this.#mcp.disconnect(OPEN_COMPUTER_USE_RUNTIME);
        } catch {
          // Best effort: the original call failure is the useful error to preserve.
        }
      }
      // Keep the MCP transport detail out of the built-in capability error.
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(`Open Computer Use failed: ${message}`);
    }
  }

  async #ensureRuntime(): Promise<void> {
    const installation = await discoverOpenComputerUse(this.#options);
    const fingerprint = JSON.stringify([installation.version, installation.configuration]);
    if (this.#fingerprint === fingerprint) return;
    await this.#mcp.setRuntimeServer(installation.configuration);
    this.#fingerprint = fingerprint;
  }
}
