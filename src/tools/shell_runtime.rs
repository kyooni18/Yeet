//! Shell-tool execution, routing, and deterministic output summarization.
//!
//! Shell policy remains part of `ToolRegistry`, while command routing and
//! output heuristics are isolated from generic tool dispatch.

use super::*;

const DEFAULT_BACKGROUND_TIMEOUT_SECONDS: usize = 4 * 60 * 60;

impl ToolRegistry {
    /// Executes a shell command under sandbox/approval policy and selects direct or actor output.
    pub(super) fn run_shell_tool(
        &mut self,
        object: &Map<String, Value>,
        _model: &str,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let background = object
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let command = string_arg(object, "command")?.trim().to_owned();
        let has_permit = self.permitted_shell_commands.remove(&command);
        let restricted = restricted_operation(&command);
        let requested_working_directory = object
            .get("workingDirectory")
            .and_then(Value::as_str)
            .unwrap_or(".");
        let effective_cwd = self.resolve_session_path(requested_working_directory)?;
        if !effective_cwd.is_dir() {
            bail!(
                "Invalid shell working directory: {}",
                effective_cwd.display()
            );
        }
        let context_root = self.context_root_for_path(&effective_cwd).cloned();
        let shell_root = context_root
            .clone()
            .unwrap_or_else(|| self.workspace_root.clone());
        let effective_working_directory = effective_cwd.to_string_lossy().into_owned();
        let policy = SandboxStore::new(&shell_root)?.load()?;
        let hard_confined = self.hard_access_root.is_some();
        let mut unrestricted =
            !hard_confined && (policy.mode == SandboxMode::Unlimited || has_permit);
        let mut allow_write = false;
        if restricted.is_some() && self.disabled_capabilities.contains("builtin:file-write") {
            bail!("File Write is disabled for this session; shell is read-only");
        }
        if context_root.is_none() {
            if hard_confined {
                bail!(
                    "MCP shell workingDirectory must stay inside the selected workspace; select a workspace within the configured MCP access root instead"
                );
            }
            if !unrestricted {
                let reason = object.get("purpose").and_then(Value::as_str).unwrap_or(
                    "The command needs to run outside the active session context roots.",
                );
                if !self.request_approval(
                    "shell",
                    &command,
                    "outside-context shell access",
                    reason,
                )? {
                    bail!("User denied outside-context shell access");
                }
                unrestricted = true;
            }
        }
        if let Some(operation) = restricted.as_deref()
            && !unrestricted
        {
            let reason = object
                .get("purpose")
                .and_then(Value::as_str)
                .unwrap_or("The command needs write access inside the current project.");
            if !self.request_approval("shell", &command, operation, reason)? {
                bail!("User denied shell operation '{operation}'");
            }
            allow_write = true;
        }
        let _mutation_guard = if restricted.is_some() {
            if background {
                bail!(
                    "Mutating shell commands cannot run detached. Run this command in the foreground so Yeet can hold the workspace mutation lease until it finishes."
                );
            }
            self.workspace_mutation_guard()?
        } else {
            None
        };
        let cache_safe_inspection = restricted.is_none() && shell_is_inspection(&command);
        if !background && !allow_write && cache_safe_inspection {
            let signature = normalize_shell_inspection(&command);
            if !self.shell_inspections.insert(signature) {
                return Ok(json!({
                    "duplicate": true,
                    "contentAlreadyReturned": true,
                    "succeeded": true,
                    "command": command,
                    "hint": "This shell inspection was already executed in this task. Reuse its prior result instead of replaying it."
                }).to_string());
            }
            if let Some(reason) = self.covered_shell_replay_reason(&command) {
                return Ok(json!({
                    "duplicate": true,
                    "contentAlreadyReturned": true,
                    "succeeded": true,
                    "command": command,
                    "blockedReplay": true,
                    "hint": reason
                })
                .to_string());
            }
        }
        let requested_mode = object.get("mode").and_then(Value::as_str).unwrap_or("auto");
        if !matches!(requested_mode, "auto" | "direct" | "actor") {
            bail!("run_shell mode must be auto, direct, or actor");
        }
        let actor =
            requested_mode == "actor" || (requested_mode == "auto" && shell_actor_route(&command));
        let default_timeout = if background {
            DEFAULT_BACKGROUND_TIMEOUT_SECONDS
        } else if actor {
            120
        } else {
            15
        };
        let timeout = usize_arg(object, "timeoutSeconds").unwrap_or(default_timeout) as u64;
        if background {
            let id = self.shell_jobs.start(
                command.clone(),
                shell_root.clone(),
                Some(effective_working_directory.clone()),
                timeout,
                allow_write,
                unrestricted,
                hard_confined,
            )?;
            self.workspace_write_generation = self.workspace_write_generation.wrapping_add(1);
            self.invalidate_workspace_cache();
            let hint = if self.artifacts_enabled {
                "Use shell_job action=wait to suspend until completion without model polling. Set reportEverySeconds only when periodic monitoring is useful; use check only for an immediate snapshot."
            } else {
                "Use run_shell with job:{action:\"wait\",jobId:<id>} to suspend until completion without model polling. Add reportEverySeconds only when periodic monitoring is useful."
            };
            return Ok(
                json!({"jobId":id,"status":"running","command":command,"hint":hint}).to_string(),
            );
        }
        let output = run_shell_cancellable(ShellExecutionRequest {
            command: &command,
            workspace_root: &shell_root,
            working_directory: Some(&effective_working_directory),
            timeout_seconds: timeout,
            capture_bytes: if actor { 512 * 1024 } else { 64 * 1024 },
            allow_write,
            unrestricted,
            force_sandboxed: hard_confined,
            cancel: Some(cancel),
        })?;
        if allow_write || (unrestricted && !cache_safe_inspection) {
            if allow_write || restricted.is_some() {
                self.workspace_write_generation = self.workspace_write_generation.wrapping_add(1);
            }
            self.invalidate_workspace_cache();
        }
        if !actor {
            let value = serde_json::to_value(&output)?;
            if self.artifacts_enabled && output.stdout_bytes + output.stderr_bytes > 6 * 1024 {
                let raw = format!(
                    "$ {}\n{}{}",
                    command,
                    output.stdout.clone().unwrap_or_default(),
                    output.stderr.clone().unwrap_or_default()
                );
                let artifact = self.artifacts.store(&raw)?;
                return Ok(json!({
                    "route":"direct","command":output.command,"workingDirectory":output.working_directory,"exitCode":output.exit_code,
                    "succeeded":output.succeeded,"durationMilliseconds":output.duration_milliseconds,"stdoutBytes":output.stdout_bytes,
                    "stderrBytes":output.stderr_bytes,"stdoutTruncated":output.stdout_truncated,"stderrTruncated":output.stderr_truncated,
                "summary":"Large shell output was externalized. Use a narrow read_artifact range for specific missing evidence, and search_artifact only when its location is unknown.","artifactId":artifact
                }).to_string());
            }
            return Ok(value.to_string());
        }
        self.evaluate_shell_output(&command, output)
    }

    /// Summarizes noisy shell output locally and retains raw output only when artifacts are enabled.
    ///
    /// Actor mode used to make a second provider request here. Besides spending
    /// tokens on data we already had, that request could fail when the selected
    /// endpoint was batch-only. Exit status plus deterministic diagnostic
    /// extraction is authoritative and provider-independent.
    fn evaluate_shell_output(
        &mut self,
        command: &str,
        output: crate::shell::ShellResult,
    ) -> Result<String> {
        if !self.artifacts_enabled {
            let bytes = output.stdout.as_ref().map_or(0, String::len)
                + output.stderr.as_ref().map_or(0, String::len);
            if bytes <= 2048 && !output.stdout_truncated && !output.stderr_truncated {
                return Ok(json!({
                    "route":"actor", "command":output.command,
                    "workingDirectory":output.working_directory, "exitCode":output.exit_code,
                    "succeeded":output.succeeded, "stdout":output.stdout, "stderr":output.stderr
                })
                .to_string());
            }
            let diagnostics = shell_diagnostic_lines(&output, 8);
            let summary = deterministic_shell_summary(&output, &diagnostics);
            return Ok(json!({
                "route":"actor","command":command,"workingDirectory":output.working_directory,"exitCode":output.exit_code,
                "succeeded":output.succeeded,"durationMilliseconds":output.duration_milliseconds,"stdoutBytes":output.stdout_bytes,
                "stderrBytes":output.stderr_bytes,"stdoutTruncated":output.stdout_truncated,"stderrTruncated":output.stderr_truncated,
                "summary":summary,"diagnostics":diagnostics
            }).to_string());
        }

        let raw = format!(
            "COMMAND: {command}\nEXIT: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
            output.exit_code,
            output.stdout.clone().unwrap_or_default(),
            output.stderr.clone().unwrap_or_default()
        );
        let artifact = self.artifacts.store(&raw)?;
        if let Some(result) = inline_shell_result(&output, &artifact) {
            return Ok(result);
        }
        let diagnostics = shell_diagnostic_lines(&output, 8);
        let summary = deterministic_shell_summary(&output, &diagnostics);
        Ok(json!({
            "route":"actor","command":command,"workingDirectory":output.working_directory,"exitCode":output.exit_code,
            "succeeded":output.succeeded,"durationMilliseconds":output.duration_milliseconds,"stdoutBytes":output.stdout_bytes,
            "stderrBytes":output.stderr_bytes,"stdoutTruncated":output.stdout_truncated,"stderrTruncated":output.stderr_truncated,
            "summary":summary,"diagnostics":diagnostics,"artifactId":artifact
        }).to_string())
    }
}

fn shell_diagnostic_lines(output: &crate::shell::ShellResult, limit: usize) -> Vec<String> {
    const NEEDLES: &[&str] = &[
        "error",
        "warning",
        "failed",
        "failure",
        "panic",
        "fatal",
        "exception",
        "assert",
    ];
    let mut diagnostics = Vec::new();
    for text in [output.stderr.as_deref(), output.stdout.as_deref()]
        .into_iter()
        .flatten()
    {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();
            if !NEEDLES.iter().any(|needle| lower.contains(needle)) {
                continue;
            }
            let bounded = trimmed.chars().take(320).collect::<String>();
            if !diagnostics.contains(&bounded) {
                diagnostics.push(bounded);
            }
            if diagnostics.len() >= limit {
                return diagnostics;
            }
        }
    }
    if !output.succeeded && diagnostics.is_empty() {
        let fallback = output
            .stderr
            .as_deref()
            .and_then(|text| text.lines().find(|line| !line.trim().is_empty()))
            .or_else(|| {
                output
                    .stdout
                    .as_deref()
                    .and_then(|text| text.lines().find(|line| !line.trim().is_empty()))
            });
        if let Some(line) = fallback {
            diagnostics.push(line.trim().chars().take(320).collect());
        }
    }
    diagnostics
}

fn deterministic_shell_summary(
    output: &crate::shell::ShellResult,
    diagnostics: &[String],
) -> String {
    let status = if output.succeeded {
        "Command succeeded".to_owned()
    } else {
        format!("Command failed with exit code {}", output.exit_code)
    };
    if diagnostics.is_empty() {
        return format!("{status} without error or warning diagnostics.");
    }
    let excerpt = diagnostics
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    format!("{status}. {excerpt}")
}

/// Small diagnostics cost less to forward intact than to summarize with another call.
fn inline_shell_result(output: &crate::shell::ShellResult, artifact: &str) -> Option<String> {
    let bytes = output.stdout.as_ref().map_or(0, String::len)
        + output.stderr.as_ref().map_or(0, String::len);
    if bytes > 2048 || output.stdout_truncated || output.stderr_truncated {
        return None;
    }
    Some(
        json!({
            "route":"actor", "command":output.command,
            "workingDirectory":output.working_directory, "exitCode":output.exit_code,
            "succeeded":output.succeeded, "stdout":output.stdout, "stderr":output.stderr,
            "artifactId":artifact
        })
        .to_string(),
    )
}

/// Returns whether a command should default to actor-mode evaluation.
fn shell_actor_route(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    [
        "cargo test",
        "cargo build",
        "swift test",
        "swift build",
        "npm test",
        "npm run",
        "pnpm ",
        "yarn ",
        "bun test",
        "xcodebuild",
        "make",
        "cmake",
        "pytest",
        "go test",
        "gradle",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
        || lower.contains(" | ")
        || lower.contains("&&")
        || lower.contains('\n')
}

/// Returns whether a command is a cache-safe read-only inspection.
pub(super) fn shell_is_inspection(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    [
        "cat ",
        "head ",
        "tail ",
        "sed ",
        "awk ",
        "grep ",
        "rg ",
        "ls",
        "find ",
        "tree ",
        "pwd",
        "git status",
        "git diff",
        "git log",
        "git show",
        "git rev-parse",
        "git ls-files",
        "git ls-tree",
        "git grep",
        "stat ",
        "wc ",
        "file ",
        "ps ",
        "pgrep ",
        "lsof ",
        "shasum ",
        "sha256sum ",
        "md5 ",
        "which ",
        "command -v ",
        "type -a ",
        "test ",
        "uname",
        "sw_vers",
    ]
    .iter()
    .any(|prefix| {
        lower.starts_with(prefix)
            || lower.contains(&format!("; {prefix}"))
            || lower.contains(&format!("&& {prefix}"))
            || lower.contains(&format!("| {prefix}"))
            || lower.contains(&format!("\n{prefix}"))
    })
}

/// Normalizes command whitespace for exact replay suppression.
fn normalize_shell_inspection(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Detects simple shell references to a path already covered by structured tools.
pub(super) fn shell_mentions_path(command: &str, path: &str) -> bool {
    if path == "." {
        return true;
    }
    command.contains(path)
        || command.contains(&format!("./{path}"))
        || command.contains(&format!("'{path}'"))
        || command.contains(&format!("\"{path}\""))
}
