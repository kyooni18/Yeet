//! Read-only Files and Changes projections for Remote clients.
//!
//! Retrieval stays in Harness resources; this module only confines every path to
//! the connection's workspace and shapes the results for the wire.
use std::{
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::harness::resources;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRow {
    pub name: String,
    /// Workspace-relative, `/`-separated.
    pub path: String,
    pub directory: bool,
    pub size: u64,
    /// Entry count for directories.
    pub items: Option<usize>,
    /// Unix seconds.
    pub modified: Option<u64>,
    /// Git porcelain XY when the entry has uncommitted changes.
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified: Option<u64>,
    pub created: Option<u64>,
    /// Unix permission string such as `rw-r--r--`.
    pub permissions: Option<String>,
    pub lines: Option<usize>,
    pub status: Option<String>,
    pub added: Option<usize>,
    pub removed: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesView {
    pub root: String,
    pub path: String,
    pub parent: Option<String>,
    pub branch: String,
    pub entries: Vec<FileRow>,
    pub selected: Option<String>,
    pub info: Option<FileInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRow {
    pub path: String,
    pub status: String,
    pub added: Option<usize>,
    pub removed: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesView {
    pub root: String,
    pub branch: String,
    pub files: Vec<ChangeRow>,
    pub selected: Option<String>,
    pub full: bool,
    pub patch: Vec<String>,
    pub message: Option<String>,
}

fn unix(time: Option<SystemTime>) -> Option<u64> {
    time?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn wire_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Resolve a workspace-relative path without allowing absolute paths, `..`, or
/// symlinks that leave the workspace.
fn confined(workspace: &Path, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        bail!("path must be relative to the workspace");
    }
    let root = workspace.canonicalize()?;
    let resolved = root.join(relative).canonicalize()?;
    if !resolved.starts_with(&root) {
        bail!("path is outside the workspace");
    }
    Ok(resolved)
}

fn permission_string(mode: Option<u32>) -> Option<String> {
    let mode = mode?;
    Some(
        (0..9)
            .rev()
            .map(|bit| {
                let set = mode & (1 << bit) != 0;
                match (bit % 3, set) {
                    (_, false) => '-',
                    (2, true) => 'r',
                    (1, true) => 'w',
                    _ => 'x',
                }
            })
            .collect(),
    )
}

pub fn files_view(workspace: &Path, path: &str, selected: Option<&str>) -> Result<FilesView> {
    let root = workspace.canonicalize()?;
    let dir = confined(workspace, path)?;
    if !dir.is_dir() {
        bail!("not a directory");
    }
    let relative = dir.strip_prefix(&root).unwrap_or(Path::new(""));
    let changes = resources::git_changes(&dir);
    let (branch, _) = resources::git_identity(&dir);
    let entries = resources::directory_entries(&dir)
        .into_iter()
        .filter(|entry| entry.name != ".git")
        .map(|entry| {
            let full = dir.join(&entry.name);
            let status = if entry.is_dir {
                let prefix = format!("{}/", entry.name);
                changes
                    .iter()
                    .any(|(changed, _)| changed.starts_with(&prefix))
                    .then(|| " M".to_owned())
            } else {
                changes.get(&entry.name).cloned()
            };
            FileRow {
                path: wire_path(&relative.join(&entry.name)),
                directory: entry.is_dir,
                size: entry.size,
                items: entry
                    .is_dir
                    .then(|| resources::directory_entry_count(&full)),
                modified: unix(entry.modified),
                status,
                name: entry.name,
            }
        })
        .collect::<Vec<_>>();

    let info = match selected {
        Some(name) => {
            let file = confined(workspace, name)?;
            if file.is_file() && file.parent() == Some(dir.as_path()) {
                let file_name = file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let meta = std::fs::metadata(&file)?;
                let extra = resources::file_metadata(&file).unwrap_or_default();
                let stats = resources::git_numstat(&dir, &file_name);
                Some(FileInfo {
                    path: wire_path(file.strip_prefix(&root).unwrap_or(&file)),
                    size: meta.len(),
                    modified: unix(meta.modified().ok()),
                    created: unix(extra.created),
                    permissions: permission_string(extra.permissions),
                    lines: resources::count_text_lines(&file),
                    status: changes.get(&file_name).cloned(),
                    added: stats.map(|(added, _)| added),
                    removed: stats.map(|(_, removed)| removed),
                    name: file_name,
                })
            } else {
                None
            }
        }
        None => None,
    };

    Ok(FilesView {
        root: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: wire_path(relative),
        parent: (!relative.as_os_str().is_empty())
            .then(|| wire_path(relative.parent().unwrap_or(Path::new("")))),
        branch,
        entries,
        selected: info.as_ref().map(|info| info.path.clone()),
        info,
    })
}

pub fn changes_view(workspace: &Path, file: Option<&str>, full: bool) -> Result<ChangesView> {
    let snapshot = resources::GitSnapshot::load(workspace);
    let files = snapshot
        .changes
        .iter()
        .map(|change| ChangeRow {
            path: wire_path(&change.path),
            status: change.status.clone(),
            added: change.stats.map(|stats| stats.added),
            removed: change.stats.map(|stats| stats.removed),
        })
        .collect::<Vec<_>>();
    let selected = file
        .and_then(|name| files.iter().find(|row| row.path == name))
        .or_else(|| files.first())
        .map(|row| row.path.clone());

    let mut patch = Vec::new();
    let mut message = snapshot.message.clone();
    if let Some(path) = &selected
        && let Some(change) = snapshot.changes.iter().find(|c| wire_path(&c.path) == *path)
    {
        let has_base = !resources::git_identity(&snapshot.root).1.is_empty();
        match resources::review_patch(
            &snapshot.root,
            &change.path,
            change.previous_path.as_deref(),
            change.status == "??",
            has_base,
            full,
        ) {
            Ok(lines) => patch = lines,
            Err(error) => message = Some(error),
        }
    }

    Ok(ChangesView {
        root: snapshot
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        branch: snapshot.branch,
        files,
        selected,
        full,
        patch,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_view_lists_the_workspace_and_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "hi\n").unwrap();

        let root = files_view(dir.path(), "", None).unwrap();
        assert_eq!(root.entries[0].name, "src");
        assert_eq!(root.entries[0].items, Some(1));
        assert!(root.parent.is_none());

        let src = files_view(dir.path(), "src", Some("src/lib.rs")).unwrap();
        assert_eq!(src.parent.as_deref(), Some(""));
        assert_eq!(src.info.unwrap().lines, Some(1));

        assert!(files_view(dir.path(), "..", None).is_err());
        assert!(files_view(dir.path(), "/etc", None).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/", dir.path().join("escape")).unwrap();
            assert!(files_view(dir.path(), "escape", None).is_err());
        }
    }

    #[test]
    fn changes_view_reports_uncommitted_files_with_their_patch() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            return;
        }
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();

        let view = changes_view(dir.path(), None, false).unwrap();
        assert_eq!(view.files.len(), 1);
        assert_eq!(view.selected.as_deref(), Some("a.txt"));
        assert_eq!(view.files[0].added, Some(1));
        assert!(view.patch.iter().any(|line| line == "+two"));
    }
}
