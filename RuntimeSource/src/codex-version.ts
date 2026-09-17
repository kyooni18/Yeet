import { execFileSync } from "node:child_process";
import process from "node:process";

const VERSION_CACHE_MS = 60_000;
let cachedVersion: string | undefined;
let cacheExpiresAt = 0;

export function parseCodexCliVersion(output: string): string | undefined {
  const match = output.match(/\b(?:codex-cli\s+)?v?(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)\b/i);
  return match?.[1];
}

export function codexCliVersion(): string | undefined {
  const override = parseCodexCliVersion(process.env.YEET_CODEX_CLIENT_VERSION?.trim() ?? "");
  if (override) return override;

  const now = Date.now();
  if (now < cacheExpiresAt) return cachedVersion;

  const configured = process.env.YEET_CODEX_BIN?.trim();
  const home = process.env.HOME?.trim();
  const candidates = [
    configured,
    "codex",
    "/opt/homebrew/bin/codex",
    "/usr/local/bin/codex",
    home ? `${home}/.local/bin/codex` : undefined,
  ].filter((value, index, values): value is string => Boolean(value) && values.indexOf(value) === index);

  cachedVersion = undefined;
  for (const executable of candidates) {
    try {
      const output = execFileSync(executable, ["--version"], {
        encoding: "utf8",
        timeout: 2_000,
        stdio: ["ignore", "pipe", "ignore"],
      });
      const version = parseCodexCliVersion(output);
      if (version) {
        cachedVersion = version;
        break;
      }
    } catch {
      // Keep trying common Codex installation locations.
    }
  }
  cacheExpiresAt = now + VERSION_CACHE_MS;
  return cachedVersion;
}
