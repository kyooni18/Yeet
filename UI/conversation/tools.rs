//! Shared tool trace copy and semantic details, preserved from the working Web view.
use super::types::{ConversationIcon, DetailSection, ToolView};
use crate::model::{ConversationToolCall, ToolCallStatus};
use serde_json::Value;

fn record(value: &str) -> Option<Value> {
    serde_json::from_str::<Value>(value)
        .ok()
        .filter(Value::is_object)
}
fn field(value: Option<&Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value?
            .get(key)?
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    })
}
fn nonempty(value: Option<&str>) -> Option<String> {
    value.filter(|text| !text.is_empty()).map(str::to_owned)
}
fn compact_path(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let pieces: Vec<_> = normalized
        .split('/')
        .filter(|piece| !piece.is_empty())
        .collect();
    if pieces.len() > 2 {
        pieces[pieces.len() - 2..].join("/")
    } else {
        normalized
    }
}
fn read_paths(args: Option<&Value>) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(path) = args
        .and_then(|args| args.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
    {
        paths.push(path.to_owned());
    }
    for request in args
        .and_then(|args| args.get("requests"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(path) = request
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            && !paths.iter().any(|entry| entry == path)
        {
            paths.push(path.to_owned());
        }
    }
    paths
}
#[derive(Default)]
struct EditFile {
    path: String,
    operation: Option<String>,
    added: Option<usize>,
    removed: Option<usize>,
    preview: Vec<String>,
}
fn edit_files(tool: &ConversationToolCall, args: Option<&Value>) -> Vec<EditFile> {
    let argument_files: Vec<_> = args
        .and_then(|args| args.get("changes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| {
            let source = change.get("path")?.as_str()?.to_owned();
            if source.is_empty() {
                return None;
            }
            let file_op = change.get("fileOp");
            Some(EditFile {
                path: field(file_op, &["destination"]).unwrap_or(source),
                operation: Some(field(file_op, &["kind"]).unwrap_or_else(|| "update".into())),
                ..Default::default()
            })
        })
        .collect();
    let result = tool.result.as_deref().and_then(record);
    let mut files: Vec<_> = result
        .as_ref()
        .and_then(|result| result.get("files"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| {
            let path = field(Some(file), &["destination", "path"])?;
            let mut item = EditFile {
                path,
                operation: field(Some(file), &["operation"]),
                ..Default::default()
            };
            if let Some(hunks) = file
                .get("diff")
                .and_then(|diff| diff.get("hunks"))
                .and_then(Value::as_array)
            {
                let mut added = 0;
                let mut removed = 0;
                for line in hunks
                    .iter()
                    .filter_map(|hunk| hunk.get("lines").and_then(Value::as_array))
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    added += usize::from(line.starts_with('+'));
                    removed += usize::from(line.starts_with('-'));
                    if item.preview.len() < 8 && (line.starts_with('+') || line.starts_with('-')) {
                        item.preview.push(line.to_owned());
                    }
                }
                item.added = Some(added);
                item.removed = Some(removed);
            }
            Some(item)
        })
        .collect();
    if files.is_empty() {
        return argument_files;
    }
    for item in argument_files {
        if !files.iter().any(|file| file.path == item.path) {
            files.push(item);
        }
    }
    files
}
fn delta(file: &EditFile) -> Option<String> {
    match (file.added?, file.removed?) {
        (0, 0) => None,
        (added, 0) => Some(format!("+{added}")),
        (0, removed) => Some(format!("−{removed}")),
        (added, removed) => Some(format!("+{added} −{removed}")),
    }
}
fn summary(
    tool: &ConversationToolCall,
    args: Option<&Value>,
    files: &[EditFile],
) -> Option<String> {
    let fallback = || field(args, &["purpose"]).or_else(|| nonempty(tool.detail.as_deref()));
    match tool.name.as_str() {
        "run_shell" => field(args, &["command"]).or_else(|| nonempty(tool.detail.as_deref())),
        "apply_file_edits" => files
            .first()
            .map(|file| {
                let text = [Some(compact_path(&file.path)), delta(file)]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                if files.len() == 1 {
                    text
                } else {
                    format!("{text} · +{} files", files.len() - 1)
                }
            })
            .or_else(fallback),
        "read_file" | "read_files" => {
            let paths = read_paths(args);
            paths
                .first()
                .map(|path| {
                    if paths.len() > 1 {
                        format!("{} · +{} files", compact_path(path), paths.len() - 1)
                    } else {
                        compact_path(path)
                    }
                })
                .or_else(fallback)
        }
        "web_search" | "search_workspace" => {
            field(args, &["query", "q", "search", "text", "pattern"]).or_else(fallback)
        }
        "web_read" => field(args, &["url", "href", "ref_id", "refId"]).or_else(fallback),
        _ => fallback().or_else(|| {
            let raw = tool.arguments.trim();
            (!raw.is_empty()).then(|| {
                if raw.chars().count() > 100 {
                    format!("{}…", raw.chars().take(97).collect::<String>())
                } else {
                    raw.to_owned()
                }
            })
        }),
    }
}
fn section(title: &str, content: String, monospaced: bool) -> DetailSection {
    DetailSection {
        title: title.into(),
        content,
        monospaced,
        is_error: false,
        has_background: false,
    }
}
fn details(
    tool: &ConversationToolCall,
    args: Option<&Value>,
    files: &[EditFile],
) -> Vec<DetailSection> {
    let mut sections = Vec::new();
    match tool.name.as_str() {
        "run_shell" => {
            if let Some(command) = field(args, &["command"]) {
                sections.push(section("Command", command, true));
            }
            if let Some(cwd) = field(args, &["workingDirectory", "working_directory", "cwd"]) {
                sections.push(section("Working directory", cwd, true));
            }
            if let Some(result) = tool
                .result
                .as_deref()
                .map(str::trim)
                .filter(|result| !result.is_empty())
            {
                sections.push(section("Output", result.into(), true));
            }
        }
        "apply_file_edits" => {
            if !files.is_empty() {
                sections.push(section(
                    "Changed files",
                    files
                        .iter()
                        .map(|file| match delta(file) {
                            Some(delta) => format!("{}  {delta}", file.path),
                            None => file.operation.as_ref().map_or_else(
                                || file.path.clone(),
                                |op| format!("{}  [{op}]", file.path),
                            ),
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    true,
                ));
                let preview = files
                    .iter()
                    .filter(|file| !file.preview.is_empty())
                    .flat_map(|file| {
                        std::iter::once(format!("--- {}", compact_path(&file.path)))
                            .chain(file.preview.iter().cloned())
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !preview.is_empty() {
                    let mut item = section("Diff", preview, true);
                    item.has_background = true;
                    sections.push(item);
                }
            }
        }
        "read_file" | "read_files" => {
            let paths = read_paths(args);
            if !paths.is_empty() {
                sections.push(section(
                    if paths.len() == 1 { "File" } else { "Files" },
                    paths.join("\n"),
                    true,
                ));
            }
        }
        _ => {
            let raw = tool.arguments.trim();
            if !raw.is_empty() {
                sections.push(section(
                    "Input",
                    args.and_then(|args| serde_json::to_string_pretty(args).ok())
                        .unwrap_or_else(|| raw.into()),
                    true,
                ));
            }
            if let Some(result) = tool
                .result
                .as_deref()
                .map(str::trim)
                .filter(|result| !result.is_empty())
            {
                sections.push(section("Result", result.into(), tool.name == "shell_job"));
            }
        }
    }
    if let Some(error) = tool
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
    {
        let mut item = section("Error", error.into(), tool.name == "run_shell");
        item.is_error = true;
        sections.push(item);
    }
    sections
}
pub fn project_tool(tool: &ConversationToolCall) -> ToolView {
    use ConversationIcon::*;
    use ToolCallStatus::*;
    let active = matches!(tool.status, Preparing | Running | AwaitingPermission);
    let name = tool.name.trim();
    let title = match name {
        "web_search" => {
            if active {
                "Searching web"
            } else {
                "Web search"
            }
        }
        "web_read" => {
            if active {
                "Reading web page"
            } else {
                "Web page"
            }
        }
        "run_shell" => {
            if active {
                "Running command"
            } else {
                "Command"
            }
        }
        "shell_job" => {
            if active {
                "Checking command"
            } else {
                "Command"
            }
        }
        "read_file" | "read_files" => {
            if active {
                "Reading file"
            } else {
                "Read file"
            }
        }
        "apply_file_edits" => {
            if active {
                "Editing files"
            } else {
                "Edit files"
            }
        }
        "search_workspace" => {
            if active {
                "Searching code"
            } else {
                "Code search"
            }
        }
        "list_files" => {
            if active {
                "Reading file list"
            } else {
                "File list"
            }
        }
        "computer_use" | "desktop_control" => {
            if active {
                "Controlling screen"
            } else {
                "Screen control"
            }
        }
        _ if name.starts_with("mcp_") => {
            if active {
                "Running MCP"
            } else {
                "MCP"
            }
        }
        _ if name.starts_with("skill_") => {
            if active {
                "Running Skill"
            } else {
                "Skill"
            }
        }
        _ => "",
    };
    let title = if title.is_empty() {
        tool.label
            .clone()
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| {
                if name.is_empty() {
                    "Tool call".into()
                } else {
                    name.replace('_', " ")
                }
            })
    } else {
        title.into()
    };
    let icon = match name {
        "run_shell" | "shell_job" => Terminal,
        "apply_file_edits" => Edit,
        "read_file" | "read_files" => File,
        "search_workspace" => Search,
        "list_files" => Folder,
        "computer_use" | "desktop_control" => Screen,
        "web_search" | "web_read" => Web,
        _ if name.starts_with("mcp_") => Mcp,
        _ if name.starts_with("skill_") => Skill,
        _ => Tool,
    };
    let status_label = match tool.status {
        Completed => Some("✓"),
        Failed => Some("Failed"),
        TimedOut => Some("Timed out"),
        AwaitingPermission => Some("Awaiting approval"),
        Cancelled => Some("Cancelled"),
        Interrupted => Some("Interrupted"),
        Suppressed => Some("suppressed"),
        Preparing | Running => None,
    }
    .map(str::to_owned);
    let args = record(&tool.arguments);
    let files = edit_files(tool, args.as_ref());
    ToolView {
        raw_tool: tool.clone(),
        title,
        summary: summary(tool, args.as_ref(), &files),
        status_label,
        metadata: tool.duration_ms.map(|duration| {
            if duration < 1000 {
                format!("{duration} ms")
            } else {
                format!("{:.1} s", duration as f64 / 1000.0)
            }
        }),
        active,
        failed: matches!(tool.status, Failed | TimedOut),
        awaits_permission: matches!(tool.status, AwaitingPermission),
        icon,
        details: details(tool, args.as_ref(), &files),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edit_result_keeps_delta_preview_and_pending_argument_files() {
        let tool: ConversationToolCall = serde_json::from_value(serde_json::json!({
            "id":"edit", "name":"apply_file_edits", "status":"failed",
            "arguments":r#"{"changes":[{"path":"src/a.rs"},{"path":"src/b.rs"}]}"#,
            "result":r#"{"files":[{"path":"src/a.rs","diff":{"hunks":[{"lines":["+added","-removed"," context"]}]}}]}"#,
            "error":"cannot finish", "durationMs":1200
        })).unwrap();
        let view = project_tool(&tool);
        assert_eq!(view.summary.as_deref(), Some("src/a.rs +1 −1 · +1 files"));
        assert_eq!(view.metadata.as_deref(), Some("1.2 s"));
        assert!(view.failed);
        assert!(view.details[0].content.contains("src/b.rs  [update]"));
        assert_eq!(view.details[1].content, "--- src/a.rs\n+added\n-removed");
        assert!(view.details[1].has_background);
        assert!(view.details.last().unwrap().is_error);
    }
}
