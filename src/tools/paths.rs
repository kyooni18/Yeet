//! Filesystem-boundary checks and workspace revision fingerprints.
//!
//! Path normalization is centralized here so file and shell tools share the
//! same escape checks. Revision fingerprints include uncommitted worktree data
//! used to invalidate retained project evidence after local edits.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    time::UNIX_EPOCH,
};

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

/// Returns whether a requested path resolves outside the active workspace.
pub(super) fn path_outside_workspace(root: &Path, input: &str) -> Result<bool> {
    if input.trim().is_empty() || input.contains('\0') {
        bail!("invalid file path");
    }
    let input = Path::new(input);
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let resolved = canonicalize_existing_ancestor(&joined)?;
    Ok(!(resolved == root || resolved.starts_with(root)))
}

/// Computes the committed revision plus a bounded fingerprint of local worktree changes.
pub(crate) fn workspace_revision_for_path(root: &Path) -> Option<String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let head_output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !head_output.status.success() {
        return None;
    }
    let head = String::from_utf8(head_output.stdout)
        .ok()?
        .trim()
        .to_owned();
    if head.is_empty() {
        return None;
    }

    let diff = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["diff", "--binary", "HEAD", "--", "."])
        .output()
        .ok()?;
    let status = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
        .ok()?;
    if !diff.status.success() || !status.status.success() {
        return Some(head);
    }
    if diff.stdout.is_empty() && status.stdout.is_empty() {
        return Some(head);
    }

    let mut digest = Sha256::new();
    digest.update(head.as_bytes());
    digest.update(b"\0tracked-diff\0");
    digest.update(&diff.stdout);
    digest.update(b"\0status\0");
    digest.update(&status.stdout);
    for entry in status.stdout.split(|byte| *byte == 0) {
        if !entry.starts_with(b"?? ") {
            continue;
        }
        let relative = String::from_utf8_lossy(&entry[3..]);
        let path = root.join(relative.as_ref());
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        digest.update(b"\0untracked\0");
        digest.update(relative.as_bytes());
        digest.update(metadata.len().to_le_bytes());
        if let Ok(modified) = metadata.modified()
            && let Ok(delta) = modified.duration_since(UNIX_EPOCH)
        {
            digest.update(delta.as_nanos().to_le_bytes());
        }
        if metadata.is_file()
            && metadata.len() <= 1024 * 1024
            && let Ok(bytes) = fs::read(&path)
        {
            digest.update(&bytes);
        }
    }
    Some(format!("{head}+worktree:{:x}", digest.finalize()))
}

/// Canonicalizes the existing prefix of a path while preserving a non-existing tail.
pub(super) fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let lexical = normalize_absolute_path(path)?;
    let mut ancestor = lexical.clone();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name().map(|value| value.to_os_string()) else {
            break;
        };
        tail.push(name);
        if !ancestor.pop() {
            break;
        }
    }
    let mut resolved = ancestor.canonicalize().unwrap_or(ancestor);
    for component in tail.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

/// Lexically normalizes an absolute path without requiring its target to exist.
fn normalize_absolute_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("path normalization requires an absolute path");
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    Ok(normalized)
}
