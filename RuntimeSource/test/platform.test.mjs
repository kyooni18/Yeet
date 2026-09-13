import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import { defaultConfigDirectory } from "../dist/index.js";

const neverExists = () => false;

test("defaultConfigDirectory preserves an explicit override", () => {
  assert.equal(defaultConfigDirectory({ env: { YEET_CONFIG_DIR: "/custom/yeet" }, platform: "linux", home: "/home/test", exists: neverExists }), "/custom/yeet");
});

test("defaultConfigDirectory preserves an existing legacy directory on every platform", () => {
  const home = path.resolve("legacy-home");
  const legacy = path.join(home, ".yeet");
  assert.equal(defaultConfigDirectory({ env: {}, platform: "win32", home, exists: (candidate) => candidate === legacy }), legacy);
});

test("defaultConfigDirectory uses XDG for a clean Linux install", () => {
  const home = path.resolve("linux-home");
  const xdg = path.resolve("xdg-config");
  assert.equal(defaultConfigDirectory({ env: { XDG_CONFIG_HOME: xdg }, platform: "linux", home, exists: neverExists }), path.join(xdg, "yeet"));
});

test("defaultConfigDirectory uses roaming AppData for a clean Windows install", () => {
  const home = path.resolve("windows-home");
  const appData = path.resolve("roaming-app-data");
  assert.equal(defaultConfigDirectory({ env: { APPDATA: appData }, platform: "win32", home, exists: neverExists }), path.join(appData, "Yeet"));
});
