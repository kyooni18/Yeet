//! Compact, ranked repository outline for the model's turn context.
//!
//! The map lists the most-referenced source files with their public
//! declaration signatures only. It is deliberately byte-stable: no line
//! numbers, integer (log-bucketed) ranks, path-ordered output, and per-file
//! parse caching keyed on size and mtime. Editing a function body therefore
//! leaves the rendered map unchanged, so the turn overlay can be skipped
//! instead of re-sent.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

use sha2::{Digest, Sha256};

use super::symbols::{self, Symbol};

/// Default overlay size. Matches the coordinator's bytes/3 token estimate.
pub const DEFAULT_TOKEN_BUDGET: usize = 1_500;
const MAX_FILES_SCANNED: usize = 4_000;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_LINES_PER_FILE: usize = 8;
const COMPACT_NAMES: usize = 4;
const HEADER_RESERVE_BYTES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMap {
    pub text: String,
    /// Hex SHA-256 of `text`; equal digests mean a byte-identical overlay.
    pub digest: String,
    pub source_files: usize,
    pub shown_files: usize,
}

/// A file's declarations and the identifier-like words it mentions.
type FileSurface = (Arc<Vec<Symbol>>, Arc<HashSet<String>>);

struct ParsedFile {
    stamp: (u64, Option<SystemTime>),
    symbols: Arc<Vec<Symbol>>,
    identifiers: Arc<HashSet<String>>,
}

fn parse_cache() -> &'static Mutex<HashMap<PathBuf, ParsedFile>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, ParsedFile>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Builds the map for `root`, or `None` when it has no supported source files.
pub fn build(root: &Path, token_budget: usize) -> Option<RepoMap> {
    let files = source_files(root);
    if files.is_empty() {
        return None;
    }
    let parsed: Vec<(String, FileSurface)> = files
        .into_iter()
        .filter_map(|relative| {
            let surface = parse_cached(&root.join(&relative))?;
            Some((relative, surface))
        })
        .collect();

    // How many files mention each identifier; a declaration's rank is how many
    // *other* files use its name, bucketed so small edits rarely reorder files.
    let mut file_mentions: HashMap<&str, usize> = HashMap::new();
    for (_, (_, identifiers)) in &parsed {
        for identifier in identifiers.iter() {
            *file_mentions.entry(identifier.as_str()).or_default() += 1;
        }
    }
    // Log-bucketed so ordinary edits rarely change which lines are selected.
    // Member names (new, len, label) are far less unique than item names.
    let weight = |symbol: &Symbol| -> u32 {
        if symbol.name.len() < 3 {
            return 0;
        }
        let others = file_mentions
            .get(symbol.name.as_str())
            .copied()
            .unwrap_or(1)
            .saturating_sub(1);
        let bucket = usize::BITS - others.leading_zeros();
        if symbol.depth > 0 { bucket / 2 } else { bucket }
    };
    let mut ranked: Vec<(u32, &str, String, String)> = parsed
        .iter()
        .filter_map(|(path, (symbols, _))| {
            let surface = public_surface(symbols);
            let (score, block, line) = select_lines(path, &surface, &weight)?;
            let score = if is_test_path(path) { score / 4 } else { score };
            Some((score, path.as_str(), block, line))
        })
        .collect();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(right.1)));

    // Half the budget shows signatures for the core files; the rest buys
    // breadth as one names-only line per further file.
    let budget_bytes = token_budget.saturating_mul(3);
    let mut detailed: Vec<(&str, String)> = Vec::new();
    let mut compact: Vec<(&str, String)> = Vec::new();
    let mut used = HEADER_RESERVE_BYTES;
    for (_, path, block, line) in ranked {
        if used + block.len() <= budget_bytes / 2 {
            used += block.len();
            detailed.push((path, block));
        } else if used + line.len() <= budget_bytes {
            used += line.len();
            compact.push((path, line));
        }
    }
    if detailed.is_empty() {
        return None;
    }
    detailed.sort_by(|left, right| left.0.cmp(right.0));
    compact.sort_by(|left, right| left.0.cmp(right.0));
    let shown_files = detailed.len() + compact.len();
    let mut text = format!(
        "Repository map: {shown_files} of {} source files, ranked by cross-file references. Public signatures only (no bodies or line numbers); read files for detail.\n",
        parsed.len()
    );
    for (_, block) in &detailed {
        text.push_str(block);
    }
    if !compact.is_empty() {
        text.push_str("More files (most-referenced names):\n");
        for (_, line) in &compact {
            text.push_str(line);
        }
    }
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    Some(RepoMap {
        text,
        digest,
        source_files: parsed.len(),
        shown_files,
    })
}

/// Public items, plus public members of kept parents. An impl header is kept
/// only when at least one of its members is public.
fn public_surface(symbols: &[Symbol]) -> Vec<&Symbol> {
    let mut shown: Vec<&Symbol> = Vec::new();
    let mut parent_kept = false;
    let mut seen: HashSet<(u8, &str)> = HashSet::new();
    for symbol in symbols {
        // cfg-gated platform variants repeat the same signature.
        if symbol.kind != "impl" && !seen.insert((symbol.depth, symbol.signature.as_str())) {
            continue;
        }
        if symbol.kind == "module" {
            continue;
        }
        if symbol.depth == 0 {
            if shown.last().is_some_and(|last| last.kind == "impl") {
                shown.pop();
            }
            parent_kept = symbol.public || symbol.kind == "impl";
            if parent_kept {
                shown.push(symbol);
            }
        } else if parent_kept && symbol.public {
            shown.push(symbol);
        }
    }
    if shown.last().is_some_and(|last| last.kind == "impl") {
        shown.pop();
    }
    shown
}

/// Picks the file's most-referenced public lines (impl headers ride along with
/// their chosen members), rendered in source order.
fn select_lines(
    path: &str,
    surface: &[&Symbol],
    weight: &impl Fn(&Symbol) -> u32,
) -> Option<(u32, String, String)> {
    let mut candidates: Vec<usize> = (0..surface.len())
        .filter(|index| surface[*index].kind != "impl")
        .collect();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by_key(|index| (std::cmp::Reverse(weight(surface[*index])), *index));
    let omitted = candidates.len().saturating_sub(MAX_LINES_PER_FILE);
    candidates.truncate(MAX_LINES_PER_FILE);
    let score = candidates.iter().map(|index| weight(surface[*index])).sum();
    let mut names: Vec<&str> = Vec::new();
    // Item names identify a file far better than member names like `new`.
    let mut by_identity = candidates.clone();
    by_identity.sort_by_key(|index| surface[*index].depth);
    for index in &by_identity {
        let name = surface[*index].name.as_str();
        if names.len() < COMPACT_NAMES && !names.contains(&name) {
            names.push(name);
        }
    }
    let line = format!("  {path}: {}\n", names.join(", "));
    let mut chosen: HashSet<usize> = candidates.iter().copied().collect();
    for index in candidates {
        if surface[index].depth > 0
            && let Some(parent) = (0..index).rev().find(|parent| surface[*parent].depth == 0)
        {
            chosen.insert(parent);
        }
    }
    let mut block = format!("{path}\n");
    for (index, symbol) in surface.iter().enumerate() {
        if chosen.contains(&index) {
            block.push_str(if symbol.depth > 0 { "    " } else { "  " });
            block.push_str(&symbol.signature);
            block.push('\n');
        }
    }
    if omitted > 0 {
        block.push_str(&format!("  … {omitted} more\n"));
    }
    Some((score, block, line))
}

fn is_test_path(path: &str) -> bool {
    path.split('/').any(|part| {
        matches!(part, "test" | "tests" | "__tests__" | "fixtures")
            || part.ends_with("_test.rs")
            || part.ends_with("tests.rs")
            || part.contains(".test.")
            || part.contains(".spec.")
    })
}

fn parse_cached(path: &Path) -> Option<FileSurface> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let stamp = (metadata.len(), metadata.modified().ok());
    if let Ok(cache) = parse_cache().lock()
        && let Some(entry) = cache.get(path)
        && entry.stamp == stamp
    {
        return Some((entry.symbols.clone(), entry.identifiers.clone()));
    }
    let source = std::fs::read_to_string(path).ok()?;
    let symbols = Arc::new(symbols::outline(path, &source));
    let identifiers = Arc::new(
        source
            .split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
            .filter(|word| word.len() >= 3 && !word.starts_with(|ch: char| ch.is_ascii_digit()))
            .map(str::to_owned)
            .collect::<HashSet<_>>(),
    );
    if let Ok(mut cache) = parse_cache().lock() {
        cache.insert(
            path.to_path_buf(),
            ParsedFile {
                stamp,
                symbols: symbols.clone(),
                identifiers: identifiers.clone(),
            },
        );
    }
    Some((symbols, identifiers))
}

/// Sorted workspace-relative paths of supported source files. Git's view
/// (tracked plus untracked-but-not-ignored) is authoritative when available so
/// .gitignore is respected; otherwise fall back to a bounded directory walk.
fn source_files(root: &Path) -> Vec<String> {
    let mut files = git_files(root).unwrap_or_else(|| walk_files(root));
    files.retain(|path| symbols::supported(Path::new(path)));
    files.sort();
    files.dedup();
    files.truncate(MAX_FILES_SCANNED);
    files
}

fn git_files(root: &Path) -> Option<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .filter_map(|entry| std::str::from_utf8(entry).ok().map(str::to_owned))
            .collect(),
    )
}

fn walk_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                if !name.starts_with('.')
                    && !super::io::ignored_workspace_directory(&name)
                    && !matches!(name.as_str(), "target" | "dist" | "build")
                {
                    stack.push(entry.path());
                }
            } else if file_type.is_file()
                && let Ok(relative) = entry.path().strip_prefix(root)
            {
                files.push(relative.to_string_lossy().replace('\\', "/"));
            }
            if files.len() >= MAX_FILES_SCANNED * 4 {
                return files;
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_is_deterministic_body_edit_stable_and_budget_capped() {
        let root = std::env::temp_dir().join(format!("yeet-repo-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        for index in 0..40 {
            std::fs::write(
                root.join(format!("src/m{index:02}.rs")),
                format!(
                    "pub struct Widget{index} {{ value: u32 }}\npub fn build_widget_{index}(value: u32) -> Widget{index} {{ Widget{index} {{ value }} }}\n"
                ),
            )
            .unwrap();
        }
        // core.rs is referenced by every other file, so it must rank in.
        std::fs::write(root.join("src/core.rs"), "pub fn shared_core() {}\n").unwrap();
        for index in 0..40 {
            let path = root.join(format!("src/m{index:02}.rs"));
            let mut text = std::fs::read_to_string(&path).unwrap();
            text.push_str("fn uses() { shared_core(); }\n");
            std::fs::write(path, text).unwrap();
        }

        let first = build(&root, 200).unwrap();
        assert!(first.text.len() <= 200 * 3, "{}", first.text.len());
        assert!(first.shown_files < first.source_files);
        assert!(first.text.contains("src/core.rs\n  pub fn shared_core()"));
        assert_eq!(build(&root, 200).unwrap(), first);

        // A body-only edit keeps the overlay byte-identical.
        std::fs::write(
            root.join("src/core.rs"),
            "pub fn shared_core() { let _changed = 1; }\n",
        )
        .unwrap();
        assert_eq!(build(&root, 200).unwrap().digest, first.digest);
        let _ = std::fs::remove_dir_all(&root);
    }
}
