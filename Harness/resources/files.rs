//! Filesystem metadata and lightweight file inspection, independent of presentation.
use std::{collections::HashMap, path::Path, time::SystemTime};

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Default)]
pub struct FileMetadata {
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// Unix permission bits when the host exposes them.
    pub permissions: Option<u32>,
}

/// Bounded text content for resource inspectors. No decoding of binary files.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilePreview {
    pub lines: Vec<String>,
    pub truncated: bool,
    pub message: Option<String>,
}

pub fn file_preview(path: &Path) -> FilePreview {
    use std::io::Read;
    const BYTES: usize = 256 * 1024;
    const LINES: usize = 400;
    let unavailable = |message: &str| FilePreview {
        lines: Vec::new(),
        truncated: false,
        message: Some(message.into()),
    };
    let Ok(file) = std::fs::File::open(path) else {
        return unavailable("File content could not be read");
    };
    let mut bytes = Vec::new();
    if file
        .take((BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return unavailable("File content could not be read");
    }
    let cut = bytes.len() > BYTES;
    bytes.truncate(BYTES);
    if bytes.contains(&0) {
        return unavailable("Binary file · text preview unavailable");
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if cut && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => return unavailable("Non-UTF-8 file · text preview unavailable"),
    };
    let mut lines = text.lines();
    let preview = lines.by_ref().take(LINES).map(str::to_owned).collect();
    FilePreview {
        lines: preview,
        truncated: cut || lines.next().is_some(),
        message: None,
    }
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

pub fn directory_entry_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, Iterator::count)
}

pub fn file_metadata(path: &Path) -> Option<FileMetadata> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode())
    };
    #[cfg(not(unix))]
    let permissions = None;

    Some(FileMetadata {
        created: metadata.created().ok(),
        accessed: metadata.accessed().ok(),
        permissions,
    })
}

pub fn path_is_file(path: &Path) -> bool {
    path.is_file()
}

pub fn path_is_dir(path: &Path) -> bool {
    path.is_dir()
}

/// Canonicalize an existing resource, or its parent when the final path is
/// missing, while retaining the caller's original path if neither is available.
pub fn canonical_resource_path(path: &Path) -> std::path::PathBuf {
    path.canonicalize()
        .or_else(|_| {
            path.parent()
                .unwrap_or(Path::new("."))
                .canonicalize()
                .map(|parent| parent.join(path.file_name().unwrap_or_default()))
        })
        .unwrap_or_else(|_| path.to_path_buf())
}

pub fn git_identity(dir: &Path) -> (String, String) {
    let value = |args: &[&str]| git(dir, args).unwrap_or_default().trim().to_owned();
    (
        value(&["branch", "--show-current"]),
        value(&["rev-parse", "--short", "HEAD"]),
    )
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = crate::platform::hidden_command("git")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_file_metadata_and_path_helpers_cover_existing_resources() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "one\n").unwrap();

        assert_eq!(directory_entry_count(dir.path()), 1);
        assert_eq!(file_preview(&file).lines, ["one"]);
        std::fs::write(&file, "line\n".repeat(401)).unwrap();
        let preview = file_preview(&file);
        assert_eq!(preview.lines.len(), 400);
        assert!(preview.truncated);
        std::fs::write(&file, [0, 1, 2]).unwrap();
        assert!(file_preview(&file).message.unwrap().contains("Binary"));
        assert!(path_is_dir(dir.path()));
        assert!(path_is_file(&file));
        assert!(file_metadata(&file).is_some());
        assert_eq!(canonical_resource_path(&file), file.canonicalize().unwrap());
    }
}
