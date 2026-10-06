//! Workspace grounding and subject discovery for debate runs.
//!
//! Debate framing uses a bounded local project brief and only marks a topic as
//! workspace-bound when the proposition explicitly refers to this codebase.

use serde_json::Value;

pub(super) const DEBATE_PROJECT_BRIEF_CHARS: usize = 2_800;
const DEBATE_README_BRIEF_CHARS: usize = 1_100;

/// Builds a bounded project brief from manifests, structure, and README prose.
pub(super) fn debate_project_brief(workspace: &std::path::Path) -> String {
    let root = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let project_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("unknown");

    let mut lines = vec![format!("Project name: {project_name}")];
    let mut metadata = Vec::new();
    if let Some(value) = cargo_project_metadata(&root.join("Cargo.toml")) {
        metadata.push(value);
    }
    if let Some(value) = package_json_project_metadata(&root.join("package.json")) {
        metadata.push(value);
    }
    if root.join("Package.swift").is_file() {
        metadata.push("Swift package manifest: Package.swift".into());
    }
    if root.join("pyproject.toml").is_file() {
        metadata.push("Python project manifest: pyproject.toml".into());
    }
    if root.join("CMakeLists.txt").is_file() {
        metadata.push("C/C++ build manifest: CMakeLists.txt".into());
    }
    if !metadata.is_empty() {
        lines.push(format!("Project metadata: {}", metadata.join(" | ")));
    }

    if let Ok(entries) = std::fs::read_dir(&root) {
        let mut names = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.')
                    || matches!(name.as_str(), "target" | "node_modules" | "dist" | "build")
                {
                    return None;
                }
                let suffix = entry
                    .file_type()
                    .ok()
                    .filter(|kind| kind.is_dir())
                    .map(|_| "/");
                Some(format!("{name}{}", suffix.unwrap_or("")))
            })
            .collect::<Vec<_>>();
        names.sort_by_key(|name| name.to_ascii_lowercase());
        names.truncate(18);
        if !names.is_empty() {
            lines.push(format!("Top-level structure: {}", names.join(", ")));
        }
    }

    if let Some(readme) = read_text_prefix(&root.join("README.md"), 24_000) {
        let summary = readme_brief(&readme);
        if !summary.is_empty() {
            lines.push(format!("README overview: {summary}"));
        }
    }
    truncate_chars(&lines.join("\n"), DEBATE_PROJECT_BRIEF_CHARS)
}

/// Extracts human-readable package metadata from Cargo.toml.
fn cargo_project_metadata(path: &std::path::Path) -> Option<String> {
    let text = read_text_prefix(path, 16_000)?;
    let mut in_package = false;
    let mut name = None;
    let mut description = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if name.is_none() {
            name = quoted_assignment(line, "name");
        }
        if description.is_none() {
            description = quoted_assignment(line, "description");
        }
    }
    match (name, description) {
        (Some(name), Some(description)) => Some(format!("Rust crate {name}: {description}")),
        (Some(name), None) => Some(format!("Rust crate: {name}")),
        _ => Some("Rust project manifest: Cargo.toml".into()),
    }
}

/// Extracts human-readable package metadata from package.json.
fn package_json_project_metadata(path: &std::path::Path) -> Option<String> {
    let text = read_text_prefix(path, 16_000)?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    let name = value.get("name").and_then(Value::as_str).map(str::trim);
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (name, description) {
        (Some(name), Some(description)) if !name.is_empty() => {
            Some(format!("Node package {name}: {description}"))
        }
        (Some(name), _) if !name.is_empty() => Some(format!("Node package: {name}")),
        _ => Some("Node project manifest: package.json".into()),
    }
}

/// Reads at most `max_bytes` from a text file for lightweight grounding.
fn read_text_prefix(path: &std::path::Path, max_bytes: usize) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let end = bytes.len().min(max_bytes);
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

/// Parses a simple quoted TOML assignment.
fn quoted_assignment(line: &str, key: &str) -> Option<String> {
    let rest = line
        .strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?
        .trim();
    let value = rest.strip_prefix('"')?.split('"').next()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Converts README prose into a bounded one-line project summary.
fn readme_brief(readme: &str) -> String {
    let mut pieces = Vec::new();
    let mut in_code = false;
    for raw in readme.lines() {
        let line = raw.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || line.is_empty() || line.starts_with('#') || line.starts_with("![") {
            continue;
        }
        pieces.push(line.trim_start_matches(['-', '*']).trim().to_owned());
        if pieces.join(" ").chars().count() >= DEBATE_README_BRIEF_CHARS {
            break;
        }
    }
    truncate_chars(&pieces.join(" "), DEBATE_README_BRIEF_CHARS)
}

/// Truncates Unicode text by characters rather than bytes.
fn truncate_chars(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    let mut compact = value
        .chars()
        .take(maximum.saturating_sub(1))
        .collect::<String>();
    compact.push('…');
    compact
}

/// Determines whether a debate proposition refers to the current workspace.
pub(super) fn discover_debate_subject(
    topic: &str,
    workspace: &std::path::Path,
) -> crate::debate::DebateSubject {
    let lower = topic.to_lowercase();
    let root = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let explicit_workspace_reference = [
        "current implementation",
        "existing implementation",
        "this implementation",
        "current code",
        "existing code",
        "this code",
        "current controller",
        "existing controller",
        "this controller",
        "current algorithm",
        "existing algorithm",
        "this algorithm",
        "current project",
        "existing project",
        "this project",
        "this workspace",
        "current workspace",
        "this repository",
        "current repository",
        "this codebase",
        "current codebase",
        "현재 구현",
        "기존 구현",
        "지금 구현",
        "이 구현",
        "현 구현",
        "현재 코드",
        "기존 코드",
        "이 코드",
        "현재 컨트롤러",
        "기존 컨트롤러",
        "이 컨트롤러",
        "현재 알고리즘",
        "기존 알고리즘",
        "이 알고리즘",
        "현재 프로젝트",
        "기존 프로젝트",
        "이 프로젝트",
        "현재 작업공간",
        "이 작업공간",
        "현재 코드베이스",
        "이 코드베이스",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let english_contextual_reference = ["current ", "existing ", "this ", "as implemented"]
        .iter()
        .any(|marker| lower.contains(marker))
        && [
            "logic",
            "design",
            "architecture",
            "control loop",
            "controller",
            "trajectory",
            "deorbit",
            "landing",
            "implementation",
            "algorithm",
            "code",
            "codebase",
            "workspace",
            "repository",
        ]
        .iter()
        .any(|noun| lower.contains(noun));
    let korean_contextual_reference = ["현재", "지금", "기존", "현행", "이 "]
        .iter()
        .any(|marker| lower.contains(marker))
        && [
            "구현",
            "코드",
            "로직",
            "설계",
            "아키텍처",
            "제어",
            "컨트롤러",
            "알고리즘",
            "궤적",
            "감속",
            "착륙",
            "프로젝트",
            "작업공간",
            "코드베이스",
        ]
        .iter()
        .any(|noun| lower.contains(noun));
    let named_workspace_reference = root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| name.chars().count() >= 3)
        .is_some_and(|name| topic_mentions_identifier(&lower, &name.to_lowercase()));
    let workspace_bound = explicit_workspace_reference
        || english_contextual_reference
        || korean_contextual_reference
        || named_workspace_reference;

    if !workspace_bound {
        return crate::debate::DebateSubject::default();
    }

    let anchors = [
        "Cargo.toml",
        "Package.swift",
        "package.json",
        "pyproject.toml",
        "CMakeLists.txt",
        "Makefile",
        "README.md",
        "src",
        "Sources",
        "include",
        "tests",
        "Tests",
    ]
    .into_iter()
    .filter(|name| root.join(name).exists())
    .map(str::to_owned)
    .collect();

    crate::debate::DebateSubject {
        workspace_bound: true,
        workspace_root: root.display().to_string(),
        revision: crate::tools::workspace_revision_for_path(&root),
        anchor_files: anchors,
    }
}

/// Matches a project identifier on token boundaries to avoid substring false positives.
pub(super) fn topic_mentions_identifier(topic_lower: &str, identifier_lower: &str) -> bool {
    topic_lower
        .match_indices(identifier_lower)
        .any(|(start, matched)| {
            let end = start + matched.len();
            let before_is_word = topic_lower[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
            let after_is_word = topic_lower[end..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);
            !before_is_word && !after_is_word
        })
}
