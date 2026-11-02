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

test("unsafe path permission never bypasses snapshots or seen lines", async () => {
  const f = await fixture("one\ntwo\nthree\n");
  try {
    const edit = { kind: "replace", range: { start: 3, end: 3 }, text: "THREE" };
    await assert.rejects(f.backend.apply({ unsafe: true, changes: [{ path: "a.txt", edits: [edit] }] }),
      error => error.mutationOutcome === "unchanged" && /Snapshot handle is required/.test(error.message));
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 1, unsafe: true });
    await assert.rejects(f.backend.apply({ unsafe: true, changes: [{ path: "a.txt", snapshot: read.snapshot, edits: [edit] }] }),
      error => error.mutationOutcome === "unchanged" && /did not display required lines/.test(error.message));
    await writeFile(f.file, "concurrent\ntwo\nthree\n");
    await assert.rejects(f.backend.apply({ unsafe: true, changes: [{ path: "a.txt", snapshot: read.snapshot,
      edits: [{ kind: "replace", range: { start: 1, end: 1 }, text: "ONE" }] }] }), /stale/);
    await assert.rejects(f.backend.apply({ unsafe: true, changes: [{ path: "a.txt", snapshot: read.snapshot,
      fileOp: { kind: "delete" } }] }), /stale/);
    assert.equal(await readFile(f.file, "utf8"), "concurrent\ntwo\nthree\n");
  } finally { await f.cleanup(); }
});

test("disjoint edits preserve shifted read coverage without crediting unseen diff gaps", async () => {
  const f = await fixture(Array.from({ length: 200 }, (_, i) => `line-${i + 1}`).join("\n") + "\n");
  try {
    await f.backend.read({ path: "a.txt", startLine: 10, endLine: 10 });
    const read = await f.backend.read({ path: "a.txt", startLine: 190, endLine: 191 });
    const result = await f.backend.apply({ changes: [{ path: "a.txt", snapshot: read.snapshot, edits: [
      { kind: "replace", range: { start: 10, end: 10 }, text: "ten-a\nten-b" },
      { kind: "replace", range: { start: 190, end: 190 }, text: "one-ninety-a\none-ninety-b\none-ninety-c" },
    ] }] });
    assert.equal(result.mutationOutcome, "applied");
    assert.equal(result.diagnosticsStatus, "skipped");
    assert.deepEqual(result.files[0].seenRanges, [{ start: 10, end: 11 }, { start: 191, end: 194 }]);
    assert.equal(result.files[0].revision, createHash("sha256").update(await readFile(f.file)).digest("hex"));
    assert.equal(result.files[0].diff.hunks[0].truncated, true);
    await assert.rejects(f.backend.apply({ changes: [{ path: "a.txt", snapshot: result.files[0].snapshot,
      edits: [{ kind: "replace", range: { start: 100, end: 100 }, text: "unseen" }] }] }), /did not display required lines/);
    await f.backend.apply({ changes: [{ path: "a.txt", snapshot: result.files[0].snapshot,
      edits: [{ kind: "replace", range: { start: 194, end: 194 }, text: "known shifted line" }] }] });
  } finally { await f.cleanup(); }
});

test("large supplied files retain complete coverage even with shortened edit previews", async () => {
  const f = await fixture("existing\n");
  try {
    const text = Array.from({ length: 240 }, (_, i) => `supplied-${i + 1}`).join("\n") + "\n";
    const result = await f.backend.apply({ changes: [{ path: "created.txt", fileOp: { kind: "create", text } }] });
    assert.deepEqual(result.files[0].seenRanges, [{ start: 1, end: 240 }]);
    await f.backend.apply({ changes: [{ path: "created.txt", snapshot: result.files[0].snapshot,
      edits: [{ kind: "replace", range: { start: 150, end: 150 }, text: "updated supplied line" }] }] });
    assert.match(await readFile(path.join(f.root, "created.txt"), "utf8"), /updated supplied line/);
  } finally { await f.cleanup(); }
});

test("limiting daemon coverage prevents older broad reads from authorizing later edits", async () => {
  const f = await fixture("one\ntwo\nthree\n");
  try {
    const read = await f.backend.read({ path: "a.txt", startLine: 1, endLine: 3 });
    const revision = createHash("sha256").update("one\ntwo\nthree\n").digest("hex");
    assert.throws(() => f.backend.limitSnapshotCoverage(read.snapshot, "wrong", [{ start: 1, end: 1 }]), /changed while binding/);
    f.backend.limitSnapshotCoverage(read.snapshot, revision, [{ start: 1, end: 1 }]);
    f.backend.limitSnapshotCoverage(read.snapshot, revision, [{ start: 1, end: 3 }]);
    await assert.rejects(f.backend.apply({ changes: [{ path: "a.txt", snapshot: read.snapshot,
      edits: [{ kind: "replace", range: { start: 3, end: 3 }, text: "unseen" }] }] }), /did not display required lines/);
  } finally { await f.cleanup(); }
});

test("diagnostics failures after commit report applied without inviting a duplicate edit", async () => {
  const f = await fixture("one\n");
  try {
    const backend = new EditBackend({ root: f.root, transactionDir: path.join(f.root, ".diagnostic-transactions"),
      diagnostics: { diagnose: async () => { throw new Error("diagnostics unavailable"); } } });
    await backend.initialize();
    const read = await backend.read({ path: "a.txt" });
    const result = await backend.apply({ changes: [{ path: "a.txt", snapshot: read.snapshot,
      edits: [{ kind: "replace", range: { start: 1, end: 1 }, text: "ONE" }] }] });
    assert.equal(result.mutationOutcome, "applied");
    assert.equal(result.diagnosticsStatus, "failed");
    assert.equal(result.diagnosticsError, "diagnostics unavailable");
    assert.match(result.files[0].snapshot, /^s_/);
    assert.equal(await readFile(f.file, "utf8"), "ONE\n");
  } finally { await f.cleanup(); }
});

test("unknown commit failures remain unknown instead of claiming unchanged", async () => {
  const f = await fixture("one\n");
  try {
    const read = await f.backend.read({ path: "a.txt" });
    f.backend.transaction.commit = async () => { throw new Error("commit response unavailable"); };
    await assert.rejects(f.backend.apply({ changes: [{ path: "a.txt", snapshot: read.snapshot,
      edits: [{ kind: "replace", range: { start: 1, end: 1 }, text: "ONE" }] }] }),
      error => error.mutationOutcome === "unknown" && error.code === "edit_commit_failed");
  } finally { await f.cleanup(); }
});

test("diagnostics status distinguishes reported errors, completed checks, and explicit skips", async () => {
  const f = await fixture("one\n");
  try {
    let severity = "error";
    let calls = 0;
    const backend = new EditBackend({ root: f.root, transactionDir: path.join(f.root, ".status-transactions"),
      diagnostics: { diagnose: async () => { calls++; return [{ path: "a.txt", severity, message: "check result" }]; } } });
    await backend.initialize();
    let snapshot = (await backend.read({ path: "a.txt" })).snapshot;
    for (const [text, expectedStatus, diagnostics] of [["two", "failed", true], ["three", "passed", true], ["four", "skipped", false]]) {
      const result = await backend.apply({ diagnostics, changes: [{ path: "a.txt", snapshot,
        edits: [{ kind: "replace", range: { start: 1, end: 1 }, text }] }] });
      assert.equal(result.mutationOutcome, "applied");
      assert.equal(result.diagnosticsStatus, expectedStatus);
      assert.equal(result.diagnosticsError, undefined);
      snapshot = result.files[0].snapshot;
      severity = "warning";
    }
    assert.equal(calls, 2);
  } finally { await f.cleanup(); }
});

test("created snapshots use canonical identities through a workspace alias", async () => {
  const parent = await mkdtemp(path.join(os.tmpdir(), "yeet-edit-root-alias-"));
  try {
    const actual = path.join(parent, "actual");
    const alias = path.join(parent, "alias");
    await mkdir(actual);
    await symlink(actual, alias, "dir");
    const backend = new EditBackend({ root: alias, transactionDir: path.join(actual, ".transactions") });
    await backend.initialize();
    const created = await backend.apply({ changes: [{ path: "nested/new.txt", fileOp: { kind: "create", text: "new\n" } }] });
    assert.equal(created.files[0].path, "nested/new.txt");
    await backend.apply({ changes: [{ path: "nested/new.txt", snapshot: created.files[0].snapshot,
      edits: [{ kind: "replace", range: { start: 1, end: 1 }, text: "updated" }] }] });
    assert.equal(await readFile(path.join(actual, "nested/new.txt"), "utf8"), "updated\n");
    await symlink(path.join(actual, "nested/new.txt"), path.join(actual, "leaf-link.txt"));
    await assert.rejects(backend.read({ path: "leaf-link.txt" }), /symbolic link/);
  } finally { await rm(parent, { recursive: true, force: true }); }
});
