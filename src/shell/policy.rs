//! Conservative shell command classification for permission checks.

use std::{collections::HashSet, path::Path};

use super::{command_words, is_assignment, split_shell_segments};

const MAX_NESTED_SHELL_POLICY_DEPTH: usize = 32;

pub fn restricted_operation(command: &str) -> Option<String> {
    restricted_operation_inner(command, 0)
}

fn restricted_operation_inner(command: &str, depth: usize) -> Option<String> {
    if depth >= MAX_NESTED_SHELL_POLICY_DEPTH {
        // Treat excessively nested shell wrappers conservatively instead of
        // recursively walking an attacker- or model-controlled command until
        // the MCP stdio process exhausts its main-thread stack.
        return Some("nested shell command".into());
    }
    let write_executables: HashSet<&str> = [
        "rm",
        "mv",
        "cp",
        "mkdir",
        "rmdir",
        "touch",
        "truncate",
        "tee",
        "install",
        "ln",
        "chmod",
        "chown",
        "chgrp",
        "chflags",
        "xattr",
        "patch",
        "ed",
        "ex",
        "vi",
        "vim",
        "nvim",
        "nano",
        "emacs",
        "del",
        "erase",
        "copy",
        "move",
        "md",
        "rd",
        "ren",
        "rename",
        "remove-item",
        "move-item",
        "copy-item",
        "new-item",
        "set-content",
        "add-content",
        "clear-content",
        "rename-item",
        "set-item",
        "out-file",
    ]
    .into_iter()
    .collect();
    let git_mutating: HashSet<&str> = [
        "add",
        "am",
        "apply",
        "branch",
        "checkout",
        "cherry-pick",
        "clean",
        "clone",
        "commit",
        "config",
        "fetch",
        "gc",
        "init",
        "merge",
        "mv",
        "pull",
        "push",
        "rebase",
        "reset",
        "restore",
        "revert",
        "rm",
        "stash",
        "submodule",
        "switch",
        "tag",
        "worktree",
    ]
    .into_iter()
    .collect();
    let dependency_tools: HashSet<&str> = [
        "npm", "pnpm", "yarn", "bun", "cargo", "pip", "pip3", "gem", "brew",
    ]
    .into_iter()
    .collect();
    let dependency_mutating: HashSet<&str> = [
        "add",
        "install",
        "uninstall",
        "remove",
        "rm",
        "update",
        "upgrade",
        "link",
        "unlink",
        "publish",
        "pack",
        "prune",
        "dedupe",
    ]
    .into_iter()
    .collect();

    for segment in split_shell_segments(command) {
        let words = command_words(&segment);
        if words.is_empty() {
            continue;
        }
        let mut index = 0;
        while index < words.len()
            && (is_assignment(&words[index])
                || matches!(
                    words[index].as_str(),
                    "command" | "exec" | "env" | "nice" | "nohup" | "time" | "sudo"
                ))
        {
            index += 1;
        }
        let Some(raw) = words.get(index) else {
            continue;
        };
        let executable = Path::new(raw)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or(raw)
            .to_ascii_lowercase();
        if write_executables.contains(executable.as_str()) {
            return Some(executable);
        }
        let rest = &words[index + 1..];
        if executable == "sed"
            && rest.iter().any(|word| {
                word == "--in-place"
                    || word.starts_with("--in-place=")
                    || (word.starts_with('-') && !word.starts_with("--") && word[1..].contains('i'))
            })
        {
            return Some("sed -i".into());
        }
        if executable == "git" {
            if rest.len() == 2
                && rest[0].eq_ignore_ascii_case("branch")
                && rest[1].eq_ignore_ascii_case("--show-current")
            {
                continue;
            }
            let mut skip_next = false;
            for word in rest {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if matches!(
                    word.as_str(),
                    "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path"
                ) {
                    skip_next = true;
                    continue;
                }
                if word.starts_with('-') {
                    continue;
                }
                if git_mutating.contains(word.to_ascii_lowercase().as_str()) {
                    return Some(format!("git {word}"));
                }
                break;
            }
        }
        if dependency_tools.contains(executable.as_str())
            && let Some(subcommand) = rest.iter().find(|word| !word.starts_with('-'))
            && dependency_mutating.contains(subcommand.to_ascii_lowercase().as_str())
        {
            return Some(format!("{executable} {subcommand}"));
        }
        if matches!(executable.as_str(), "sh" | "bash" | "zsh") {
            for pair in rest.windows(2) {
                if pair[0].starts_with('-')
                    && pair[0].contains('c')
                    && let Some(value) = restricted_operation_inner(&pair[1], depth + 1)
                {
                    return Some(value);
                }
            }
        }
        if matches!(executable.as_str(), "cmd" | "cmd.exe") {
            for pair in rest.windows(2) {
                if matches!(pair[0].to_ascii_lowercase().as_str(), "/c" | "/k")
                    && let Some(value) = restricted_operation_inner(&pair[1], depth + 1)
                {
                    return Some(value);
                }
            }
        }
        if matches!(
            executable.as_str(),
            "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
        ) {
            for pair in rest.windows(2) {
                let flag = pair[0].to_ascii_lowercase();
                if matches!(flag.as_str(), "-c" | "-command")
                    && let Some(value) = restricted_operation_inner(&pair[1], depth + 1)
                {
                    return Some(value);
                }
                // An encoded command hides its script text from naive
                // inspection; decode it and classify the payload.
                if matches!(flag.as_str(), "-e" | "-ec" | "-encodedcommand")
                    && let Some(script) = decode_powershell_command(&pair[1])
                    && let Some(value) = restricted_operation_inner(&script, depth + 1)
                {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn decode_powershell_command(encoded: &str) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    if bytes.len() % 2 != 0 {
        return None;
    }
    let (units, _) = bytes.as_chunks::<2>();
    let units = units
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    String::from_utf16(&units).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(script: &str) -> String {
        use base64::Engine;
        let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn encoded_powershell_commands_are_classified_by_their_payload() {
        let hidden = format!("pwsh -EncodedCommand {}", encode("Remove-Item -Recurse x"));
        assert_eq!(restricted_operation(&hidden), Some("remove-item".into()));
        let hidden = format!("powershell -ec {}", encode("Get-Date"));
        assert_eq!(restricted_operation(&hidden), None);
        assert_eq!(
            restricted_operation("pwsh -EncodedCommand not-base64!"),
            None
        );
    }
}
