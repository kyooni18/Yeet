import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import os from "node:os";
import path from "node:path";
import {
  mkdtemp,
  mkdir,
  readFile,
  readdir,
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

test("stale snapshots independently rebase multiple unchanged targets with different offsets", async () => {
  const f = await fixture("top\nA\nmiddle\nB\nbottom\n");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 5 });
    await writeFile(f.file, "zero\ntop\nA\nmiddle\ninserted\nB\nbottom\n", "utf8");
    const result = await f.backend.apply({
      changes: [{
        path: "a.txt",
        snapshot: read.snapshot,
        edits: [
          { kind: "replace", range: { start: 2, end: 2 }, text: "AA" },
          { kind: "replace", range: { start: 4, end: 4 }, text: "BB" },
        ],
      }],
    });

    assert.match(result.files[0].warnings[0], /independently revalidated/);
    assert.equal(
      await readFile(f.file, "utf8"),
      "zero\ntop\nAA\nmiddle\ninserted\nBB\nbottom\n",
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

test("prepared update journal restores an installed replacement from the rollback copy", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "yeet-edit-update-recovery-"));
  try {
    const journalDir = path.join(root, ".transactions");
    const target = path.join(root, "a.txt");
    const backup = path.join(root, ".a.txt.yeet-backup-crash");
    const stage = path.join(root, ".a.txt.yeet-stage-crash");
    await mkdir(journalDir);
    await writeFile(target, "new\n", "utf8");
    await writeFile(backup, "old\n", "utf8");
    const installedDigest = createHash("sha256").update("new\n", "utf8").digest("hex");
    await writeFile(
      path.join(journalDir, "crash.json"),
      JSON.stringify({
        id: "crash",
        state: "prepared",
        ownerPid: 2_147_483_647,
        records: [{
          target,
          stage,
          backup,
          hadOriginal: true,
          installed: true,
          installedDigest,
        }],
      }),
      "utf8",
    );

    const writer = new TransactionalWriter(journalDir);
    await writer.initialize();
    assert.equal(await readFile(target, "utf8"), "old\n");
    assert.deepEqual(await readdir(journalDir), []);
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
