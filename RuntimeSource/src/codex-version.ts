import { execFile } from "node:child_process";
import path from "node:path";
import process from "node:process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const VERSION_CACHE_MS = 60_000;
let cachedVersion: string | undefined;
let cacheExpiresAt = 0;
let pendingVersion: Promise<string | undefined> | undefined;

export function parseCodexCliVersion(output: string): string | undefined {
  const match = output.match(/\b(?:codex-cli\s+)?v?(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)\b/i);
  return match?.[1];
}

export async function codexCliVersion(): Promise<string | undefined> {
  const override = parseCodexCliVersion(process.env.YEET_CODEX_CLIENT_VERSION?.trim() ?? "");
  if (override) return override;

  const now = Date.now();
  if (now < cacheExpiresAt) return cachedVersion;
  // Discovery runs during model refresh and calls. Never block the bridge's
  // event loop (including cancellation and other providers) on a CLI wrapper.
  if (!pendingVersion) {
    pendingVersion = detectCodexCliVersion().then((version) => {
      cachedVersion = version;
      cacheExpiresAt = Date.now() + VERSION_CACHE_MS;
      return version;
    }).finally(() => { pendingVersion = undefined; });
  }
  return pendingVersion;
}

async function detectCodexCliVersion(): Promise<string | undefined> {
  const configured = process.env.YEET_CODEX_BIN?.trim();
  const home = process.env.HOME?.trim();
  const nodeBin = path.dirname(process.execPath);
  const candidates = [
    configured,
    "codex",
    path.join(nodeBin, "codex"),
    process.env.NVM_BIN ? path.join(process.env.NVM_BIN, "codex") : undefined,
    process.env.PNPM_HOME ? path.join(process.env.PNPM_HOME, "codex") : undefined,
    "/opt/homebrew/bin/codex",
    "/usr/local/bin/codex",
    home ? `${home}/.local/bin/codex` : undefined,
    home ? `${home}/.npm-global/bin/codex` : undefined,
    home ? `${home}/.volta/bin/codex` : undefined,
  ].filter((value, index, values): value is string => Boolean(value) && values.indexOf(value) === index);

  for (const executable of candidates) {
    try {
      const { stdout } = await execFileAsync(executable, ["--version"], {
        encoding: "utf8",
        timeout: 2_000,
        // npm's Codex launcher uses /usr/bin/env node. A Linux service may
        // launch Yeet with YEET_NODE but omit that Node installation from PATH.
        env: { ...process.env, PATH: [process.env.PATH, nodeBin].filter(Boolean).join(path.delimiter) },
      });
      const version = parseCodexCliVersion(stdout);
      if (version) return version;
    } catch {
      // Keep trying common Codex installation locations.
    }
  }
  return undefined;
}
