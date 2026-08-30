import assert from "node:assert/strict";
import os from "node:os";
import path from "node:path";
import {
  mkdtemp,
  readFile,
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
    assert.equal(await readFile(f.file, "utf8"), "one\ntwo\nTHREE\nfour\n");
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
  } finally {
    await f.cleanup();
  }
});
