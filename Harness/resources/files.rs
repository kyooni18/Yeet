//! Filesystem metadata and lightweight file inspection, independent of presentation.
use std::{collections::HashMap, path::Path, process::Command, time::SystemTime};

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// Read a directory in the stable directory-first order used by resource browsing.
pub fn directory_entries(dir: &Path) -> Vec<FileEntry> {
    let mut entries: Vec<FileEntry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some(FileEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: meta.is_dir(),
                size: meta.len(),
                modified: meta.modified().ok(),
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

pub fn git_identity(dir: &Path) -> (String, String) {
    let value = |args: &[&str]| git(dir, args).unwrap_or_default().trim().to_owned();
    (
        value(&["branch", "--show-current"]),
        value(&["rev-parse", "--short", "HEAD"]),
    )
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Paths relative to `dir` mapped to their two-character porcelain status.
pub fn git_changes(dir: &Path) -> HashMap<String, String> {
    let Some(prefix) = git(dir, &["rev-parse", "--show-prefix"]) else {
        return HashMap::new();
    };
    let prefix = prefix.trim();
    let Some(status) = git(dir, &["status", "--porcelain", "--untracked-files=all"]) else {
        return HashMap::new();
    };
    status
        .lines()
        .filter(|line| line.len() > 3)
        .filter_map(|line| {
            let path = line[3..].rsplit(" -> ").next()?.trim_matches('"');
            let relative = path.strip_prefix(prefix)?;
            Some((relative.to_owned(), line[..2].to_owned()))
        })
        .collect()
}

pub fn git_diff(dir: &Path, name: &str) -> Vec<String> {
    git(dir, &["diff", "HEAD", "--no-color", "--", name])
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

pub fn git_numstat(dir: &Path, name: &str) -> Option<(usize, usize)> {
    let output = git(dir, &["diff", "HEAD", "--numstat", "--", name])?;
    let mut parts = output.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

pub fn count_text_lines(path: &Path) -> Option<usize> {
    if std::fs::metadata(path).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    std::str::from_utf8(&bytes).ok()?;
    Some(
        bytes.iter().filter(|byte| **byte == b'\n').count()
            + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n")),
    )
}
