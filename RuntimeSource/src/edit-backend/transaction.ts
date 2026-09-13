import path from "node:path";
import { createHash, randomBytes } from "node:crypto";
import { access, chmod, link, mkdir, open, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { configDirectory } from "./config.js";

export interface PlannedFileState {
  path: string;
  content?: string;
  mode?: number;
  expectedDigest?: string;
  expectMissing?: boolean;
}

interface JournalRecord {
  target: string;
  stage?: string;
  backup: string;
  hadOriginal: boolean;
  expectedDigest?: string;
  expectMissing?: boolean;
  installed?: boolean;
  installedDigest?: string;
}

interface Journal {
  id: string;
  state: "prepared" | "committed";
  records: JournalRecord[];
  ownerPid?: number;
}

function isJournal(value: unknown): value is Journal {
  if (!value || typeof value !== "object") return false;
  const journal = value as Partial<Journal>;
  if (typeof journal.id !== "string" || !["prepared", "committed"].includes(journal.state ?? "")) return false;
  if (!Array.isArray(journal.records)) return false;
  return journal.records.every(record => {
    if (!record || typeof record !== "object") return false;
    const candidate = record as Partial<JournalRecord>;
    return typeof candidate.target === "string"
      && typeof candidate.backup === "string"
      && typeof candidate.hadOriginal === "boolean"
      && (candidate.stage === undefined || typeof candidate.stage === "string")
      && (candidate.expectedDigest === undefined || typeof candidate.expectedDigest === "string")
      && (candidate.expectMissing === undefined || typeof candidate.expectMissing === "boolean")
      && (candidate.installed === undefined || typeof candidate.installed === "boolean")
      && (candidate.installedDigest === undefined || typeof candidate.installedDigest === "string");
  });
}

async function exists(file: string): Promise<boolean> {
  try {
    await access(file);
    return true;
  } catch {
    return false;
  }
}

async function sha256File(file: string): Promise<string> {
  const data = await readFile(file);
  return createHash("sha256").update(data).digest("hex");
}

function sha256Text(value: string): string {
  return createHash("sha256").update(value, "utf8").digest("hex");
}

function processIsAlive(pid: number | undefined): boolean {
  if (!Number.isInteger(pid) || pid === undefined || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

async function atomicWriteJson(file: string, value: unknown): Promise<void> {
  const temporary = `${file}.tmp-${randomBytes(4).toString("hex")}`;
  await writeFile(temporary, JSON.stringify(value, null, 2), { mode: 0o600 });
  const handle = await open(temporary, "r");
  try { await handle.sync(); } finally { await handle.close(); }
  await rename(temporary, file);
}

async function fsyncDir(directory: string): Promise<void> {
  try {
    const handle = await open(directory, "r");
    try { await handle.sync(); } finally { await handle.close(); }
  } catch {
    // Some platforms/filesystems do not permit directory fsync. The rename journal still provides rollback.
  }
}

export class TransactionalWriter {
  readonly journalDir: string;

  constructor(journalDir = path.join(configDirectory(), "transactions")) {
    this.journalDir = journalDir;
  }

  async initialize(): Promise<void> {
    await mkdir(this.journalDir, { recursive: true, mode: 0o700 });
    await this.recoverPending();
  }

  async recoverPending(): Promise<void> {
    await mkdir(this.journalDir, { recursive: true, mode: 0o700 });
    const entries = await readdir(this.journalDir, { withFileTypes: true });
    for (const entry of entries) {
      if (!entry.isFile() || !entry.name.endsWith(".json")) continue;
      const journalPath = path.join(this.journalDir, entry.name);
      let journal: Journal;
      try {
        const parsed: unknown = JSON.parse(await readFile(journalPath, "utf8"));
        if (!isJournal(parsed)) continue;
        journal = parsed;
      } catch {
        continue;
      }
      // A second Yeet process may start while another process is between the
      // journal write and commit. Never recover a live transaction: doing so
      // would move its backups back over the files it is currently editing.
      // Journals from older versions have no ownerPid and are recovered using
      // the legacy fail-safe behavior.
      if (journal.state === "prepared" && processIsAlive(journal.ownerPid)) continue;
      if (journal.state === "committed") await this.#cleanupCommitted(journal, journalPath);
      else await this.#rollback(journal, journalPath);
    }
  }

  async commit(states: readonly PlannedFileState[]): Promise<void> {
    const unique = new Set<string>();
    for (const state of states) {
      if (unique.has(state.path)) throw new Error(`Transaction contains duplicate target: ${state.path}`);
      unique.add(state.path);
    }
    const id = `${Date.now()}-${process.pid}-${randomBytes(5).toString("hex")}`;
    const records: JournalRecord[] = [];

    for (const state of states) {
      const directory = path.dirname(state.path);
      await mkdir(directory, { recursive: true });
      const basename = path.basename(state.path);
      const hadOriginal = await exists(state.path);
      const backup = path.join(directory, `.${basename}.yeet-backup-${id}`);
      let stage: string | undefined;
      if (state.content !== undefined) {
        stage = path.join(directory, `.${basename}.yeet-stage-${id}`);
        const handle = await open(stage, "wx", state.mode ?? 0o644);
        try {
          await handle.writeFile(state.content, "utf8");
          await handle.sync();
        } finally {
          await handle.close();
        }
        if (state.mode !== undefined) await chmod(stage, state.mode);
      }
      records.push({
        target: state.path,
        ...(stage ? { stage } : {}),
        backup,
        hadOriginal,
        ...(state.expectedDigest ? { expectedDigest: state.expectedDigest } : {}),
        ...(state.expectMissing !== undefined ? { expectMissing: state.expectMissing } : {}),
        ...(state.content !== undefined ? { installedDigest: sha256Text(state.content) } : {}),
      });
    }

    const journal: Journal = { id, state: "prepared", records, ownerPid: process.pid };
    const journalPath = path.join(this.journalDir, `${id}.json`);
    await atomicWriteJson(journalPath, journal);
    await fsyncDir(this.journalDir);

    let committed = false;
    try {
      for (const record of records) {
        const existsNow = await exists(record.target);
        if (record.expectMissing && existsNow) throw new Error(`Target was created concurrently: ${record.target}`);
        if (record.hadOriginal !== existsNow) throw new Error(`Target existence changed concurrently: ${record.target}`);
        if (record.hadOriginal) {
          await rename(record.target, record.backup);
          if (record.expectedDigest) {
            const actual = await sha256File(record.backup);
            if (actual !== record.expectedDigest) throw new Error(`Target changed after preflight: ${record.target}`);
          }
        }
      }
      for (const record of records) {
        if (!record.stage) continue;
        if (record.expectMissing) {
          // rename() replaces an existing target. A hard-link install gives
          // create and move destinations the required no-clobber semantics.
          await link(record.stage, record.target);
          record.installed = true;
          await atomicWriteJson(journalPath, journal);
          await fsyncDir(this.journalDir);
          await rm(record.stage, { force: true });
        } else {
          await rename(record.stage, record.target);
          record.installed = true;
          await atomicWriteJson(journalPath, journal);
          await fsyncDir(this.journalDir);
        }
      }
      for (const directory of new Set(records.map(record => path.dirname(record.target)))) await fsyncDir(directory);
      journal.state = "committed";
      await atomicWriteJson(journalPath, journal);
      await fsyncDir(this.journalDir);
      committed = true;
    } catch (error) {
      await this.#rollback(journal, journalPath);
      throw error;
    }
    if (committed) {
      // The edit is already durable once the journal is marked committed.
      // Cleanup is deliberately best-effort; rolling back here would turn a
      // successful edit into data loss if removing a backup or journal fails.
      try {
        await this.#cleanupCommitted(journal, journalPath);
      } catch {
        // The committed journal is intentionally left for the next startup's
        // recovery pass to finish cleaning up.
      }
    }
  }

  async #cleanupCommitted(journal: Journal, journalPath: string): Promise<void> {
    for (const record of journal.records) {
      if (record.stage) await rm(record.stage, { force: true });
      await rm(record.backup, { force: true });
    }
    await rm(journalPath, { force: true });
  }

  async #rollback(journal: Journal, journalPath: string): Promise<void> {
    for (const record of [...journal.records].reverse()) {
      const backupExists = await exists(record.backup);
      const targetExists = await exists(record.target);
      if (backupExists) {
        if (targetExists) {
          const installed = record.installed || (record.installedDigest && await this.#matchesDigest(record.target, record.installedDigest));
          // Do not delete a target that appeared while this transaction was
          // preparing. It may belong to another writer.
          if (!installed) continue;
          await rm(record.target, { force: true });
        }
        await rename(record.backup, record.target);
      } else if (!record.hadOriginal && targetExists) {
        const installed = record.installed
          || (!record.stage && record.installedDigest && await this.#matchesDigest(record.target, record.installedDigest))
          || (record.stage && await this.#sameFile(record.stage, record.target));
        if (installed) await rm(record.target, { force: true });
      }
      if (record.stage) await rm(record.stage, { force: true });
    }
    await rm(journalPath, { force: true });
  }

  async #matchesDigest(file: string, expected: string): Promise<boolean> {
    try {
      return await sha256File(file) === expected;
    } catch {
      return false;
    }
  }

  async #sameFile(left: string, right: string): Promise<boolean> {
    try {
      const [leftStat, rightStat] = await Promise.all([stat(left), stat(right)]);
      return leftStat.dev === rightStat.dev && leftStat.ino === rightStat.ino;
    } catch {
      return false;
    }
  }
}
