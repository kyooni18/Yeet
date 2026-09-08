//! Architectural regression checks for source-module growth.
//!
//! The test excludes individual `#[cfg(test)]` items rather than assuming all
//! tests live at the end of a file. Focused child modules are expected to stay
//! small, while the hard ceiling prevents a parent module from regressing into
//! a multi-thousand-line implementation before it is split again.

use std::{
    fs,
    path::{Path, PathBuf},
};

const HARD_PRODUCTION_LINE_LIMIT: usize = 1_700;
const HARD_FILE_LINE_LIMIT: usize = 3_000;
const TARGET_MODULE_LINE_LIMIT: usize = 1_200;
const COORDINATOR_PRODUCTION_LINE_LIMIT: usize = 1_250;
const RUNTIME_SOURCE_LINE_LIMIT: usize = 1_200;

#[test]
fn rust_production_modules_have_a_hard_size_ceiling() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut oversized = Vec::new();
    visit_rust_files(&root, &mut |path| {
        let source = fs::read_to_string(path).expect("source file should be readable");
        let production_lines = production_line_count(&source);
        if production_lines > HARD_PRODUCTION_LINE_LIMIT {
            oversized.push(format!("{}: {production_lines}", display_path(path)));
        }
    });
    assert!(
        oversized.is_empty(),
        "production modules exceeded the {HARD_PRODUCTION_LINE_LIMIT}-line hard ceiling; split by responsibility before adding more code:\n{}",
        oversized.join("\n")
    );
}

#[test]
fn rust_source_files_do_not_return_to_monolithic_sizes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut oversized = Vec::new();
    visit_rust_files(&root, &mut |path| {
        let source = fs::read_to_string(path).expect("source file should be readable");
        let lines = source.lines().count();
        if lines > HARD_FILE_LINE_LIMIT {
            oversized.push(format!("{}: {lines}", display_path(path)));
        }
    });
    assert!(
        oversized.is_empty(),
        "source files exceeded the {HARD_FILE_LINE_LIMIT}-line absolute ceiling; split tests and implementation before adding more code:\n{}",
        oversized.join("\n")
    );
}

#[test]
fn extracted_responsibility_modules_stay_within_target() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let focused_roots = [root.join("agent"), root.join("backend"), root.join("tools")];
    let mut oversized = Vec::new();
    for focused_root in focused_roots {
        visit_rust_files(&focused_root, &mut |path| {
            let source = fs::read_to_string(path).expect("source file should be readable");
            let lines = source.lines().count();
            if lines > TARGET_MODULE_LINE_LIMIT {
                oversized.push(format!("{}: {lines}", display_path(path)));
            }
        });
    }
    assert!(
        oversized.is_empty(),
        "focused responsibility modules exceeded the {TARGET_MODULE_LINE_LIMIT}-line target:\n{}",
        oversized.join("\n")
    );
}

#[test]
fn central_coordinators_do_not_absorb_extracted_responsibilities_again() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let coordinators = [
        root.join("agent.rs"),
        root.join("backend.rs"),
        root.join("tools.rs"),
    ];
    let mut oversized = Vec::new();
    for path in coordinators {
        let source = fs::read_to_string(&path).expect("coordinator source should be readable");
        let production_lines = production_line_count(&source);
        if production_lines > COORDINATOR_PRODUCTION_LINE_LIMIT {
            oversized.push(format!("{}: {production_lines}", display_path(&path)));
        }
    }
    assert!(
        oversized.is_empty(),
        "central coordinators exceeded the {COORDINATOR_PRODUCTION_LINE_LIMIT}-line production ceiling; put new behavior in a responsibility module instead:\n{}",
        oversized.join("\n")
    );
}

#[test]
fn runtime_source_modules_stay_bounded() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("RuntimeSource/src");
    let mut oversized = Vec::new();
    visit_source_files(&root, "ts", &mut |path| {
        let source = fs::read_to_string(path).expect("RuntimeSource file should be readable");
        let lines = source.lines().count();
        if lines > RUNTIME_SOURCE_LINE_LIMIT {
            oversized.push(format!("{}: {lines}", display_path(path)));
        }
    });
    assert!(
        oversized.is_empty(),
        "RuntimeSource modules exceeded the {RUNTIME_SOURCE_LINE_LIMIT}-line ceiling; split by responsibility before adding more code:\n{}",
        oversized.join("\n")
    );
}

#[test]
fn extracted_modules_explain_their_responsibility() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let modules = [
        "agent/cache.rs",
        "agent/policy.rs",
        "agent/progress.rs",
        "agent/runaway.rs",
        "agent/tool_protocol.rs",
        "backend/debate_context.rs",
        "backend/debate_runtime.rs",
        "backend/events.rs",
        "backend/lifecycle.rs",
        "backend/settings.rs",
        "backend/settings_support.rs",
        "backend/state.rs",
        "backend/titles.rs",
        "backend/transport.rs",
        "tools/definitions.rs",
        "tools/editing.rs",
        "tools/io.rs",
        "tools/paths.rs",
        "tools/shell_runtime.rs",
        "tools/support.rs",
    ];
    let mut undocumented = Vec::new();
    for relative in modules {
        let path = root.join(relative);
        let source = fs::read_to_string(&path).expect("extracted module should be readable");
        let first_nonempty = source
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default();
        if !first_nonempty.trim_start().starts_with("//!") {
            undocumented.push(relative.to_owned());
        }
    }
    assert!(
        undocumented.is_empty(),
        "extracted modules must start with a `//!` responsibility note:\n{}",
        undocumented.join("\n")
    );
}

/// Recursively visits Rust source files under one directory.
fn visit_rust_files(root: &Path, visitor: &mut impl FnMut(&Path)) {
    visit_source_files(root, "rs", visitor);
}

/// Recursively visits source files with one extension under a directory.
fn visit_source_files(root: &Path, extension: &str, visitor: &mut impl FnMut(&Path)) {
    let mut entries = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("unable to read {}: {error}", root.display()))
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            visit_source_files(&path, extension, visitor);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            visitor(&path);
        }
    }
}

/// Counts lines outside items guarded by `#[cfg(test)]`.
fn production_line_count(source: &str) -> usize {
    let mut production = 0usize;
    let mut pending_test_item = false;
    let mut skipping_test_item = false;
    let mut test_brace_depth = 0isize;

    for line in source.lines() {
        let trimmed = line.trim();
        if skipping_test_item {
            test_brace_depth += brace_delta(line);
            if test_brace_depth <= 0 {
                skipping_test_item = false;
                test_brace_depth = 0;
            }
            continue;
        }
        if pending_test_item {
            if trimmed.starts_with("#[") || trimmed.is_empty() {
                continue;
            }
            let delta = brace_delta(line);
            if delta > 0 {
                skipping_test_item = true;
                test_brace_depth = delta;
            } else if !trimmed.ends_with(';') {
                // Multi-line signature or declaration: remain pending until
                // its opening brace or terminating semicolon appears.
                pending_test_item = true;
                continue;
            }
            pending_test_item = false;
            continue;
        }
        if trimmed == "#[cfg(test)]" {
            pending_test_item = true;
            continue;
        }
        production += 1;
    }
    production
}

/// Computes lexical brace depth for ordinary Rust item boundaries.
fn brace_delta(line: &str) -> isize {
    line.chars().fold(0isize, |depth, ch| match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

/// Produces a repository-relative path for actionable test failures.
fn display_path(path: &Path) -> String {
    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
        .unwrap_or(path)
        .display()
        .to_string()
}
