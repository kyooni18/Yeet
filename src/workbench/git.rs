//! Git resources with NUL-delimited paths and combined index/worktree changes.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangeStats {
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone)]
pub struct GitChange {
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,
    /// Git porcelain XY: index status followed by working-tree status.
    pub status: String,
    /// None means binary/unavailable, not zero changes.
    pub stats: Option<ChangeStats>,
}

#[derive(Debug, Clone, Default)]
pub struct GitSnapshot {
    pub workspace: PathBuf,
    pub root: PathBuf,
    pub branch: String,
    pub changes: Vec<GitChange>,
    pub message: Option<String>,
}

impl GitSnapshot {
    pub fn load(workspace: &Path) -> Self {
        let mut snapshot = Self {
            workspace: workspace.into(),
            root: workspace.into(),
            ..Self::default()
        };
        let result = (|| -> Result<(), String> {
            snapshot.root = path_from_bytes(trim_newline(&git(
                workspace,
                &["rev-parse", "--show-toplevel"],
            )?));
            snapshot.branch = git(&snapshot.root, &["symbolic-ref", "--short", "HEAD"])
                .or_else(|_| git(&snapshot.root, &["rev-parse", "--short", "HEAD"]))
                .map(|bytes| String::from_utf8_lossy(trim_newline(&bytes)).into_owned())
                .unwrap_or_default();
            let status = git(
                &snapshot.root,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            )?;
            let base = if git(&snapshot.root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
                "HEAD".to_owned()
            } else {
                String::from_utf8_lossy(&git(
                    &snapshot.root,
                    &["hash-object", "-t", "tree", "--stdin"],
                )?)
                .trim()
                .to_owned()
            };
            let stats = parse_numstat(&git(
                &snapshot.root,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--numstat",
                    "-z",
                    &base,
                ],
            )?);
            let mut records = status
                .split(|byte| *byte == 0)
                .filter(|entry| !entry.is_empty());
            while let Some(entry) = records.next() {
                if entry.len() < 4 {
                    continue;
                }
                let path = path_from_bytes(&entry[3..]);
                let renamed = entry[..2].contains(&b'R') || entry[..2].contains(&b'C');
                let previous_path = if renamed {
                    records.next().map(path_from_bytes)
                } else {
                    None
                };
                let mut counts = if &entry[..2] == b"??" {
                    count_untracked(&snapshot.root.join(&path))
                } else {
                    stats
                        .get(&path)
                        .copied()
                        .unwrap_or(Some(ChangeStats::default()))
                };
                // Porcelain can retain an index rename whose combined patch
                // has too little similarity for Git's rename detection. Include
                // the old-path deletion in that case instead of losing it.
                if let Some(old_counts) = previous_path.as_ref().and_then(|path| stats.get(path)) {
                    counts = counts.zip(*old_counts).map(|(new, old)| ChangeStats {
                        added: new.added + old.added,
                        removed: new.removed + old.removed,
                    });
                }
                snapshot.changes.push(GitChange {
                    path,
                    previous_path,
                    status: String::from_utf8_lossy(&entry[..2]).into_owned(),
                    stats: counts,
                });
            }
            snapshot.changes.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(())
        })();
        if let Err(error) = result {
            snapshot.message = Some(error);
        }
        snapshot
    }

    /// Read a real patch, including deleted, untracked and unborn-repository files.
    pub fn patch(path: &Path) -> Result<Vec<String>, String> {
        let dir = path
            .ancestors()
            .skip(1)
            .find(|dir| dir.is_dir())
            .ok_or("File directory unavailable")?;
        let root = path_from_bytes(trim_newline(&git(dir, &["rev-parse", "--show-toplevel"])?));
        let absolute = dir.canonicalize().map_err(|e| e.to_string())?.join(
            path.strip_prefix(dir)
                .map_err(|_| "File directory unavailable")?,
        );
        let relative = absolute
            .strip_prefix(&root)
            .map_err(|_| "File is outside the repository")?;
        let tracked = command(&root)
            .args(["ls-files", "--error-unmatch", "--"])
            .arg(relative)
            .output()
            .map_err(|e| e.to_string())?
            .status
            .success();
        let untracked = !tracked && path.exists();
        let mut cmd = command(&root);
        cmd.args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--unified=3",
        ]);
        if untracked {
            cmd.args(["--no-index", "--", "/dev/null"]).arg(relative);
        } else {
            let base = if git(&root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
                "HEAD".to_owned()
            } else {
                String::from_utf8_lossy(&git(&root, &["hash-object", "-t", "tree", "--stdin"])?)
                    .trim()
                    .to_owned()
            };
            cmd.arg(base).arg("--").arg(relative);
        }
        let output = cmd.output().map_err(|e| e.to_string())?;
        if output.status.success() || (untracked && output.status.code() == Some(1)) {
            Ok(String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::to_owned)
                .collect())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().into())
        }
    }
}

fn command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("--literal-pathspecs")
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null());
    cmd
}

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = command(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().into())
    }
}

fn parse_numstat(bytes: &[u8]) -> HashMap<PathBuf, Option<ChangeStats>> {
    let mut counts = HashMap::new();
    let mut records = bytes.split(|b| *b == 0);
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, |b| *b == b'\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let stats = String::from_utf8_lossy(added)
            .parse()
            .ok()
            .zip(String::from_utf8_lossy(removed).parse().ok())
            .map(|(added, removed)| ChangeStats { added, removed });
        let path = if path.is_empty() {
            records.next();
            records.next().unwrap_or_default()
        } else {
            path
        };
        counts.insert(path_from_bytes(path), stats);
    }
    counts
}

fn count_untracked(path: &Path) -> Option<ChangeStats> {
    if std::fs::metadata(path).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    Some(ChangeStats {
        added: bytes.iter().filter(|b| **b == b'\n').count()
            + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n")),
        removed: 0,
    })
}

fn trim_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}
#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}
#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).as_ref())
}
