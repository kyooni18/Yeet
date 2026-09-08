import assert from "node:assert/strict";
import os from "node:os";
import path from "node:path";
import {
  mkdtemp,
  mkdir,
  readFile,
  realpath,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import test from "node:test";

import {
  EditBackend,
  TransactionalWriter,
} from "../dist/index.js";

async function fixture(content, extension = "txt") {
  const root = await mkdtemp(path.join(os.tmpdir(), "yeet-edit-runtime-"));
  const file = path.join(root, `a.${extension}`);
  await writeFile(file, content, "utf8");
  const backend = new EditBackend({
    root,
    transactionDir: path.join(root, ".transactions"),
  });
  await backend.initialize();
  return {
    root,
    file,
    backend,
    cleanup: () => rm(root, { recursive: true, force: true }),
  };
}

test("read snapshots enforce seen-line provenance and return a fresh snapshot", async () => {
  const f = await fixture("one\ntwo\nthree\nfour\n");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 2 });
    assert.equal(read.numbered, "1:one\n2:two");
    assert.match(read.anchored, /^1:[0-9a-f]{4}\|one\n2:[0-9a-f]{4}\|two$/);
    await assert.rejects(
      f.backend.apply({
        changes: [{
          path: "a.txt",
          snapshot: read.snapshot,
          edits: [{ kind: "replace", range: { start: 3, end: 3 }, text: "THREE" }],
        }],
      }),
      /did not display required lines/,
    );

    const full = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 4 });
    const applied = await f.backend.apply({
      changes: [{
        path: "a.txt",
        snapshot: full.snapshot,
        edits: [{ kind: "replace", range: { start: 3, end: 3 }, text: "THREE" }],
      }],
    });
    assert.match(applied.files[0].snapshot, /^s_/);
    assert.match(applied.files[0].anchors, /3:[0-9a-f]{4}\|THREE/);
    assert.equal(await readFile(f.file, "utf8"), "one\ntwo\nTHREE\nfour\n");
  } finally {
    await f.cleanup();
  }
});

test("read clamps default and explicit end lines to EOF", async () => {
  const f = await fixture("one\ntwo\nthree\n");
  try {
    const defaultRead = await f.backend.read({ path: "a.txt" });
    assert.equal(defaultRead.startLine, 1);
    assert.equal(defaultRead.endLine, 3);
    assert.equal(defaultRead.totalLines, 3);
    assert.equal(defaultRead.content, "one\ntwo\nthree");

    const oversizedRead = await f.backend.read({ path: "a.txt", startLine: 2, endLine: 400 });
    assert.equal(oversizedRead.startLine, 2);
    assert.equal(oversizedRead.endLine, 3);
    assert.equal(oversizedRead.content, "two\nthree");
  } finally {
    await f.cleanup();
  }
});

test("read defaults to a bounded 160-line source page", async () => {
  const content = Array.from({ length: 220 }, (_, index) => `line-${index + 1}`).join("\n");
  const f = await fixture(content);
  try {
    const read = await f.backend.read({ path: "a.txt" });
    assert.equal(read.startLine, 1);
    assert.equal(read.endLine, 160);
    assert.equal(read.totalLines, 220);
  } finally {
    await f.cleanup();
  }
});

test("workspace search returns compact matches and skips generated dependency trees", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "yeet-search-runtime-"));
  try {
    await mkdir(path.join(root, "Sources"), { recursive: true });
    await mkdir(path.join(root, "node_modules", "noise"), { recursive: true });
    await mkdir(path.join(root, "built", "localized"), { recursive: true });
    await mkdir(path.join(root, ".yeet", "context-artifacts", "session"), { recursive: true });
    await writeFile(path.join(root, "Sources", "A.swift"), "one\nNeedle here\nthree\n", "utf8");
    await writeFile(path.join(root, "Sources", "B.swift"), "needle lower\n", "utf8");
    await writeFile(path.join(root, "node_modules", "noise", "ignored.js"), "needle ignored\n", "utf8");
    await writeFile(path.join(root, "built", "localized", "ignored.js"), "needle built\n", "utf8");
    await writeFile(path.join(root, ".yeet", "context-artifacts", "session", "ignored.txt"), "needle yeet artifact\n", "utf8");
    const backend = new EditBackend({ root, transactionDir: path.join(root, ".transactions") });
    await backend.initialize();

    const result = await backend.search({ query: "needle", maxResults: 10 });
    assert.deepEqual(result.matches.map((item) => `${item.path}:${item.line}`), ["Sources/A.swift:2", "Sources/B.swift:1"]);
    assert.equal(result.truncated, false);
    assert.equal(result.filesScanned, 2);
    await assert.rejects(
      backend.search({ query: "needle", path: ".yeet" }),
      /generated or internal workspace state/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("workspace file listing discovers files without exposing generated or Yeet state", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "yeet-list-runtime-"));
  try {
    await mkdir(path.join(root, "Sources", "Core"), { recursive: true });
    await mkdir(path.join(root, ".yeet", "context-artifacts"), { recursive: true });
    await mkdir(path.join(root, "node_modules", "pkg"), { recursive: true });
    await writeFile(path.join(root, "copy.swift"), "print(1)\n", "utf8");
    await writeFile(path.join(root, "Sources", "Core", "A.swift"), "struct A {}\n", "utf8");
    await writeFile(path.join(root, ".yeet", "context-artifacts", "noise.txt"), "noise\n", "utf8");
    await writeFile(path.join(root, "node_modules", "pkg", "noise.js"), "noise\n", "utf8");
    const backend = new EditBackend({ root, transactionDir: path.join(root, ".transactions") });
    await backend.initialize();

    const result = await backend.listFiles({ maxDepth: 4, maxResults: 20 });
    assert.deepEqual(result.entries, [
      { path: "Sources", kind: "directory" },
      { path: "Sources/Core", kind: "directory" },
      { path: "Sources/Core/A.swift", kind: "file" },
      { path: "copy.swift", kind: "file" },
    ]);
    assert.equal(result.truncated, false);
    await assert.rejects(
      backend.listFiles({ path: ".yeet" }),
      /generated or internal workspace state/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("workspace search is literal by default and supports explicit regex", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "yeet-search-regex-"));
  try {
    await mkdir(path.join(root, "Sources"), { recursive: true });
    await writeFile(path.join(root, "Sources", "A.swift"), "useRouter here\nDI.router here\nuseRouter|DI\\.router literal\n", "utf8");
    const backend = new EditBackend({ root, transactionDir: path.join(root, ".transactions") });
    await backend.initialize();

    const literal = await backend.search({ query: "useRouter|DI\\.router" });
    assert.deepEqual(literal.matches.map((item) => item.line), [3]);

    const regex = await backend.search({ query: "useRouter|DI\\.router", regex: true });
    assert.deepEqual(regex.matches.map((item) => item.line), [1, 2, 3]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("line hashes reject misaddressed edits before touching the file", async () => {
  const f = await fixture("one\ntwo\nthree\n");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 3 });
    const second = read.anchored.split("\n")[1];
    const hash = /^2:([0-9a-f]{4})\|/.exec(second)?.[1];
    assert.ok(hash);

    await assert.rejects(
      f.backend.apply({
        changes: [{
          path: "a.txt",
          snapshot: read.snapshot,
          edits: [{
            kind: "replace",
            range: { start: 2, end: 2, startHash: "0000", endHash: "0000" },
            text: "TWO",
          }],
        }],
      }),
      /anchor mismatch/,
    );
    assert.equal(await readFile(f.file, "utf8"), "one\ntwo\nthree\n");

    await f.backend.apply({
      changes: [{
        path: "a.txt",
        snapshot: read.snapshot,
        edits: [{
          kind: "replace",
          range: { start: 2, end: 2, startHash: hash, endHash: hash },
          text: "TWO",
        }],
      }],
    });
    assert.equal(await readFile(f.file, "utf8"), "one\nTWO\nthree\n");
  } finally {
    await f.cleanup();
  }
});

test("stale snapshots rebase one consistent offset and fail closed on changed anchors", async () => {
  const f = await fixture("one\ntwo\nthree\nfour\n");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 2, endLine: 4 });
    await writeFile(f.file, "zero\none\ntwo\nthree\nfour\n", "utf8");
    const rebased = await f.backend.apply({
      changes: [{
        path: "a.txt",
        snapshot: read.snapshot,
        edits: [{ kind: "replace", range: { start: 3, end: 3 }, text: "THREE" }],
      }],
    });
    assert.match(rebased.files[0].warnings[0], /rebased by \+1 lines/);
    assert.equal(await readFile(f.file, "utf8"), "zero\none\ntwo\nTHREE\nfour\n");

    const stale = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 5 });
    await writeFile(f.file, "zero\none\ntwo\nchanged\nfour\n", "utf8");
    await assert.rejects(
      f.backend.apply({
        changes: [{
          path: "a.txt",
          snapshot: stale.snapshot,
          edits: [{ kind: "replace", range: { start: 4, end: 4 }, text: "THREE" }],
        }],
      }),
      /cannot be safely rebased/,
    );
  } finally {
    await f.cleanup();
  }
});

test("multi-file edits commit as one transaction", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "yeet-edit-multi-"));
  try {
    const aFile = path.join(root, "a.txt");
    const bFile = path.join(root, "b.txt");
    await writeFile(aFile, "a1\na2\n", "utf8");
    await writeFile(bFile, "b1\nb2\n", "utf8");
    const backend = new EditBackend({ root, transactionDir: path.join(root, ".transactions") });
    await backend.initialize();
    const a = await backend.read({ path: "a.txt", startLine: 1, endLine: 2 });
    const b = await backend.read({ path: "b.txt", startLine: 1, endLine: 2 });
    await backend.apply({
      changes: [
        { path: "a.txt", snapshot: a.snapshot, edits: [{ kind: "replace", range: { start: 2, end: 2 }, text: "A2" }] },
        { path: "b.txt", snapshot: b.snapshot, edits: [{ kind: "replace", range: { start: 1, end: 1 }, text: "B1" }] },
      ],
    });
    assert.equal(await readFile(aFile, "utf8"), "a1\nA2\n");
    assert.equal(await readFile(bFile, "utf8"), "B1\nb2\n");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("hashline, unified patch, and sloppy dialects share the snapshot-safe apply path", async () => {
  const f = await fixture("one\ntwo\nthree\n");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 3 });
    await f.backend.applyDialect(
      `[a.txt@${read.snapshot}]\nPUT 2.=2:\n+TWO\n`,
      { dialect: "hashline" },
    );
    const next = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 3 });
    await f.backend.applyDialect(
      [
        "--- a/a.txt",
        "+++ b/a.txt",
        "@@ -2,1 +2,1 @@",
        "-TWO",
        "+two",
        "",
      ].join("\n"),
      { dialect: "apply_patch", snapshots: { "a.txt": next.snapshot } },
    );
    const finalRead = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 3 });
    await f.backend.applyDialect(
      `[a.txt@${finalRead.snapshot}]\n<<<<<<< SEARCH\nthree\n=======\nTHREE\n>>>>>>> REPLACE\n`,
      { dialect: "sloppy" },
    );
    assert.equal(await readFile(f.file, "utf8"), "one\ntwo\nTHREE\n");
  } finally {
    await f.cleanup();
  }
});

test("unified patch rejects hunk content that disagrees with its snapshot", async () => {
  const f = await fixture("one\ntwo\nthree\n");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 3 });
    await assert.rejects(
      f.backend.applyDialect(
        [
          "--- a/a.txt",
          "+++ b/a.txt",
          "@@ -2,1 +2,1 @@",
          "-not-two",
          "+TWO",
          "",
        ].join("\n"),
        { dialect: "apply_patch", snapshots: { "a.txt": read.snapshot } },
      ),
      /context does not match snapshot/,
    );
    assert.equal(await readFile(f.file, "utf8"), "one\ntwo\nthree\n");
  } finally {
    await f.cleanup();
  }
});

test("hashline block edits use the configured resolver", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "yeet-edit-block-"));
  try {
    const file = path.join(root, "a.ts");
    await writeFile(file, "before\nfunction x() {\n  return 1;\n}\nafter\n", "utf8");
    const backend = new EditBackend({
      root,
      transactionDir: path.join(root, ".transactions"),
      blockResolver: ({ line }) => line === 2 ? { start: 2, end: 4 } : null,
    });
    await backend.initialize();
    const read = await backend.read({ path: "a.ts", startLine: 1, endLine: 5 });
    await backend.applyDialect(`[a.ts@${read.snapshot}]\nCUT 2*\n`, { dialect: "hashline" });
    assert.equal(await readFile(file, "utf8"), "before\nafter\n");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("adjacent replacement and insertion edits are applied without overlap ambiguity", async () => {
  const f = await fixture("one\ntwo\n");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 2 });
    await f.backend.apply({
      changes: [{
        path: "a.txt",
        snapshot: read.snapshot,
        edits: [
          { kind: "replace", range: { start: 1, end: 1 }, text: "ONE" },
          { kind: "insert", at: { kind: "after", line: 1 }, text: "between" },
        ],
      }],
    });
    assert.equal(await readFile(f.file, "utf8"), "ONE\nbetween\ntwo\n");
  } finally {
    await f.cleanup();
  }
});

test("transactions preserve BOM/CRLF and reject symlink and traversal paths", async () => {
  const f = await fixture("\uFEFFone\r\ntwo\r\n", "txt");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 2 });
    await f.backend.apply({
      changes: [{
        path: "a.txt",
        snapshot: read.snapshot,
        edits: [{ kind: "replace", range: { start: 2, end: 2 }, text: "TWO" }],
      }],
    });
    assert.equal(await readFile(f.file, "utf8"), "\uFEFFone\r\nTWO\r\n");

    await assert.rejects(f.backend.read({ path: "../outside.txt" }), /escapes workspace root/);
    const link = path.join(f.root, "link.txt");
    await symlink(f.file, link);
    await assert.rejects(f.backend.read({ path: "link.txt" }), /symbolic link/);
  } finally {
    await f.cleanup();
  }
});

test("unsafe outside access permits snapshot-free edits beyond the project root", async () => {
  const parent = await mkdtemp(path.join(os.tmpdir(), "yeet-edit-unlimited-"));
  const root = path.join(parent, "project");
  const external = path.join(parent, "outside.txt");
  try {
    await mkdir(root);
    await writeFile(external, "outside\n", "utf8");
    const backend = new EditBackend({
      root,
      allowOutside: true,
      transactionDir: path.join(root, ".transactions"),
    });
    await backend.initialize();
    const read = await backend.read({ path: external, startLine: 1, endLine: 1, unsafe: true });
    assert.equal(read.path, await realpath(external));
    await backend.apply({
      unsafe: true,
      changes: [{
        path: external,
        edits: [{ kind: "replace", range: { start: 1, end: 1 }, text: "changed" }],
      }],
    });
    assert.equal(await readFile(external, "utf8"), "changed\n");

    const link = path.join(root, "outside-link.txt");
    await symlink(external, link);
    const linked = await backend.read({ path: link, startLine: 1, endLine: 1, unsafe: true });
    assert.equal(linked.content, "changed");
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

test("transaction writer rolls back when the live digest differs", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "yeet-edit-transaction-"));
  try {
    const file = path.join(root, "a.txt");
    const journalDir = path.join(root, ".transactions");
    await writeFile(file, "original\n", "utf8");
    const writer = new TransactionalWriter(journalDir);
    await writer.initialize();
    await assert.rejects(
      writer.commit([{ path: file, content: "new\n", expectedDigest: "00".repeat(32) }]),
      /changed after preflight/,
    );
    assert.equal(await readFile(file, "utf8"), "original\n");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("move edits are committed transactionally and return a destination snapshot", async () => {
  const f = await fixture("one\ntwo\n");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 2 });
    const result = await f.backend.apply({
      changes: [{
        path: "a.txt",
        snapshot: read.snapshot,
        edits: [{ kind: "replace", range: { start: 2, end: 2 }, text: "TWO" }],
        fileOp: { kind: "move", destination: "b.txt" },
      }],
    });
    await assert.rejects(readFile(f.file, "utf8"));
    assert.equal(await readFile(path.join(f.root, "b.txt"), "utf8"), "one\nTWO\n");
    assert.match(result.files[0].snapshot, /^s_/);
    assert.match(result.files[0].anchors, /2:[0-9a-f]{4}\|TWO/);
  } finally {
    await f.cleanup();
  }
});
