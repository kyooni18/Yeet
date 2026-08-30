import os from "node:os";
import path from "node:path";
import { readFile } from "node:fs/promises";
export function configDirectory() {
    return process.env.YEET_CONFIG_DIR ?? path.join(os.homedir(), ".yeet");
}
export async function loadEditBackendConfig() {
    const directory = configDirectory();
    const defaults = {
        defaultDialect: "structured",
        models: [],
        enforceSeenLines: true,
        transactionDir: path.join(directory, "transactions"),
    };
    try {
        const parsed = JSON.parse(await readFile(path.join(directory, "edit-backend.json"), "utf8"));
        return {
            defaultDialect: parsed.defaultDialect ?? defaults.defaultDialect,
            models: parsed.models ?? defaults.models,
            enforceSeenLines: parsed.enforceSeenLines ?? defaults.enforceSeenLines,
            transactionDir: parsed.transactionDir ?? defaults.transactionDir,
        };
    }
    catch (error) {
        if (error.code === "ENOENT")
            return defaults;
        throw error;
    }
}
//# sourceMappingURL=config.js.map