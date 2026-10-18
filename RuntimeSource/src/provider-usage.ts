import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import type { ProviderUsageStatus, ProviderUsageWindow } from "./auth.js";
import { claudeCodeExecutable } from "./claude-version.js";

const execFileAsync = promisify(execFile);

function asObject(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return value as Record<string, unknown>;
}

export function numeric(value: unknown): number | undefined {
  const parsed = typeof value === "number" ? value : typeof value === "string" && value.trim() ? Number(value) : Number.NaN;
  return Number.isFinite(parsed) ? parsed : undefined;
}

export function percent(value: unknown, fraction = false): number | undefined {
  const parsed = numeric(value);
  if (parsed === undefined) return undefined;
  const scaled = fraction && parsed >= 0 && parsed <= 1 ? parsed * 100 : parsed;
  return Math.max(0, Math.min(100, Math.round(scaled)));
}

export function resetIso(value: unknown): string | undefined {
  if (typeof value === "string" && value.trim()) {
    const milliseconds = Date.parse(value);
    return Number.isFinite(milliseconds) ? new Date(milliseconds).toISOString() : undefined;
  }
  const parsed = numeric(value);
  if (parsed === undefined || parsed <= 0) return undefined;
  const milliseconds = parsed > 10_000_000_000 ? parsed : parsed * 1_000;
  return new Date(milliseconds).toISOString();
}

export function durationLabel(seconds: unknown, fallback: string): string {
  const parsed = numeric(seconds);
  if (parsed === undefined || parsed <= 0) return fallback;
  if (parsed % 604_800 === 0) return `${parsed / 604_800}w`;
  if (parsed % 86_400 === 0) return `${parsed / 86_400}d`;
  if (parsed % 3_600 === 0) return `${parsed / 3_600}h`;
  return fallback;
}

export function usageWindow(
  id: string,
  label: string,
  usedPercent: number,
  resetsAt?: string,
): ProviderUsageWindow {
  return {
    id,
    label,
    usedPercent,
    remainingPercent: Math.max(0, 100 - usedPercent),
    ...(resetsAt ? { resetsAt } : {}),
  };
}

export function unavailableUsage(provider: string, source: string, message: string, plan?: string): ProviderUsageStatus {
  return {
    provider,
    available: false,
    source,
    fetchedAt: new Date().toISOString(),
    ...(plan ? { plan } : {}),
    windows: [],
    message,
  };
}

export function claudeUsageLabel(id: string): string {
  const labels: Record<string, string> = {
    five_hour: "5h",
    seven_day: "7d",
    seven_day_oauth_apps: "OAuth apps 7d",
    seven_day_opus: "Opus 7d",
    seven_day_sonnet: "Sonnet 7d",
    seven_day_overage_included: "Model 7d",
    session: "5h",
    weekly_all: "7d",
    weekly_scoped: "Model 7d",
    cinder_cove: "Cinder Cove",
    overage: "Overage",
  };
  return labels[id] ?? id.replaceAll("_", " ");
}

type ClaudeStoredToken = { token?: string; expired: boolean };

function claudeTokenFromPayload(value: unknown): ClaudeStoredToken {
  const payload = asObject(value);
  const oauth = asObject(payload.claudeAiOauth);
  const token = typeof oauth.accessToken === "string"
    ? oauth.accessToken.trim()
    : typeof payload.accessToken === "string"
      ? payload.accessToken.trim()
      : "";
  const expiresAt = oauth.expiresAt ?? payload.expiresAt;
  const expiry = typeof expiresAt === "number"
    ? expiresAt
    : typeof expiresAt === "string" && expiresAt.trim()
      ? (Number.isFinite(Number(expiresAt)) ? Number(expiresAt) : Date.parse(expiresAt))
      : undefined;
  // Keep a minute of headroom so a request does not start with a dying token.
  if (expiry !== undefined && Number.isFinite(expiry) && expiry <= Date.now() + 60_000) {
    return { expired: Boolean(token) };
  }
  return { ...(token ? { token } : {}), expired: false };
}

/**
 * Resolve only the credential needed to call Anthropic's server-side usage API.
 * Usage itself is never read from Claude Code state, files, control protocol, or
 * local caches. Environment auth follows Claude Code's documented precedence;
 * the file/keychain branches only recover the same subscription OAuth token.
 */
export async function resolveClaudeOAuthToken(): Promise<string | undefined> {
  const environment = process.env.CLAUDE_CODE_OAUTH_TOKEN?.trim();
  if (environment) return environment;

  const stored = await readStoredClaudeToken();
  if (stored.token || !stored.expired) return stored.token;
  if (process.env.YEET_CLAUDE_CODE_AUTO_REFRESH === "0") return undefined;
  await refreshClaudeCodeCredential();
  return (await readStoredClaudeToken()).token;
}

async function readStoredClaudeToken(): Promise<ClaudeStoredToken> {
  const configuredPath = process.env.CLAUDE_CREDENTIALS_PATH?.trim();
  const credentialPath = configuredPath || join(homedir(), ".claude", ".credentials.json");
  let expired = false;
  try {
    const stored = claudeTokenFromPayload(JSON.parse(await readFile(credentialPath, "utf8")));
    if (stored.token) return stored;
    expired ||= stored.expired;
  } catch {
    // Native macOS installs normally use Keychain instead of this file.
  }

  if (process.platform === "darwin") {
    try {
      const { stdout } = await execFileAsync(
        "/usr/bin/security",
        ["find-generic-password", "-s", "Claude Code-credentials", "-w"],
        { encoding: "utf8", timeout: 2_000, maxBuffer: 64 * 1024 },
      );
      const stored = claudeTokenFromPayload(JSON.parse(stdout));
      if (stored.token) return stored;
      expired ||= stored.expired;
    } catch {
      // No subscription login is available to query directly.
    }
  }
  return { expired };
}

let claudeRefresh: Promise<void> | undefined;

/**
 * Claude Code owns its subscription credential and rotates the refresh token
 * whenever it refreshes, so Yeet never refreshes it directly. A minimal Claude
 * Code run lets Claude Code refresh and persist its own credential instead.
 */
async function refreshClaudeCodeCredential(): Promise<void> {
  claudeRefresh ??= execFileAsync(
    claudeCodeExecutable(),
    ["-p", "--model", "haiku", "--max-turns", "1", "--tools", "", "Reply with OK."],
    { encoding: "utf8", timeout: 90_000, maxBuffer: 1024 * 1024, cwd: homedir() },
  ).then(() => undefined, () => undefined).finally(() => {
    claudeRefresh = undefined;
  });
  await claudeRefresh;
}
