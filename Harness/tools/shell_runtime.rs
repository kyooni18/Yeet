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
        let requested_working_directory = object.get("workingDirectory").and_then(Value::as_str);
        let persisted_cwd = self
            .shell_cwd
            .clone()
            .filter(|_| requested_working_directory.is_none())
            .filter(|path| path.is_dir());
        let effective_cwd = match persisted_cwd {
            Some(path) => path,
            None => self
                .context
                .resolve_session_path(requested_working_directory.unwrap_or("."))?,
        };
        if !effective_cwd.is_dir() {
            bail!(
                "Invalid shell working directory: {}",
                effective_cwd.display()
            );
        }
        let context_root = self.context.context_root_for_path(&effective_cwd).cloned();
        let shell_root = context_root
            .clone()
            .unwrap_or_else(|| self.context.workspace_root.clone());
        let effective_working_directory = effective_cwd.to_string_lossy().into_owned();
        let policy = SandboxStore::new(&shell_root)?.load()?;
        let hard_confined = self.context.hard_access_root.is_some();
        let mut unrestricted =
            !hard_confined && (policy.mode == SandboxMode::Unlimited || has_permit);
        let mut allow_write = false;
        if restricted.is_some()
            && self
                .catalog
                .disabled_capabilities
                .contains("builtin:file-write")
        {
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
                if !self.context.request_approval(
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
            if !self
                .context
                .request_approval("shell", &command, operation, reason)?
            {
                bail!("User denied shell operation '{operation}'");
            }
            allow_write = true;
        }
        let cache_safe_inspection = restricted.is_none() && shell_is_inspection(&command);
        let requested_mode = object.get("mode").and_then(Value::as_str).unwrap_or("auto");
        if !matches!(requested_mode, "auto" | "direct" | "actor") {
            bail!("run_shell mode must be auto, direct, or actor");
        }
        let actor =
            requested_mode == "actor" || (requested_mode == "auto" && shell_actor_route(&command));
        let default_timeout = if background {
            DEFAULT_BACKGROUND_TIMEOUT_SECONDS
        } else {
            120
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
            self.evidence.workspace_write_generation =
                self.evidence.workspace_write_generation.wrapping_add(1);
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
        let wrapped = with_cwd_report(&command);
        let output = run_shell_cancellable(ShellExecutionRequest {
            command: &wrapped,
            workspace_root: &shell_root,
            working_directory: Some(&effective_working_directory),
            timeout_seconds: timeout,
            capture_bytes: if actor { 512 * 1024 } else { 64 * 1024 },
            allow_write,
            unrestricted,
            force_sandboxed: hard_confined,
            cancel: Some(cancel),
        });
        // A failed or timed-out command may already have changed files. Do not
        // keep cached workspace evidence merely because no ShellResult arrived.
        if output.is_err() && (allow_write || unrestricted) {
            self.evidence.workspace_write_generation =
                self.evidence.workspace_write_generation.wrapping_add(1);
            self.invalidate_workspace_cache();
        }
        let mut output = output?;
        output.command = command.clone();
        let previous_cwd = self.shell_cwd.clone();
        let cwd_note = self.track_shell_cwd(&mut output, &effective_cwd);
        if requested_working_directory.is_some() {
            self.shell_cwd = previous_cwd;
        }
        let shell_cwd = self
            .shell_cwd
            .as_deref()
            .unwrap_or(&self.context.working_directory)
            .display()
            .to_string();
        let result = self.shell_result(
            &command,
            output,
            actor,
            allow_write,
            restricted.is_some(),
            unrestricted,
            cache_safe_inspection,
        )?;
        let mut value: Value = serde_json::from_str(&result)?;
        value["workingDirectory"] = json!(effective_working_directory);
        value["shellCwd"] = json!(shell_cwd);
        if let Some(note) = cwd_note {
            value["cwdNote"] = json!(note);
        }
        Ok(value.to_string())
    }

    /// Records where the command left the shell. Only directories inside the
    /// session's context roots persist; anything else falls back to the
    /// session cwd so a stray `cd /` cannot widen later commands' scope.
    fn track_shell_cwd(
        &mut self,
        output: &mut crate::shell::ShellResult,
        started_in: &Path,
    ) -> Option<String> {
        let captured_bytes = output.stdout.as_ref().map_or(0, String::len);
        let reported = output.stdout.as_mut().and_then(take_cwd_report);
        let trailer_bytes = captured_bytes - output.stdout.as_ref().map_or(0, String::len);
        output.stdout_bytes = output.stdout_bytes.saturating_sub(trailer_bytes);
        let ended_in = PathBuf::from(reported?);
        let ended_in = ended_in.canonicalize().unwrap_or(ended_in);
        if ended_in == started_in {
            return None;
        }
        if ended_in.is_dir() && self.shell_may_start_in(&ended_in) {
            let note = ended_in.display().to_string();
            self.shell_cwd = Some(ended_in);
            Some(note)
        } else {
            self.shell_cwd = None;
            Some(format!(
                "{} is outside the session context; next command starts in {}",
                ended_in.display(),
                self.context.working_directory.display()
            ))
        }
    }

    /// Whether a later command could launch in `directory` without approval:
    /// inside a context root, and readable under that root's sandbox policy.
    fn shell_may_start_in(&self, directory: &Path) -> bool {
        let Some(root) = self.context.context_root_for_path(directory) else {
            return false;
        };
        let Ok(policy) = SandboxStore::new(root).and_then(|store| store.load()) else {
            return false;
        };
        if directory == root || policy.mode == SandboxMode::Unlimited {
            return true;
        }
        directory.strip_prefix(root).is_ok_and(|relative| {
            policy
                .workspace_read
                .allows(&relative.to_string_lossy().replace('\\', "/"))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn shell_result(
        &mut self,
        command: &str,
        output: crate::shell::ShellResult,
        actor: bool,
        allow_write: bool,
        restricted: bool,
        unrestricted: bool,
        cache_safe_inspection: bool,
    ) -> Result<String> {
        let command = command.to_owned();
        if allow_write || (unrestricted && !cache_safe_inspection) {
            if allow_write || restricted {
                self.evidence.workspace_write_generation =
                    self.evidence.workspace_write_generation.wrapping_add(1);
            }
            self.invalidate_workspace_cache();
        }
        self.shell_output_result(&command, output, actor)
    }

    /// Shows the model the real output: whole when it fits, otherwise a head
    /// and tail window with the full text kept as an artifact. Replacing large
    /// output with a keyword-matched summary hid results (a grep reported as
    /// "Command succeeded" plus an unrelated line), so the summary is gone and
    /// diagnostics are added only to failures whose middle was omitted.
    pub(super) fn shell_output_result(
        &mut self,
        command: &str,
        output: crate::shell::ShellResult,
        actor: bool,
    ) -> Result<String> {
        let stdout = output.stdout.clone().unwrap_or_default();
        let stderr = output.stderr.clone().unwrap_or_default();
        let complete = !output.stdout_truncated && !output.stderr_truncated;
        if complete && stdout.len() + stderr.len() <= SHELL_INLINE_OUTPUT_BYTES {
            let mut value = serde_json::to_value(&output)?;
            if actor {
                value["route"] = json!("actor");
            }
            return Ok(value.to_string());
        }
        let diagnostics = if output.succeeded {
            Vec::new()
        } else {
            shell_diagnostic_lines(&output, 8)
        };
        let mut value = json!({
            "command": command,
            "workingDirectory": output.working_directory,
            "exitCode": output.exit_code,
            "succeeded": output.succeeded,
            "durationMilliseconds": output.duration_milliseconds,
            "stdoutBytes": output.stdout_bytes,
            "stderrBytes": output.stderr_bytes,
            "stdoutTruncated": output.stdout_truncated,
            "stderrTruncated": output.stderr_truncated,
            "outputWindowed": true,
            "artifactComplete": complete,
            "stdout": output_window(&stdout, SHELL_STDOUT_WINDOW_BYTES),
            "stderr": output_window(&stderr, SHELL_STDERR_WINDOW_BYTES),
            "diagnostics": diagnostics,
        });
        if actor {
            value["route"] = json!("actor");
        }
        if self.artifacts_enabled {
            let raw = format!(
                "$ {command}\nEXIT: {}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}",
                output.exit_code
            );
            value["artifactId"] = json!(self.artifacts.store(&raw)?);
        }
        Ok(value.to_string())
    }
}

const SHELL_INLINE_OUTPUT_BYTES: usize = 16 * 1024;
const SHELL_STDOUT_WINDOW_BYTES: usize = 12 * 1024;
const SHELL_STDERR_WINDOW_BYTES: usize = 4 * 1024;

/// Keeps the first quarter and last three quarters of `max_bytes`, cut at
/// line boundaries where possible, and marks what was omitted in between.
pub(super) fn output_window(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut head_end = max_bytes / 4;
    while !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let head_end = text[..head_end]
        .rfind('\n')
        .filter(|index| *index >= head_end / 2)
        .map_or(head_end, |index| index + 1);
    let mut tail_start = text.len() - (max_bytes - max_bytes / 4);
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let tail_len = text.len() - tail_start;
    let tail_start = text[tail_start..]
        .find('\n')
        .filter(|index| *index < tail_len / 4)
        .map_or(tail_start, |index| tail_start + index + 1);
    let omitted = tail_start.saturating_sub(head_end);
    format!(
        "{}... [{omitted} bytes omitted] ...\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
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

const CWD_REPORT_MARKER: &str = "__YEET_SHELL_CWD__=";

/// Appends a trailer that prints the shell's final directory, preserving the
/// command's exit status. Commands that `exit` early simply report nothing.
fn with_cwd_report(command: &str) -> String {
    if cfg!(windows) {
        return command.to_owned();
    }
    format!(
        "{command}\n__yeet_status=$?\nprintf '\\n{CWD_REPORT_MARKER}%s\\n' \"$(pwd -P)\"\nexit $__yeet_status"
    )
}

/// Removes the cwd trailer from captured stdout and returns the directory.
fn take_cwd_report(stdout: &mut String) -> Option<String> {
    let start = stdout.rfind(CWD_REPORT_MARKER)?;
    let path = stdout[start + CWD_REPORT_MARKER.len()..]
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let cut = stdout[..start].strip_suffix('\n').map_or(start, str::len);
    stdout.truncate(cut);
    (!path.is_empty()).then_some(path)
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

impl ToolRegistry {
    /// Requests a one-time, exact-command shell permit from the user.
    pub(super) fn ask_user_tool(
        &mut self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let question = string_arg(object, "question")?.trim().to_owned();
        if question.is_empty() {
            bail!("ask_user requires a question");
        }
        let mut options: Vec<String> = object
            .get("options")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        options.dedup();
        options.truncate(3);
        let allow_custom = object
            .get("allowCustom")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            || options.is_empty();
        let answer = self.context.permission.ask(
            crate::model::UserQuestion {
                id: uuid::Uuid::new_v4().to_string(),
                question: question.clone(),
                options: options.clone(),
                allow_custom,
            },
            cancel,
        );
        Ok(match answer {
            Some(answer) => {
                let custom = !options.contains(&answer);
                json!({"question": question, "answer": answer, "custom": custom})
            }
            None => json!({
                "question": question,
                "answer": null,
                "dismissed": true,
                "note": "The user dismissed the question. Proceed with your best judgment or stop and explain."
            }),
        }
        .to_string())
    }

    pub(super) fn request_shell_permission_tool(
        &mut self,
        object: &Map<String, Value>,
    ) -> Result<String> {
        let command = string_arg(object, "command")?.trim().to_owned();
        let reason = string_arg(object, "reason")?.trim().to_owned();
        if command.is_empty() || reason.is_empty() {
            bail!("request_shell_permission requires command and reason");
        }
        let restricted = restricted_operation(&command);
        let operation = restricted
            .clone()
            .unwrap_or_else(|| "unrestricted shell access".into());
        if restricted.is_some()
            && self
                .catalog
                .disabled_capabilities
                .contains("builtin:file-write")
        {
            bail!(
                "File Write is disabled for this session; mutating shell commands cannot be permitted"
            );
        }
        let granted = self
            .context
            .request_approval("shell", &command, &operation, &reason)?;
        if granted {
            self.permitted_shell_commands.insert(command.clone());
        }
        Ok(json!({"granted": granted, "permissionRequired": true, "command": command, "operation": operation, "oneTime": true}).to_string())
    }
}
