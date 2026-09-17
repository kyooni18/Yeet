#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const cargoText = readFileSync(join(root, "Cargo.toml"), "utf8");
const packageStart = cargoText.indexOf("[package]");
if (packageStart < 0) throw new Error("Cargo.toml does not contain a [package] section");
const afterPackage = cargoText.slice(packageStart + "[package]".length);
const nextSection = afterPackage.search(/^\[/m);
const packageBlock = nextSection >= 0 ? afterPackage.slice(0, nextSection) : afterPackage;
const cargoVersion = /^version\s*=\s*"([^"]+)"\s*$/m.exec(packageBlock)?.[1];
if (!cargoVersion) {
  throw new Error("Unable to determine [package] version from Cargo.toml");
}

const runtimeVersion = JSON.parse(readFileSync(join(root, "RuntimeSource", "package.json"), "utf8")).version;
const webVersion = JSON.parse(readFileSync(join(root, "web", "package.json"), "utf8")).version;
const versions = new Map([
  ["Cargo.toml", cargoVersion],
  ["RuntimeSource/package.json", runtimeVersion],
  ["web/package.json", webVersion],
]);

const semver = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
for (const [source, version] of versions) {
  if (typeof version !== "string" || !semver.test(version)) {
    throw new Error(`${source} has an invalid release version: ${String(version)}`);
  }
  if (version !== cargoVersion) {
    throw new Error(`${source} version ${version} does not match Cargo.toml version ${cargoVersion}`);
  }
}

const requested = process.argv[2] ?? process.env.YEET_VERSION;
if (requested) {
  const expected = requested.trim().replace(/^v/, "");
  if (!semver.test(expected)) {
    throw new Error(`Invalid requested release version: ${requested}`);
  }
  if (expected !== cargoVersion) {
    throw new Error(`Requested release ${requested} does not match repository version ${cargoVersion}`);
  }
}

console.log(`Release versions are synchronized at ${cargoVersion}.`);
