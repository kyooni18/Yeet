import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join } from "node:path";

export interface PlatformDirectoryOptions {
  env?: NodeJS.ProcessEnv;
  platform?: NodeJS.Platform;
  home?: string;
  exists?: (path: string) => boolean;
}

/**
 * Resolve Yeet's per-user state directory consistently with the Rust host.
 * Existing ~/.yeet installs win so upgrades never split Rust and RuntimeSource
 * state. New Linux/Windows installs use the native configuration location.
 */
export function defaultConfigDirectory(options: PlatformDirectoryOptions = {}): string {
  const env = options.env ?? process.env;
  const platform = options.platform ?? process.platform;
  const home = options.home ?? homedir();
  const exists = options.exists ?? existsSync;
  const explicit = env.YEET_CONFIG_DIR?.trim();
  if (explicit) return explicit;

  const legacy = join(home, ".yeet");
  if (exists(legacy) || platform === "darwin") return legacy;

  if (platform === "win32") {
    const base = env.APPDATA?.trim() || join(home, "AppData", "Roaming");
    return join(base, "Yeet");
  }

  const xdg = env.XDG_CONFIG_HOME?.trim();
  if (xdg && isAbsolute(xdg)) return join(xdg, "yeet");
  return join(home, ".config", "yeet");
}
