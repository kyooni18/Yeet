//! Semantic projection of tool arguments and results into specialized details.
//!
//! Only what the tool recorded at execution time is used; nothing here reads the
//! filesystem or reconstructs data the backend did not return.
use super::types::*;
use crate::model::{ConversationToolCall, ToolCallStatus};
use serde_json::Value;

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}
fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key)?.as_u64()
}
fn index(value: &Value, key: &str) -> Option<usize> {
    number(value, key).and_then(|number| usize::try_from(number).ok())
}
fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}
/// The structured result: the execution-time payload, else the visible result
/// when it happens to be JSON (live progress, older transcripts).
fn payload(tool: &ConversationToolCall) -> Option<Value> {
    tool.raw_result
        .as_deref()
        .into_iter()
        .chain(tool.result.as_deref())
        .find_map(|raw| serde_json::from_str::<Value>(raw).ok())
        .filter(|value| value.is_object() || value.is_array())
}
/// A finished call whose only record is compact text (older transcripts): the
/// generic view is the truthful one, since no structure was captured.
fn has_text_result(tool: &ConversationToolCall) -> bool {
    tool.result
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty())
}
fn externalized(value: &Value) -> Option<Externalized> {
    flag(value, "externalized").then(|| Externalized {
        artifact_id: text(value, "artifactId"),
        bytes: number(value, "bytes"),
    })
}

pub(super) fn project(
    tool: &ConversationToolCall,
    args: Option<&Value>,
) -> (Vec<RichDetail>, Vec<DetailSection>) {
    let mut rich = match tool.name.as_str() {
        "apply_file_edits" => edit(tool, args),
        "run_shell" | "shell_job" => terminal(tool, args),
        "read_file" | "read_files" => source(tool, args),
        _ => return (Vec::new(), Vec::new()),
    };
    if rich.is_empty() {
        return (rich, Vec::new());
    }
    if let Some(error) = tool
        .error
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        rich.push(RichDetail::Notice {
            message: error.to_owned(),
            is_error: true,
        });
    }
    (rich, raw_sections(tool, args))
}

fn raw_sections(tool: &ConversationToolCall, args: Option<&Value>) -> Vec<DetailSection> {
    let section = |title: &str, content: String| DetailSection {
        title: title.into(),
        content,
        monospaced: true,
        is_error: false,
        has_background: false,
    };
    let mut sections = Vec::new();
    let arguments = tool.arguments.trim();
    if !arguments.is_empty() && arguments != "{}" {
        sections.push(section(
            "Input",
            args.and_then(|args| serde_json::to_string_pretty(args).ok())
                .unwrap_or_else(|| arguments.to_owned()),
        ));
    }
    if let Some(result) = tool
        .result
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        sections.push(section("Output", result.to_owned()));
    }
    if let Some(raw) = tool.raw_result.as_deref() {
        let pretty = serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .unwrap_or_else(|| raw.to_owned());
        sections.push(section("Raw result", pretty));
    }
    sections
}

// ---- apply_file_edits --------------------------------------------------

const OMITTED_MARKER: &str = " ... diff preview truncated ...";

fn diff_lines(hunk: &Value, old_start: usize, new_start: usize) -> Vec<DiffLine> {
    let raw: Vec<&str> = hunk
        .get("lines")
        .and_then(Value::as_array)
        .map(|lines| lines.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let truncated = flag(hunk, "truncated");
    let marker = truncated
        .then(|| raw.iter().position(|line| *line == OMITTED_MARKER))
        .flatten();
    let (mut old, mut new) = (old_start, new_start);
    let mut lines = Vec::with_capacity(raw.len());
    for (position, line) in raw.iter().enumerate() {
        if Some(position) == marker {
            // Lines after the dropped middle are numbered from the hunk's end.
            let tail = &raw[position + 1..];
            let old_tail = tail.iter().filter(|l| !l.starts_with('+')).count();
            let new_tail = tail.iter().filter(|l| !l.starts_with('-')).count();
            if let Some(end) = index(hunk, "oldEnd") {
                old = (end + 1).saturating_sub(old_tail);
            }
            if let Some(end) = index(hunk, "newEnd") {
                new = (end + 1).saturating_sub(new_tail);
            }
            lines.push(DiffLine {
                kind: DiffLineKind::Omitted,
                old: None,
                new: None,
                text: String::new(),
            });
            continue;
        }
        let (kind, body) = match line.chars().next() {
            Some('+') => (DiffLineKind::Added, &line[1..]),
            Some('-') => (DiffLineKind::Removed, &line[1..]),
            Some(' ') => (DiffLineKind::Context, &line[1..]),
            _ => (DiffLineKind::Context, *line),
        };
        let (old_number, new_number) = match kind {
            DiffLineKind::Added => (None, Some(new)),
            DiffLineKind::Removed => (Some(old), None),
            _ => (Some(old), Some(new)),
        };
        old += usize::from(kind != DiffLineKind::Added);
        new += usize::from(kind != DiffLineKind::Removed);
        lines.push(DiffLine {
            kind,
            old: old_number,
            new: new_number,
            text: body.to_owned(),
        });
    }
    lines
}

fn diff_file(file: &Value) -> Option<DiffFile> {
    let source = text(file, "path");
    let destination = text(file, "destination");
    let path = destination.clone().or(source.clone())?;
    let operation = text(file, "operation").unwrap_or_else(|| "update".into());
    let hunks_json = file
        .get("diff")
        .and_then(|diff| diff.get("hunks"))
        .and_then(Value::as_array);
    let hunks: Vec<DiffHunk> = hunks_json
        .into_iter()
        .flatten()
        .map(|hunk| {
            let old_start = index(hunk, "oldStart").unwrap_or(1);
            let new_start = index(hunk, "newStart").unwrap_or(1);
            DiffHunk {
                old_start,
                old_end: index(hunk, "oldEnd"),
                new_start,
                new_end: index(hunk, "newEnd"),
                lines: diff_lines(hunk, old_start, new_start),
                truncated: flag(hunk, "truncated"),
            }
        })
        .collect();
    Some(DiffFile {
        previous_path: destination.and(source).filter(|source| *source != path),
        truncated: hunks.iter().any(|hunk| hunk.truncated),
        has_diff: hunks_json.is_some(),
        warnings: file
            .get("warnings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|warning| warning.as_str().map(str::to_owned))
            .collect(),
        path,
        operation,
        hunks,
    })
}

fn edit(tool: &ConversationToolCall, args: Option<&Value>) -> Vec<RichDetail> {
    let result = payload(tool).filter(Value::is_object);
    if result.is_none() && has_text_result(tool) {
        return Vec::new();
    }
    let mut detail = DiffDetail::default();
    if let Some(result) = &result {
        detail.externalized = externalized(result);
        detail.files = result
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(diff_file)
            .collect();
    }
    // Calls with no (or only externalized) result still show what they targeted.
    for change in args
        .and_then(|args| args.get("changes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(source) = text(change, "path") else {
            continue;
        };
        let file_op = change.get("fileOp");
        let destination = file_op.and_then(|op| text(op, "destination"));
        let path = destination.clone().unwrap_or_else(|| source.clone());
        if detail.files.iter().any(|file| file.path == path) {
            continue;
        }
        detail.files.push(DiffFile {
            previous_path: destination.map(|_| source),
            operation: file_op
                .and_then(|op| text(op, "kind"))
                .unwrap_or_else(|| "update".into()),
            path,
            ..Default::default()
        });
    }
    let mut rich = Vec::new();
    let diagnostics: Vec<DiagnosticItem> = result
        .as_ref()
        .and_then(|result| result.get("diagnostics"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(DiagnosticItem {
                severity: text(item, "severity").unwrap_or_else(|| "info".into()),
                path: text(item, "path"),
                line: index(item, "line"),
                column: index(item, "column"),
                message: text(item, "message")?,
            })
        })
        .collect();
    if !detail.files.is_empty() || detail.externalized.is_some() {
        rich.push(RichDetail::Diff(detail));
    }
    if !diagnostics.is_empty() {
        rich.push(RichDetail::Diagnostics { items: diagnostics });
    }
    rich
}

// ---- run_shell / shell_job ---------------------------------------------

fn stream(value: &Value, name: &str, tail: Option<&Value>) -> Option<OutputStream> {
    let truncated = flag(value, &format!("{name}Truncated"));
    let bytes = number(value, &format!("{name}Bytes"));
    if let Some(output) = text(value, name) {
        return Some(OutputStream {
            text: output,
            bytes,
            truncated,
            tail: false,
        });
    }
    let tail_text = tail.and_then(|tail| text(tail, &format!("{name}Tail")))?;
    Some(OutputStream {
        text: tail_text,
        bytes: tail
            .and_then(|tail| number(tail, &format!("{name}Bytes")))
            .or(bytes),
        truncated,
        tail: true,
    })
}

fn terminal(tool: &ConversationToolCall, args: Option<&Value>) -> Vec<RichDetail> {
    let result = payload(tool).filter(Value::is_object);
    let value = result.clone().unwrap_or(Value::Null);
    let progress = value
        .get("progress")
        .filter(|progress| progress.is_object());
    let exit_code = value.get("exitCode").and_then(Value::as_i64);
    let status = text(&value, "status");
    let handed_off = flag(&value, "outputHandedOff")
        || (status.as_deref() == Some("running") && value.get("exitCode").is_none());
    let state = match tool.status {
        ToolCallStatus::Cancelled => TerminalState::Cancelled,
        ToolCallStatus::Interrupted => TerminalState::Interrupted,
        ToolCallStatus::TimedOut => TerminalState::TimedOut,
        ToolCallStatus::AwaitingPermission | ToolCallStatus::Preparing => TerminalState::Waiting,
        ToolCallStatus::Running => {
            if flag(&value, "outputHandedOff") {
                TerminalState::BackgroundJob
            } else {
                TerminalState::Running
            }
        }
        ToolCallStatus::Failed | ToolCallStatus::Completed | ToolCallStatus::Suppressed => {
            match (status.as_deref(), exit_code) {
                _ if handed_off => TerminalState::BackgroundJob,
                (Some("cancelled"), _) => TerminalState::Cancelled,
                (Some("failed"), _) => TerminalState::Failed,
                (_, Some(code)) if code != 0 => TerminalState::Failed,
                _ if matches!(tool.status, ToolCallStatus::Failed) => TerminalState::Failed,
                _ => TerminalState::Succeeded,
            }
        }
    };
    let command = args
        .and_then(|args| text(args, "command"))
        .or_else(|| text(&value, "command"));
    let working_directory = text(&value, "workingDirectory").or_else(|| {
        args.and_then(|args| {
            text(args, "workingDirectory")
                .or_else(|| text(args, "working_directory"))
                .or_else(|| text(args, "cwd"))
        })
    });
    let stdout = stream(&value, "stdout", progress);
    let stderr = stream(&value, "stderr", progress);
    // A text-only result carries no stream information; show it unlabelled.
    let combined = (result.is_none() && stdout.is_none() && stderr.is_none())
        .then(|| tool.result.as_deref())
        .flatten()
        .map(str::trim_end)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    if command.is_none() && stdout.is_none() && stderr.is_none() && combined.is_none() {
        // shell_job calls with nothing recorded yet (or pure status payloads).
        if result.is_none() {
            return Vec::new();
        }
    }
    let duration_ms = number(&value, "durationMilliseconds")
        .or_else(|| number(&value, "elapsedMilliseconds"))
        .or(tool.duration_ms);
    vec![RichDetail::Terminal(TerminalDetail {
        working_directory,
        command,
        state,
        exit_code,
        duration_ms,
        stdout,
        stderr,
        combined,
        job_id: text(&value, "jobId").or_else(|| tool.job_id.clone()),
        artifact_id: text(&value, "artifactId"),
        windowed: flag(&value, "outputWindowed") || flag(&value, "outputHandedOff"),
    })]
}

// ---- read_file / read_files --------------------------------------------

/// Splits `N:hash|text` (or `N|text`) into the line number and the source text.
/// The hash is edit-anchor metadata, never part of the source.
fn parse_anchor(line: &str) -> Option<(usize, &str)> {
    let (head, text) = line.split_once('|')?;
    let (number, hash) = head.split_once(':').unwrap_or((head, ""));
    (!number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    .then(|| number.parse().ok().map(|number| (number, text)))
    .flatten()
}

fn anchored_lines(content: &str, start: Option<usize>) -> Vec<SourceLine> {
    let mut next = start.unwrap_or(1);
    content
        .split('\n')
        .map(|line| {
            let (number, text) = parse_anchor(line).unwrap_or((next, line));
            next = number + 1;
            SourceLine {
                number,
                text: text.to_owned(),
            }
        })
        .collect()
}

fn source_file(item: &Value) -> Option<SourceFile> {
    let path = text(item, "path");
    if let Some(error) = text(item, "error") {
        return Some(SourceFile {
            path: path.unwrap_or_default(),
            start_line: None,
            end_line: None,
            total_lines: None,
            next_start_line: None,
            availability: SourceAvailability::Failed,
            lines: Vec::new(),
            artifact_id: None,
            error: Some(error),
        });
    }
    let start_line = index(item, "startLine");
    let (content, availability) = match (text(item, "lines"), externalized(item)) {
        (Some(lines), _) => (Some(lines), SourceAvailability::Lines),
        (None, Some(_)) => (text(item, "preview"), SourceAvailability::Preview),
        (None, None) => (None, SourceAvailability::Pending),
    };
    let mut lines = content
        .as_deref()
        .map(|content| anchored_lines(content, start_line))
        .unwrap_or_default();
    // A bounded preview may end mid-line; keep only complete lines.
    if availability == SourceAvailability::Preview
        && lines.len() > 1
        && !content
            .as_deref()
            .is_some_and(|content| content.ends_with('\n'))
    {
        lines.pop();
    }
    let availability = if lines.is_empty() && availability != SourceAvailability::Pending {
        SourceAvailability::Pending
    } else {
        availability
    };
    Some(SourceFile {
        path: path?,
        start_line,
        end_line: index(item, "endLine"),
        total_lines: index(item, "totalLines"),
        next_start_line: index(item, "nextStartLine"),
        availability,
        lines,
        artifact_id: text(item, "artifactId"),
        error: None,
    })
}

fn source(tool: &ConversationToolCall, args: Option<&Value>) -> Vec<RichDetail> {
    if payload(tool).is_none() && has_text_result(tool) {
        return Vec::new();
    }
    let mut files: Vec<SourceFile> = match payload(tool) {
        Some(Value::Array(items)) => items.iter().filter_map(source_file).collect(),
        Some(value) => source_file(&value).into_iter().collect(),
        None => Vec::new(),
    };
    if files.is_empty() {
        // Requested files with no recorded result: say what was asked, show no lines.
        let requested = args
            .into_iter()
            .chain(
                args.and_then(|args| args.get("requests"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            )
            .filter_map(|item| {
                Some(SourceFile {
                    path: text(item, "path")?,
                    start_line: index(item, "startLine"),
                    end_line: index(item, "endLine"),
                    total_lines: None,
                    next_start_line: None,
                    availability: SourceAvailability::Pending,
                    lines: Vec::new(),
                    artifact_id: None,
                    error: None,
                })
            });
        files.extend(requested);
    }
    if files.is_empty() {
        Vec::new()
    } else {
        vec![RichDetail::Source(SourceDetail { files })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(
        name: &str,
        arguments: Value,
        status: &str,
        raw: Option<Value>,
        result: Option<&str>,
    ) -> ConversationToolCall {
        let mut value = json!({
            "id": "t", "name": name, "status": status,
            "arguments": arguments.to_string(),
        });
        if let Some(raw) = raw {
            value["rawResult"] = json!(raw.to_string());
        }
        if let Some(result) = result {
            value["result"] = json!(result);
        }
        serde_json::from_value(value).unwrap()
    }
    fn project_for(tool: &ConversationToolCall) -> Vec<RichDetail> {
        let args = serde_json::from_str::<Value>(&tool.arguments).ok();
        project(tool, args.as_ref()).0
    }

    #[test]
    fn edits_keep_hunks_numbering_truncation_and_diagnostics() {
        let tool = call(
            "apply_file_edits",
            json!({"changes": [{"path": "src/a.rs"}, {"path": "old.rs", "fileOp": {"kind": "rename", "destination": "new.rs"}}]}),
            "completed",
            Some(json!({
                "files": [
                    {"path": "src/a.rs", "operation": "update", "diff": {"path": "src/a.rs", "hunks": [
                        {"oldStart": 10, "oldEnd": 13, "newStart": 10, "newEnd": 13, "truncated": false,
                         "lines": [" ctx", "-    old", "+    new", " tail"]},
                        {"oldStart": 40, "oldEnd": 200, "newStart": 41, "newEnd": 201, "truncated": true,
                         "lines": [" a", "-b", "+c", " ... diff preview truncated ...", " y", "-z", "+w", " end"]}
                    ]}},
                    {"path": "old.rs", "destination": "new.rs", "operation": "rename", "diff": {"path": "new.rs", "hunks": []}}
                ],
                "diagnostics": [{"path": "src/a.rs", "severity": "warning", "message": "unused", "line": 12, "column": 3}]
            })),
            None,
        );
        let rich = project_for(&tool);
        let RichDetail::Diff(diff) = &rich[0] else {
            panic!("{rich:?}")
        };
        let file = &diff.files[0];
        assert_eq!((file.added(), file.removed(), file.truncated), (3, 3, true));
        let first = &file.hunks[0].lines;
        assert_eq!((first[0].old, first[0].new), (Some(10), Some(10)));
        assert_eq!((first[1].old, first[1].new), (Some(11), None));
        assert_eq!((first[2].old, first[2].new), (None, Some(11)));
        assert_eq!((first[3].old, first[3].new), (Some(12), Some(12)));
        // After the omitted middle, numbering resumes from the hunk's real end.
        let second = &file.hunks[1].lines;
        assert_eq!(second[3].kind, DiffLineKind::Omitted);
        assert_eq!((second[4].old, second[4].new), (Some(198), Some(199)));
        assert_eq!((second[7].old, second[7].new), (Some(200), Some(201)));
        let renamed = &diff.files[1];
        assert_eq!(
            (renamed.path.as_str(), renamed.previous_path.as_deref()),
            ("new.rs", Some("old.rs"))
        );
        assert!(renamed.has_diff && renamed.hunks.is_empty());
        assert!(matches!(&rich[1], RichDetail::Diagnostics { items } if items[0].line == Some(12)));
    }

    #[test]
    fn externalized_pending_and_text_only_edits_stay_truthful() {
        let args = json!({"changes": [{"path": "big.rs"}]});
        let external = call(
            "apply_file_edits",
            args.clone(),
            "completed",
            Some(
                json!({"artifactId": "art-1", "externalized": true, "bytes": 90000, "preview": "{\"files\":[{\"pa"}),
            ),
            None,
        );
        let rich = project_for(&external);
        let RichDetail::Diff(diff) = &rich[0] else {
            panic!()
        };
        assert_eq!(
            diff.externalized.as_ref().unwrap().artifact_id.as_deref(),
            Some("art-1")
        );
        assert!(!diff.files[0].has_diff);
        let mut failed = call("apply_file_edits", args.clone(), "failed", None, None);
        failed.error = Some("stale snapshot".into());
        let rich = project_for(&failed);
        assert!(matches!(&rich[0], RichDetail::Diff(diff) if !diff.files[0].has_diff));
        assert!(matches!(
            rich.last(),
            Some(RichDetail::Notice { is_error: true, .. })
        ));
        // Older transcripts only kept compact text: no invented structure.
        let legacy = call(
            "apply_file_edits",
            args,
            "completed",
            None,
            Some("update big.rs\n@@ -1 +1 @@\n-a\n+b"),
        );
        assert!(project_for(&legacy).is_empty());
    }

    #[test]
    fn shell_states_separate_failure_from_stderr_and_report_live_and_handoff() {
        let args = json!({"command": "cargo test", "workingDirectory": "/w"});
        let ok = call(
            "run_shell",
            args.clone(),
            "completed",
            Some(json!({
                "command": "cargo test", "workingDirectory": "/w/sub", "exitCode": 0, "succeeded": true,
                "durationMilliseconds": 1200, "stdoutBytes": 3, "stderrBytes": 9,
                "stdout": "ok\n", "stderr": "warning: x\n"
            })),
            None,
        );
        let RichDetail::Terminal(terminal) = &project_for(&ok)[0] else {
            panic!()
        };
        assert_eq!(terminal.state, TerminalState::Succeeded);
        assert_eq!(terminal.working_directory.as_deref(), Some("/w/sub"));
        assert_eq!(terminal.stderr.as_ref().unwrap().text, "warning: x\n");
        assert_eq!(terminal.duration_ms, Some(1200));

        let failed = call(
            "run_shell",
            args.clone(),
            "completed",
            Some(json!({
                "exitCode": 2, "succeeded": false, "stdout": null, "stderr": "boom\n"
            })),
            None,
        );
        let RichDetail::Terminal(terminal) = &project_for(&failed)[0] else {
            panic!()
        };
        assert_eq!(
            (terminal.state, terminal.exit_code),
            (TerminalState::Failed, Some(2))
        );

        let live = call("run_shell", args.clone(), "running", None, Some(&json!({
            "status": "running", "command": "cargo test", "elapsedMilliseconds": 3400,
            "progress": {"stdoutBytes": 11, "stderrBytes": 0, "stdoutTail": "compiling\n", "stderrTail": null}
        }).to_string()));
        let RichDetail::Terminal(terminal) = &project_for(&live)[0] else {
            panic!()
        };
        assert_eq!(terminal.state, TerminalState::Running);
        assert!(terminal.stdout.as_ref().unwrap().tail && terminal.stderr.is_none());

        let handoff = call(
            "run_shell",
            args.clone(),
            "completed",
            Some(json!({
                "jobId": "job-7", "status": "running", "outputHandedOff": true, "artifactId": "art-9",
                "progress": {"stdoutBytes": 4, "stderrBytes": 0, "stdoutTail": "so far", "stderrTail": null}
            })),
            None,
        );
        let RichDetail::Terminal(terminal) = &project_for(&handoff)[0] else {
            panic!()
        };
        assert_eq!(terminal.state, TerminalState::BackgroundJob);
        assert_eq!(terminal.job_id.as_deref(), Some("job-7"));
        assert!(terminal.windowed);

        for (status, state) in [
            ("cancelled", TerminalState::Cancelled),
            ("timed_out", TerminalState::TimedOut),
        ] {
            let tool = call("run_shell", args.clone(), status, None, None);
            let RichDetail::Terminal(terminal) = &project_for(&tool)[0] else {
                panic!()
            };
            assert_eq!(terminal.state, state);
            assert!(terminal.stdout.is_none() && terminal.combined.is_none());
        }
        // Text-only results carry no stream information.
        let legacy = call(
            "run_shell",
            args,
            "completed",
            None,
            Some("Command succeeded. hi"),
        );
        let RichDetail::Terminal(terminal) = &project_for(&legacy)[0] else {
            panic!()
        };
        assert_eq!(terminal.combined.as_deref(), Some("Command succeeded. hi"));
        assert!(terminal.stdout.is_none());
    }

    #[test]
    fn reads_parse_anchors_use_the_effective_range_and_flag_previews() {
        let single = call(
            "read_file",
            json!({"path": "src/main.rs", "startLine": 1, "endLine": 400}),
            "completed",
            Some(json!({
                "path": "src/main.rs", "snapshot": "s", "startLine": 3, "endLine": 6, "totalLines": 9,
                "lines": "3:ab12|fn main() {\n4:cd34|\n5:ef56|\tlet x = \"é|漢\";\n6:0a0a|}", "nextStartLine": 7
            })),
            None,
        );
        let RichDetail::Source(source) = &project_for(&single)[0] else {
            panic!()
        };
        let file = &source.files[0];
        assert_eq!(
            (file.start_line, file.end_line, file.total_lines),
            (Some(3), Some(6), Some(9))
        );
        assert_eq!(file.lines.len(), 4);
        assert_eq!(file.lines[0].text, "fn main() {");
        assert_eq!(file.lines[1].text, "");
        assert_eq!(file.lines[2].text, "\tlet x = \"é|漢\";");
        assert_eq!(file.next_start_line, Some(7));

        let batch = call(
            "read_file",
            json!({"requests": [{"path": "a"}, {"path": "b"}, {"path": "c"}]}),
            "completed",
            Some(json!([
                {"path": "a", "startLine": 1, "endLine": 1, "totalLines": 1, "lines": "1:aa|only"},
                {"error": "no such file", "path": "b"},
                {"path": "c", "externalized": true, "artifactId": "art-3", "startLine": 1, "endLine": 900,
                 "totalLines": 900, "preview": "1:aa|one\n2:bb|two\n3:cc|thr"}
            ])),
            None,
        );
        let RichDetail::Source(source) = &project_for(&batch)[0] else {
            panic!()
        };
        assert_eq!(source.files.len(), 3);
        assert_eq!(source.files[1].availability, SourceAvailability::Failed);
        let preview = &source.files[2];
        assert_eq!(preview.availability, SourceAvailability::Preview);
        // The cut-off final line is not presented as source.
        assert_eq!(
            preview.lines.iter().map(|l| l.number).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(preview.artifact_id.as_deref(), Some("art-3"));
    }

    #[test]
    fn tools_without_a_specialized_view_keep_the_generic_fallback() {
        let tool = call(
            "web_search",
            json!({"query": "x"}),
            "completed",
            None,
            Some("hits"),
        );
        assert!(project_for(&tool).is_empty());
        let view = super::super::project_tool(&tool);
        assert!(view.rich.is_empty() && view.details.iter().any(|d| d.title == "Result"));
    }
}
