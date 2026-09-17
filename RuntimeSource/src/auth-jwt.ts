//! JWT metadata extraction used by browser/OAuth authentication flows.

function jwtPayload(token: string | undefined): Record<string, unknown> | undefined {
  if (!token) return undefined;
  try {
    const encoded = token.split(".")[1];
    if (!encoded) return undefined;
    const value: unknown = JSON.parse(Buffer.from(encoded, "base64url").toString("utf8"));
    return value && typeof value === "object" && !Array.isArray(value)
      ? value as Record<string, unknown>
      : undefined;
  } catch {
    return undefined;
  }
}

export function jwtExpiresAt(token: string | undefined): string | undefined {
  const payload = jwtPayload(token);
  return typeof payload?.exp === "number" && Number.isFinite(payload.exp)
    ? new Date(payload.exp * 1_000).toISOString()
    : undefined;
}

export function accountIdFromJwt(token: string | undefined): string | undefined {
  const payload = jwtPayload(token);
  const auth = payload?.["https://api.openai.com/auth"];
  if (!auth || typeof auth !== "object" || Array.isArray(auth)) return undefined;
  const accountId = (auth as Record<string, unknown>).chatgpt_account_id;
  return typeof accountId === "string" ? accountId : undefined;
}
