import os from "node:os";
import path from "node:path";
import { readFile } from "node:fs/promises";
import type { ModelDialectRule } from "./types.js";

export interface EditBackendConfig {
  defaultDialect: string;
  models: ModelDialectRule[];
  enforceSeenLines: boolean;
  transactionDir: string;
}

export function configDirectory(): string {
  return process.env.YEET_CONFIG_DIR ?? path.join(os.homedir(), ".yeet");
}

export async function loadEditBackendConfig(): Promise<EditBackendConfig> {
  const directory = configDirectory();
  const defaults: EditBackendConfig = {
    defaultDialect: "structured",
    models: [],
    enforceSeenLines: true,
    transactionDir: path.join(directory, "transactions"),
  };
  try {
    const parsed = JSON.parse(await readFile(path.join(directory, "edit-backend.json"), "utf8")) as Partial<EditBackendConfig>;
    return {
      defaultDialect: parsed.defaultDialect ?? defaults.defaultDialect,
      models: parsed.models ?? defaults.models,
      enforceSeenLines: parsed.enforceSeenLines ?? defaults.enforceSeenLines,
      transactionDir: parsed.transactionDir ?? defaults.transactionDir,
    };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return defaults;
    throw error;
  }
}


