import path from "node:path";
import os from "node:os";
import { createHash, randomBytes } from "node:crypto";
import { access, chmod, mkdir, open, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
async function exists(file) {
    try {
        await access(file);
        return true;
    }
    catch {
        return false;
    }
}
async function sha256File(file) {
    const data = await readFile(file);
    return createHash("sha256").update(data).digest("hex");
}
async function atomicWriteJson(file, value) {
    const temporary = `${file}.tmp-${randomBytes(4).toString("hex")}`;
    await writeFile(temporary, JSON.stringify(value, null, 2), { mode: 0o600 });
    const handle = await open(temporary, "r");
    try {
        await handle.sync();
    }
    finally {
        await handle.close();
    }
    await rename(temporary, file);
}
async function fsyncDir(directory) {
    try {
        const handle = await open(directory, "r");
        try {
            await handle.sync();
        }
        finally {
            await handle.close();
        }
    }
    catch {
        // Some platforms/filesystems do not permit directory fsync. The rename journal still provides rollback.
    }
}
export class TransactionalWriter {
    journalDir;
    constructor(journalDir = path.join(os.homedir(), ".yeet", "transactions")) {
        this.journalDir = journalDir;
    }
    async initialize() {
        await mkdir(this.journalDir, { recursive: true, mode: 0o700 });
        await this.recoverPending();
    }
    async recoverPending() {
        await mkdir(this.journalDir, { recursive: true, mode: 0o700 });
        const entries = await readdir(this.journalDir, { withFileTypes: true });
        for (const entry of entries) {
            if (!entry.isFile() || !entry.name.endsWith(".json"))
                continue;
            const journalPath = path.join(this.journalDir, entry.name);
            let journal;
            try {
                journal = JSON.parse(await readFile(journalPath, "utf8"));
            }
            catch {
                continue;
            }
            if (journal.state === "committed")
                await this.#cleanupCommitted(journal, journalPath);
            else
                await this.#rollback(journal, journalPath);
        }
    }
    async commit(states) {
        const unique = new Set();
        for (const state of states) {
            if (unique.has(state.path))
                throw new Error(`Transaction contains duplicate target: ${state.path}`);
            unique.add(state.path);
        }
        const id = `${Date.now()}-${process.pid}-${randomBytes(5).toString("hex")}`;
        const records = [];
        for (const state of states) {
            const directory = path.dirname(state.path);
            await mkdir(directory, { recursive: true });
            const basename = path.basename(state.path);
            const hadOriginal = await exists(state.path);
            const backup = path.join(directory, `.${basename}.yeet-backup-${id}`);
            let stage;
            if (state.content !== undefined) {
                stage = path.join(directory, `.${basename}.yeet-stage-${id}`);
                const handle = await open(stage, "wx", state.mode ?? 0o644);
                try {
                    await handle.writeFile(state.content, "utf8");
                    await handle.sync();
                }
                finally {
                    await handle.close();
                }
                if (state.mode !== undefined)
                    await chmod(stage, state.mode);
            }
            records.push({
                target: state.path,
                ...(stage ? { stage } : {}),
                backup,
                hadOriginal,
                ...(state.expectedDigest ? { expectedDigest: state.expectedDigest } : {}),
                ...(state.expectMissing !== undefined ? { expectMissing: state.expectMissing } : {}),
            });
        }
        const journal = { id, state: "prepared", records };
        const journalPath = path.join(this.journalDir, `${id}.json`);
        await atomicWriteJson(journalPath, journal);
        await fsyncDir(this.journalDir);
        try {
            for (const record of records) {
                const existsNow = await exists(record.target);
                if (record.expectMissing && existsNow)
                    throw new Error(`Target was created concurrently: ${record.target}`);
                if (record.hadOriginal !== existsNow)
                    throw new Error(`Target existence changed concurrently: ${record.target}`);
                if (record.hadOriginal) {
                    await rename(record.target, record.backup);
                    if (record.expectedDigest) {
                        const actual = await sha256File(record.backup);
                        if (actual !== record.expectedDigest)
                            throw new Error(`Target changed after preflight: ${record.target}`);
                    }
                }
            }
            for (const record of records) {
                if (record.stage)
                    await rename(record.stage, record.target);
            }
            for (const directory of new Set(records.map(record => path.dirname(record.target))))
                await fsyncDir(directory);
            journal.state = "committed";
            await atomicWriteJson(journalPath, journal);
            await fsyncDir(this.journalDir);
            await this.#cleanupCommitted(journal, journalPath);
        }
        catch (error) {
            await this.#rollback(journal, journalPath);
            throw error;
        }
    }
    async #cleanupCommitted(journal, journalPath) {
        for (const record of journal.records) {
            if (record.stage)
                await rm(record.stage, { force: true });
            await rm(record.backup, { force: true });
        }
        await rm(journalPath, { force: true });
    }
    async #rollback(journal, journalPath) {
        for (const record of [...journal.records].reverse()) {
            const backupExists = await exists(record.backup);
            const targetExists = await exists(record.target);
            if (backupExists) {
                if (targetExists)
                    await rm(record.target, { force: true });
                await rename(record.backup, record.target);
            }
            else if (!record.hadOriginal && targetExists) {
                await rm(record.target, { force: true });
            }
            if (record.stage)
                await rm(record.stage, { force: true });
        }
        await rm(journalPath, { force: true });
    }
}
//# sourceMappingURL=transaction.js.map