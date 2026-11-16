import assert from "node:assert/strict";
import test from "node:test";
import { execFile } from "node:child_process";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";

import { parseCodexCliVersion } from "../dist/codex-version.js";

test("Codex CLI version parsing tracks the installed client version", () => {
  assert.equal(parseCodexCliVersion("codex-cli 0.154.0\n"), "0.154.0");
  assert.equal(parseCodexCliVersion("codex-cli v1.2.3-beta.1\n"), "1.2.3-beta.1");
  assert.equal(parseCodexCliVersion("not a version"), undefined);
});

test("Codex version discovery finds Linux npm installs without blocking the bridge", { skip: process.platform === "win32" }, async () => {
  const home = await mkdtemp(path.join(os.tmpdir(), "yeet-codex-version-"));
  try {
    const bin = path.join(home, ".nvm", "versions", "node", "v22.0.0", "bin");
    await mkdir(bin, { recursive: true });
    await writeFile(path.join(bin, "codex"), '#!/usr/bin/env node\nsetTimeout(() => console.log("codex-cli 0.154.0"), 20);\n', { mode: 0o755 });
    const { stdout } = await promisify(execFile)(process.execPath, ["--input-type=module", "-e", `
      import { codexCliVersion } from ${JSON.stringify(new URL("../dist/codex-version.js", import.meta.url).href)};
      let responsive = false;
      setImmediate(() => { responsive = true; });
      const versions = await Promise.all([codexCliVersion(), codexCliVersion()]);
      console.log(JSON.stringify({ versions, responsive }));
    `], {
      env: { ...process.env, HOME: home, PATH: "", NVM_BIN: bin, YEET_CODEX_BIN: "", YEET_CODEX_CLIENT_VERSION: "" },
      timeout: 5_000,
    });
    assert.deepEqual(JSON.parse(stdout), { versions: ["0.154.0", "0.154.0"], responsive: true });
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});
