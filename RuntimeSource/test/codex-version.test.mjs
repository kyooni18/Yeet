import assert from "node:assert/strict";
import test from "node:test";

import { parseCodexCliVersion } from "../dist/codex-version.js";

test("Codex CLI version parsing tracks the installed client version", () => {
  assert.equal(parseCodexCliVersion("codex-cli 0.154.0\n"), "0.154.0");
  assert.equal(parseCodexCliVersion("codex-cli v1.2.3-beta.1\n"), "1.2.3-beta.1");
  assert.equal(parseCodexCliVersion("not a version"), undefined);
});
