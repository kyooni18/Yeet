//! Directory-scoped AGENT.md / AGENTS.md / YEET.md instructions.
//!
//! Root-level files are loaded by turn setup. Instructions that live deeper in
//! the tree apply only to their subtree, so they are attached lazily: the first
//! time a tool round touches a file or shell working directory, all ancestor
//! instruction files are appended parent-first as request-only overlays. History is
//! append-only, so a byte-identical copy already present is not attached again.
//! Mutations defer until newly discovered overlays reach the next model request.

use std::path::{Path, PathBuf};

use super::*;

const INSTRUCTION_FILES: [&str; 3] = ["AGENT.md", "AGENTS.md", "YEET.md"];
const MAX_FILE_BYTES: u64 = 16 * 1024;

/// Overlays for nested instruction files of the paths touched by this round.
pub(super) fn overlays_for_round(
    history: &[Message],
    workspace_root: &str,
    session_cwd: &str,
    calls: &[ToolCall],
) -> Vec<Message> {
    let root = Path::new(workspace_root);
    let mut directories: Vec<PathBuf> = Vec::new();
    for path in calls.iter().flat_map(touched_paths) {
        let absolute = Path::new(session_cwd).join(path);
        for directory in instruction_directories(root, &absolute) {
            if !directories.contains(&directory) {
                directories.push(directory);
            }
        }
    }
    let mut overlays = Vec::new();
    for directory in directories {
        let Some(text) = render(root, &directory) else {
            continue;
        };
        let pending = overlays
            .iter()
            .any(|overlay: &Message| overlay.content.as_deref() == Some(text.as_str()));
        if !pending && let Some(overlay) = repo_context::unsent_overlay(history, text) {
            overlays.push(overlay);
        }
    }
    overlays
}

fn touched_paths(call: &ToolCall) -> Vec<String> {
    let arguments = &call.arguments;
    let path = || arguments.get("path").and_then(Value::as_str);
    let nested = |list: &str| {
        arguments
            .get(list)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("path").and_then(Value::as_str))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    match call.name.as_str() {
        "read_file" => path()
            .map(str::to_owned)
            .into_iter()
            .chain(nested("requests"))
            .collect(),
        "apply_file_edits" => crate::tools::file_edit_changes(arguments)
            .iter()
            .flat_map(|change| {
                [
                    change.get("path"),
                    change.get("fileOp").and_then(|op| op.get("destination")),
                ]
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            })
            .map(str::to_owned)
            .collect(),
        "outline" => path().map(str::to_owned).into_iter().collect(),
        "run_shell" => vec![
            Path::new(
                arguments
                    .get("workingDirectory")
                    .and_then(Value::as_str)
                    .unwrap_or("."),
            )
            .join(".yeet-instruction-scope")
            .to_string_lossy()
            .into_owned(),
        ],
        _ => Vec::new(),
    }
}

/// All instruction directories strictly below `root`, parent-first.
/// A deeper file augments ancestor rules rather than hiding them.
fn instruction_directories(root: &Path, file: &Path) -> Vec<PathBuf> {
    let file = normalize(file);
    let mut found = Vec::new();
    let Some(mut directory) = file.parent() else {
        return found;
    };
    while directory.starts_with(root) && directory != root {
        if INSTRUCTION_FILES
            .iter()
            .any(|name| directory.join(name).is_file())
        {
            found.push(directory.to_path_buf());
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        directory = parent;
    }
    found.reverse();
    found
}

/// Lexically resolves `.`/`..` so model-supplied relative paths cannot walk
/// the search out of the workspace prefix check.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

fn render(root: &Path, directory: &Path) -> Option<String> {
    let relative = directory
        .strip_prefix(root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    let mut sections = Vec::new();
    for name in INSTRUCTION_FILES {
        let path = directory.join(name);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(&path)
            && !contents.trim().is_empty()
        {
            sections.push(format!("--- {relative}/{name} ---\n{}", contents.trim()));
        }
    }
    (!sections.is_empty()).then(|| {
        format!(
            "Directory instructions for files under {relative}/ (more specific than root project instructions; apply to that subtree):\n{}",
            sections.join("\n\n")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestor_nested_instructions_attach_once_parent_first() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("crates/core/src")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "root rules").unwrap();
        std::fs::write(root.join("crates/AGENTS.md"), "crates rules").unwrap();
        std::fs::write(root.join("crates/core/AGENT.md"), "core rules").unwrap();
        let root_text = root.to_string_lossy().into_owned();
        let read = |path: &str| ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: json!({"path": path}),
        };

        let calls = [
            read("crates/core/src/lib.rs"),
            read("crates/core/src/other.rs"),
        ];
        let mut history = Vec::new();
        let overlays = overlays_for_round(&history, &root_text, &root_text, &calls);
        assert_eq!(overlays.len(), 2);
        assert!(
            overlays[0]
                .content
                .as_deref()
                .unwrap()
                .contains("crates rules")
        );
        assert!(
            overlays[1]
                .content
                .as_deref()
                .unwrap()
                .contains("core rules")
        );
        history.extend(overlays);
        assert!(overlays_for_round(&history, &root_text, &root_text, &calls).is_empty());
        let move_call = ToolCall {
            id: "move".into(),
            name: "apply_file_edits".into(),
            arguments: json!({"patch": "*** Begin Patch\n*** Update File: root.txt\n*** Move to: crates/core/moved.txt\n*** End Patch"}),
        };
        assert_eq!(
            overlays_for_round(&[], &root_text, &root_text, &[move_call]).len(),
            2
        );
        // Root-level instructions are turn setup's job; escaping paths find nothing.
        assert!(overlays_for_round(&[], &root_text, &root_text, &[read("main.rs")]).is_empty());
        assert!(
            overlays_for_round(&[], &root_text, &root_text, &[read("../crates/core/x.rs")])
                .is_empty()
        );
    }
}
