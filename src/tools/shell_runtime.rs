//! Shell-tool execution, routing, and bounded model evaluation.
//!
//! Shell policy remains part of `ToolRegistry`, while command routing and
//! evaluator-specific heuristics are isolated from generic tool dispatch.

use super::*;

const SHELL_EVALUATION_CHARS: usize = 28 * 1024;

impl ToolRegistry {
    /// Executes a shell command under sandbox/approval policy and selects direct or actor output.
    pub(super) fn run_shell_tool(
        &mut self,
        object: &Map<String, Value>,
        model: &str,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let background = object
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let command = string_arg(object, "command")?.trim().to_owned();
        let has_permit = self.permitted_shell_commands.remove(&command);
        let restricted = restricted_operation(&command);
        let policy = SandboxStore::new(&self.workspace_root)?.load()?;
        let mut unrestricted = policy.mode == SandboxMode::Unlimited || has_permit;
        let mut allow_write = false;
        if restricted.is_some() && self.disabled_capabilities.contains("builtin:file-write") {
            bail!("File Write is disabled for this session; shell is read-only");
        }
        if !unrestricted
            && let Some(directory) = object.get("workingDirectory").and_then(Value::as_str)
            && path_outside_workspace(&self.workspace_root, directory)?
        {
            let reason = object
                .get("purpose")
                .and_then(Value::as_str)
                .unwrap_or("The command needs to run outside the current project directory.");
            if !self.request_approval("shell", &command, "outside-project shell access", reason)? {
                bail!("User denied outside-project shell access");
            }
            unrestricted = true;
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
        let timeout =
            usize_arg(object, "timeoutSeconds").unwrap_or(if actor { 120 } else { 15 }) as u64;
        if background {
            let id = self.shell_jobs.start(
                command.clone(),
                self.workspace_root.clone(),
                object
                    .get("workingDirectory")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                timeout,
                allow_write,
                unrestricted,
                self.protected_write_paths.clone(),
            )?;
            self.workspace_write_generation = self.workspace_write_generation.wrapping_add(1);
            self.invalidate_workspace_cache();
            return Ok(json!({"jobId":id,"status":"running","command":command,
                "hint":"Use shell_job to check completion or stop this job. Do other work while it runs."}).to_string());
        }
        let output = run_shell_cancellable_with_protected_paths(
            ShellExecutionRequest {
                command: &command,
                workspace_root: &self.workspace_root,
                working_directory: object.get("workingDirectory").and_then(Value::as_str),
                timeout_seconds: timeout,
                capture_bytes: if actor { 512 * 1024 } else { 64 * 1024 },
                allow_write,
                unrestricted,
                cancel: Some(cancel),
            },
            &self.protected_write_paths,
        )?;
        if allow_write || (unrestricted && !cache_safe_inspection) {
            if allow_write || restricted.is_some() {
                self.workspace_write_generation = self.workspace_write_generation.wrapping_add(1);
            }
            self.invalidate_workspace_cache();
        }
        if !actor {
            let value = serde_json::to_value(&output)?;
            if output.stdout_bytes + output.stderr_bytes > 6 * 1024 {
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
                    "summary":"Large shell output was externalized. Use search_artifact first, then a narrow read_artifact range if needed.","artifactId":artifact
                }).to_string());
            }
            return Ok(value.to_string());
        }
        self.evaluate_shell_output(
            &command,
            object.get("purpose").and_then(Value::as_str),
            model,
            output,
            cancel,
        )
    }

    /// Summarizes noisy shell output with the selected model while retaining raw output as an artifact.
    fn evaluate_shell_output(
        &mut self,
        command: &str,
        purpose: Option<&str>,
        model: &str,
        output: crate::shell::ShellResult,
        cancel: &AtomicBool,
    ) -> Result<String> {
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
        if !shell_output_needs_model_evaluation(&output) {
            return Ok(json!({
                "route":"actor","command":command,"workingDirectory":output.working_directory,"exitCode":output.exit_code,
                "succeeded":true,"durationMilliseconds":output.duration_milliseconds,"stdoutBytes":output.stdout_bytes,
                "stderrBytes":output.stderr_bytes,"summary":"Command succeeded without error or warning diagnostics.","artifactId":artifact
            }).to_string());
        }
        let evaluation_input = bounded_shell_evaluation_input(&raw);
        let prompt = format!(
            "Evaluate this developer command result for the lead agent. Purpose: {}\nReturn only a compact factual summary: success/failure, important diagnostics, and the minimal actionable details. Do not invent facts.\n\n{}",
            purpose.unwrap_or(
                "Determine whether the command succeeded and extract important diagnostics."
            ),
            evaluation_input
        );
        let mut request = CallRequest::simple(model, vec![Message::user(prompt)]);
        request.max_tokens = Some(512);
        request.metadata = Some(HashMap::from([(
            "purpose".into(),
            "command-evaluation".into(),
        )]));
        request.attached_capabilities = Some(Vec::new());
        let (summary, evaluator_error) = match self.bridge.complete_cancellable(&request, cancel) {
            Ok(result) => {
                if let Some(usage) = &result.usage {
                    self.accumulate_auxiliary_usage(usage);
                }
                (result.text, None)
            }
            Err(error) => {
                let diagnostic = output
                    .stderr
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| {
                        output
                            .stdout
                            .as_deref()
                            .filter(|value| !value.trim().is_empty())
                    })
                    .and_then(|value| value.lines().find(|line| !line.trim().is_empty()))
                    .map(|line| line.trim().chars().take(320).collect::<String>());
                let summary = if output.succeeded {
                    "Command succeeded; auxiliary output evaluation failed, so the recorded exit status and artifact are authoritative.".to_owned()
                } else if let Some(diagnostic) = diagnostic {
                    format!(
                        "Command failed with exit code {}. {diagnostic}",
                        output.exit_code
                    )
                } else {
                    format!("Command failed with exit code {}.", output.exit_code)
                };
                (summary, Some(error.to_string()))
            }
        };
        Ok(json!({
            "route":"actor","command":command,"workingDirectory":output.working_directory,"exitCode":output.exit_code,
            "succeeded":output.succeeded,"durationMilliseconds":output.duration_milliseconds,"stdoutBytes":output.stdout_bytes,
            "stderrBytes":output.stderr_bytes,"summary":summary,"artifactId":artifact,
            "evaluatorError":evaluator_error
        }).to_string())
    }
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

/// Bounds shell evaluator context while preserving diagnostics and both ends of output.
pub(super) fn bounded_shell_evaluation_input(raw: &str) -> String {
    if raw.chars().count() <= SHELL_EVALUATION_CHARS {
        return raw.to_owned();
    }
    const HEAD_CHARS: usize = 7 * 1024;
    const TAIL_CHARS: usize = 11 * 1024;
    const DIAGNOSTIC_CHARS: usize = 8 * 1024;
    let head = raw.chars().take(HEAD_CHARS).collect::<String>();
    let tail = raw
        .chars()
        .rev()
        .take(TAIL_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let mut diagnostics = String::new();
    for line in raw.lines() {
        let lower = line.to_ascii_lowercase();
        if ![
            "error",
            "warning",
            "failed",
            "failure",
            "panic",
            "fatal",
            "exception",
            "assert",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            continue;
        }
        let remaining = DIAGNOSTIC_CHARS.saturating_sub(diagnostics.chars().count());
        if remaining == 0 {
            break;
        }
        let line = line.chars().take(remaining.min(800)).collect::<String>();
        diagnostics.push_str(&line);
        diagnostics.push('\n');
    }
    format!(
        "{head}\n\n[... middle of shell output omitted from evaluator; full log is stored as an artifact ...]\n\nDIAGNOSTIC LINES:\n{diagnostics}\nTAIL:\n{tail}"
    )
}

/// Detects whether actor-mode output contains diagnostics worth model evaluation.
pub(super) fn shell_output_needs_model_evaluation(output: &crate::shell::ShellResult) -> bool {
    if !output.succeeded {
        return true;
    }
    let mut text = String::new();
    if let Some(stdout) = &output.stdout {
        text.push_str(stdout);
    }
    if let Some(stderr) = &output.stderr {
        text.push_str(stderr);
    }
    let lower = text.to_ascii_lowercase();
    [
        "error",
        "warning",
        "failed",
        "failure",
        "panic",
        "fatal",
        "exception",
        "assert",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Returns whether a command is a cache-safe read-only inspection.
pub(super) fn shell_is_inspection(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    [
        "cat ", "head ", "tail ", "sed ", "awk ", "grep ", "rg ", "ls", "find ", "tree ", "pwd",
    ]
    .iter()
    .any(|prefix| {
        lower.starts_with(prefix)
            || lower.contains(&format!("; {prefix}"))
            || lower.contains(&format!("&& {prefix}"))
            || lower.contains(&format!("| {prefix}"))
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

#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn short_failure_diagnostics_are_forwarded_exactly_without_an_evaluator() {
        let mut output = crate::shell::ShellResult {
            command: "cargo build".into(),
            working_directory: ".".into(),
            exit_code: 1,
            succeeded: false,
            duration_milliseconds: 1,
            stdout: Some("building".into()),
            stderr: Some("error: missing file\n  src/main.rs:3".into()),
            stdout_bytes: 8,
            stderr_bytes: 35,
            stdout_truncated: false,
            stderr_truncated: false,
        };
        let value: Value =
            serde_json::from_str(&inline_shell_result(&output, "saved-log").unwrap()).unwrap();
        assert_eq!(value["stderr"], output.stderr.as_deref().unwrap());
        assert_eq!(value["succeeded"], false);
        assert_eq!(value["exitCode"], 1);
        assert_eq!(value["artifactId"], "saved-log");
        output.stderr_truncated = true;
        assert!(inline_shell_result(&output, "saved-log").is_none());
        output.stderr_truncated = false;
        output.stderr = Some("한".repeat(1000));
        assert!(inline_shell_result(&output, "saved-log").is_none());
    }
}
