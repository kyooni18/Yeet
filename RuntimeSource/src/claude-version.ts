import { execFileSync } from "node:child_process";
import process from "node:process";

const VERSION_CACHE_MS = 60_000;
let cachedVersion: string | undefined;
let cacheExpiresAt = 0;

export function claudeCodeExecutable(): string {
  return process.env.CLAUDE_CODE_BIN?.trim() || "claude";
}

export function parseClaudeCodeVersion(output: string): string | undefined {
  return output.match(/\bv?(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)\b/)?.[1];
}

/**
 * Anthropic gates newer models on the Claude Code client version carried in the
 * subscription OAuth user-agent, so report the locally installed Claude Code.
 */
export function claudeCodeVersion(): string | undefined {
  const override = parseClaudeCodeVersion(process.env.YEET_CLAUDE_CODE_CLIENT_VERSION?.trim() ?? "");
  if (override) return override;

  const now = Date.now();
  if (now < cacheExpiresAt) return cachedVersion;

  const home = process.env.HOME?.trim();
  const candidates = [
    claudeCodeExecutable(),
    home ? `${home}/.local/bin/claude` : undefined,
    "/opt/homebrew/bin/claude",
    "/usr/local/bin/claude",
    home ? `${home}/.claude/local/claude` : undefined,
  ].filter((value, index, values): value is string => Boolean(value) && values.indexOf(value) === index);

  cachedVersion = undefined;
  for (const executable of candidates) {
    try {
      const output = execFileSync(executable, ["--version"], {
        encoding: "utf8",
        timeout: 5_000,
        stdio: ["ignore", "pipe", "ignore"],
      });
      const version = parseClaudeCodeVersion(output);
      if (version) {
        cachedVersion = version;
        break;
      }
    } catch {
      // Keep trying common Claude Code installation locations.
    }
  }
  cacheExpiresAt = now + VERSION_CACHE_MS;
  return cachedVersion;
}
