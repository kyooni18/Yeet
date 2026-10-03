//! Active post-edit verification.
//!
//! `phase` only names the execution state. This module acts on it: after a
//! tool round that successfully changed the workspace while no check has
//! passed since, the coordinator either runs the project's fast check itself
//! (when the sandbox allows it without approval) and reports only errors that
//! were not present in its previous run, or appends a short note naming the
//! detected command. Both paths are capped per turn and stop during runaway
//! rollovers, so verification feedback can never drive its own loop.

use std::collections::BTreeSet;
use std::path::Path;

use super::*;

const MAX_ROUNDS_PER_TURN: usize = 3;
const MAX_REPORTED_ERRORS: usize = 12;
const MAX_LINE_CHARS: usize = 220;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ProjectChecks {
    /// Compile/type check that is cheap enough to run after every edit round.
    pub fast: Option<String>,
    pub test: Option<String>,
    pub lint: Option<String>,
}

impl ProjectChecks {
    pub(super) fn detect(root: &Path) -> Self {
        if root.join("Cargo.toml").is_file() {
            return Self {
                fast: Some("cargo check --all-targets --message-format=short".into()),
                test: Some("cargo test".into()),
                lint: Some("cargo clippy --all-targets".into()),
            };
        }
        if let Ok(text) = std::fs::read_to_string(root.join("package.json"))
            && let Ok(package) = serde_json::from_str::<Value>(&text)
        {
            let runner = if root.join("pnpm-lock.yaml").is_file() {
                "pnpm"
            } else if root.join("yarn.lock").is_file() {
                "yarn"
            } else if root.join("bun.lock").is_file() || root.join("bun.lockb").is_file() {
                "bun"
            } else {
                "npm"
            };
            let scripts = package.get("scripts").and_then(Value::as_object);
            let script = |names: &[&str]| {
                names.iter().find_map(|name| {
                    let body = scripts?.get(*name)?.as_str()?;
                    // npm init's placeholder test script only ever fails.
                    (!body.contains("no test specified")).then(|| format!("{runner} run {name}"))
                })
            };
            return Self {
                fast: script(&["typecheck", "type-check", "check-types", "tsc"]),
                test: script(&["test"]),
                lint: script(&["lint"]),
            };
        }
        if root.join("go.mod").is_file() {
            return Self {
                fast: Some("go build ./...".into()),
                test: Some("go test ./...".into()),
                lint: Some("go vet ./...".into()),
            };
        }
        Self::default()
    }

    /// Environment line for the session overlay, or `None` when nothing was found.
    pub(super) fn describe(&self) -> Option<String> {
        let parts: Vec<String> = [
            ("fast check", &self.fast),
            ("tests", &self.test),
            ("lint", &self.lint),
        ]
        .into_iter()
        .filter_map(|(label, command)| Some(format!("{label}: `{}`", command.as_deref()?)))
        .collect();
        (!parts.is_empty()).then(|| {
            format!(
                "Detected project checks (run from the workspace root): {}. After edits the coordinator may run the fast check automatically and report only new errors.",
                parts.join("; ")
            )
        })
    }
}

pub(super) struct VerificationMonitor {
    checks: ProjectChecks,
    enabled: bool,
    rounds: usize,
    auto_check_unavailable: bool,
    previous_errors: Option<BTreeSet<String>>,
}

pub(super) struct RoundFacts {
    pub mutated: bool,
    pub failed_mutation: bool,
    pub rollover: bool,
}

impl VerificationMonitor {
    pub(super) fn new(checks: ProjectChecks, enabled: bool) -> Self {
        Self {
            checks,
            enabled,
            rounds: 0,
            auto_check_unavailable: false,
            previous_errors: None,
        }
    }

    /// Decides whether this round needs verification feedback and, when the
    /// sandbox allows it, runs the fast check. Returns a request-only note.
    pub(super) fn after_round<F>(
        &mut self,
        registry: &mut ToolRegistry,
        evidence: &mut turn_state::TurnExecutionEvidence,
        round: RoundFacts,
        cancel: &AtomicBool,
        emit: &mut F,
    ) -> Option<String>
    where
        F: FnMut(AgentEvent),
    {
        if !self.enabled
            || !round.mutated
            || round.failed_mutation
            || round.rollover
            || evidence.verification_succeeded()
            || self.rounds >= MAX_ROUNDS_PER_TURN
        {
            return None;
        }
        let suggested = self
            .checks
            .fast
            .clone()
            .or_else(|| self.checks.test.clone())?;
        self.rounds += 1;
        let runnable = self
            .checks
            .fast
            .clone()
            .filter(|command| !self.auto_check_unavailable && registry.can_auto_check(command));
        let Some(command) = runnable else {
            return Some(format!(
                "Coordinator observation: the workspace changed and no check has passed since. Detected check: `{suggested}`."
            ));
        };

        let call = ToolCall {
            id: format!("auto-check-{}", self.rounds),
            name: "run_shell".into(),
            arguments: json!({"command": command, "purpose": "Coordinator post-edit check"}),
        };
        emit(AgentEvent::ToolExecutionStarted(call.clone()));
        let output = match registry.run_auto_check(&command, cancel) {
            Ok(Some(output)) => output,
            Ok(None) | Err(_) => {
                self.auto_check_unavailable = true;
                emit(AgentEvent::ToolExecutionFinished {
                    call,
                    succeeded: false,
                    result: "auto-check unavailable".into(),
                });
                return Some(format!(
                    "Coordinator observation: the workspace changed and no check has passed since. Detected check: `{suggested}`."
                ));
            }
        };
        let combined = format!(
            "{}\n{}",
            output.stdout.as_deref().unwrap_or_default(),
            output.stderr.as_deref().unwrap_or_default()
        );
        let summary = json!({"command": command, "succeeded": output.succeeded, "exitCode": output.exit_code}).to_string();
        emit(AgentEvent::ToolExecutionFinished {
            call: call.clone(),
            succeeded: output.succeeded,
            result: summary.clone(),
        });
        let validation_call = progress::is_validation_tool_result(&call, &summary);
        evidence.observe_tool(
            &call,
            &summary,
            output.succeeded,
            false,
            validation_call,
            false,
        );

        if output.succeeded {
            let resolved = self.previous_errors.take().map_or(0, |errors| errors.len());
            return Some(format!(
                "Coordinator auto-check after your edits: `{command}` passed{}. It does not run tests{}.",
                if resolved > 0 {
                    format!(" ({resolved} earlier errors resolved)")
                } else {
                    String::new()
                },
                self.checks
                    .test
                    .as_deref()
                    .map(|test| format!("; run `{test}` if behavior changed"))
                    .unwrap_or_default()
            ));
        }
        if output.exit_code == 127 {
            // The check's tool is not installed; do not retry it this turn.
            self.auto_check_unavailable = true;
        }
        let report = diff_errors(self.previous_errors.as_ref(), &combined);
        self.previous_errors = Some(report.current);
        let mut note = format!(
            "Coordinator auto-check after your edits: `{command}` failed (exit {}). ",
            output.exit_code
        );
        if report.new_lines.is_empty() {
            if report.unchanged > 0 {
                note.push_str(&format!(
                    "No new errors; {} errors from the previous auto-check remain (not repeated).",
                    report.unchanged
                ));
            } else {
                note.push_str("Output tail:\n");
                for line in tail(&combined, 8) {
                    note.push_str(&format!("  {line}\n"));
                }
            }
            return Some(note.trim_end().to_owned());
        }
        note.push_str(&format!("{} new errors", report.new_count));
        if report.resolved > 0 || report.unchanged > 0 {
            note.push_str(&format!(
                " ({} resolved, {} unchanged and not repeated)",
                report.resolved, report.unchanged
            ));
        }
        note.push_str(":\n");
        for line in &report.new_lines {
            note.push_str(&format!("  {line}\n"));
        }
        if report.new_count > report.new_lines.len() {
            note.push_str(&format!(
                "  … {} more\n",
                report.new_count - report.new_lines.len()
            ));
        }
        Some(note.trim_end().to_owned())
    }
}

#[derive(Debug)]
struct ErrorReport {
    current: BTreeSet<String>,
    new_lines: Vec<String>,
    new_count: usize,
    resolved: usize,
    unchanged: usize,
}

/// Compares error lines by a position-free key so that errors merely shifted
/// by an edit are not reported as new.
fn diff_errors(previous: Option<&BTreeSet<String>>, output: &str) -> ErrorReport {
    let empty = BTreeSet::new();
    let previous = previous.unwrap_or(&empty);
    let mut current = BTreeSet::new();
    let mut new_lines = Vec::new();
    let mut new_count = 0usize;
    for line in output.lines().map(str::trim) {
        if !is_error_line(line) {
            continue;
        }
        let key = error_key(line);
        if !current.insert(key.clone()) || previous.contains(&key) {
            continue;
        }
        new_count += 1;
        if new_lines.len() < MAX_REPORTED_ERRORS {
            new_lines.push(line.chars().take(MAX_LINE_CHARS).collect());
        }
    }
    let unchanged = current.intersection(previous).count();
    ErrorReport {
        resolved: previous.len() - unchanged,
        unchanged,
        current,
        new_lines,
        new_count,
    }
}

fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    (lower.contains("error") || lower.contains("panicked at"))
        && !lower.contains("could not compile")
        && !lower.contains("aborting due to")
        && !lower.starts_with("error: process didn't exit")
        && !lower.contains("warnings emitted")
}

fn error_key(line: &str) -> String {
    static POSITIONS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let positions = POSITIONS.get_or_init(|| {
        regex::Regex::new(r":\d+(:\d+)?|\(\d+,\d+\)|\bline \d+").expect("static regex")
    });
    positions.replace_all(line, "").into_owned()
}

fn tail(output: &str, lines: usize) -> Vec<String> {
    let all: Vec<&str> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|line| line.chars().take(MAX_LINE_CHARS).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_errors_absent_from_the_previous_check_are_new() {
        let first = "src/a.rs:3:5: error[E0063]: missing field `median`\nsrc/b.rs:9:1: error[E0425]: cannot find value `x`\nerror: could not compile `ledger` due to 2 previous errors\n";
        let report = diff_errors(None, first);
        assert_eq!(report.new_count, 2);

        // b.rs shifted by an edit, a.rs fixed, c.rs newly broken.
        let second = "src/b.rs:12:1: error[E0425]: cannot find value `x`\nsrc/c.rs:1:1: error[E0308]: mismatched types\n";
        let report = diff_errors(Some(&report.current), second);
        assert_eq!(
            report.new_lines,
            ["src/c.rs:1:1: error[E0308]: mismatched types"]
        );
        assert_eq!((report.resolved, report.unchanged), (1, 1));
    }

    #[test]
    fn verification_note_needs_a_fresh_mutation_and_is_capped_per_turn() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut registry = ToolRegistry::new_with_bridge_handle(
            crate::tools::BridgeHandle::lazy_for_workspace(root.clone()),
            root,
            crate::workers::WorkerRegistry::new(Vec::new()).unwrap(),
            crate::permission::PermissionBroker::default(),
        )
        .unwrap();
        // No fast check, so the monitor can only suggest; it must never run anything.
        let checks = ProjectChecks {
            fast: None,
            test: Some("cargo test".into()),
            lint: None,
        };
        let mut monitor = VerificationMonitor::new(checks, true);
        let mut evidence = turn_state::TurnExecutionEvidence::default();
        let cancel = AtomicBool::new(false);
        let mut events = 0usize;
        let mut round = |monitor: &mut VerificationMonitor, mutated| {
            monitor.after_round(
                &mut registry,
                &mut evidence,
                RoundFacts {
                    mutated,
                    failed_mutation: false,
                    rollover: false,
                },
                &cancel,
                &mut |_| events += 1,
            )
        };
        assert!(round(&mut monitor, false).is_none());
        for _ in 0..MAX_ROUNDS_PER_TURN {
            assert!(round(&mut monitor, true).unwrap().contains("`cargo test`"));
        }
        assert!(round(&mut monitor, true).is_none());
        assert_eq!(events, 0);
    }
}
