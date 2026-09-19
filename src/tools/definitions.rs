//! Built-in tool schemas exposed to the model runtime.
//!
//! Keeping JSON schemas here prevents execution logic from becoming coupled to
//! long provider-facing descriptions and makes capability review straightforward.

use serde_json::json;

use crate::core::ToolDefinition;

use super::BuiltinCapabilityDescriptor;

pub(super) const BUILTIN_CAPABILITIES: &[BuiltinCapabilityDescriptor] = &[
    BuiltinCapabilityDescriptor {
        id: "builtin:file-read",
        name: "File Read",
        description: "Read one or several UTF-8 files with read_file. Outside-project paths trigger project approval unless unlimited or auto-approval mode is enabled. Reads return snapshot-safe line anchors for later edits.",
        // read_files is a hidden legacy alias so old restored calls inherit
        // the same disable/approval policy without re-exposing a second schema.
        tools: &["read_file", "read_files"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:file-write",
        name: "File Write",
        description: "Create, modify, move, or delete files with snapshot-safe apply_file_edits. Relative paths resolve from the session cwd; paths outside active context roots require approval unless unlimited or auto-approval mode is enabled. Existing files require fresh read coverage; the runtime binds the cached snapshot automatically.",
        tools: &["apply_file_edits"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:workspace-search",
        name: "Workspace Search",
        description: "Search workspace source with search_workspace. Matching is literal by default; narrow the path and use regex only when needed.",
        tools: &["search_workspace"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:shell",
        name: "Shell",
        description: "Run commands with run_shell. In sandboxed mode, mutating or outside-project execution asks for approval; unlimited mode runs without sandbox restrictions. Builds/tests/noisy commands can use actor mode. Use run_shell background=true for detached work, then shell_job action=wait for event-driven completion or fixed-rate monitoring without model polling.",
        tools: &["run_shell", "shell_job", "request_shell_permission"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:computer-use",
        name: "Computer Use",
        description: "Control native macOS applications with Codex's installed Computer Use runtime. Yeet launches it directly as a built-in capability; no MCP server needs to be configured or linked.",
        tools: &["computer_use", "computer_use_reset"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:artifacts",
        name: "Artifacts",
        description: "Inspect large tool results externalized as artifacts with read_artifact/search_artifact instead of replaying the original expensive command or read.",
        tools: &["artifact_info", "read_artifact", "search_artifact"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:document-read",
        name: "Document Read",
        description: "Extract readable content from documents without treating them as source code. Supports PDF, DOCX, spreadsheets, CSV/TSV, JSON, Markdown, and UTF-8 text, with large content stored as typed artifacts.",
        tools: &["read_document"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:data-analysis",
        name: "Data Analysis",
        description: "Analyze structured local data directly. Supports CSV, TSV, JSON arrays of objects, Excel workbooks, and ODS with summaries, value counts, grouped aggregates, and Pearson correlation.",
        tools: &["analyze_data"],
    },
    BuiltinCapabilityDescriptor {
        id: "builtin:sessions",
        name: "Session Management",
        description: "List Yeet sessions structurally and export a verified session archive without shell-based session discovery or deleting the active runtime state.",
        tools: &["list_sessions", "export_session"],
    },
];

/// Returns the built-in tool definitions that exist independently of optional web search.
pub(super) fn base_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            "find_capabilities",
            "Find up to 8 matching lazy Skills, MCPs, or Workers. Refine the query if needed. Built-in schemas come from search_tools; web_search is not activated here.",
            json!({"type":"object","properties":{"query":{"type":"string"}},"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "activate_capability",
            "Activate one exact Skill/MCP/Worker ID returned by find_capabilities. Never invent IDs; this does not enable built-ins or web_search.",
            json!({"type":"object","properties":{"capability":{"type":"string"}},"required":["capability"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "read_file",
            "Read one UTF-8 file or batch up to 8 file ranges with line/hash edit anchors and snapshots. Relative paths resolve from the session cwd; paths outside active context roots follow approval rules. Use path for one file or requests for a batch. Reuse covered ranges; refresh=true forces fresh contents/anchors.",
            json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string"},
                    "startLine":{"type":"integer","minimum":1},
                    "endLine":{"type":"integer","minimum":1},
                    "refresh":{"type":"boolean"},
                    "requests":{
                        "type":"array","minItems":1,"maxItems":8,
                        "items":{
                            "type":"object",
                            "properties":{
                                "path":{"type":"string"},
                                "startLine":{"type":"integer","minimum":1},
                                "endLine":{"type":"integer","minimum":1},
                                "refresh":{"type":"boolean"}
                            },
                            "required":["path"],
                            "additionalProperties":false
                        }
                    }
                },
                "oneOf":[{"required":["path"]},{"required":["requests"]}],
                "additionalProperties":false
            }),
        ),
        ToolDefinition::new(
            "search_workspace",
            "Search text from the session cwd or another active context root. Matching is literal unless regex=true; narrow path when possible.",
            json!({"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string"},"maxResults":{"type":"integer","minimum":1,"maximum":100},"caseSensitive":{"type":"boolean"},"regex":{"type":"boolean"}},"required":["query"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "read_document",
            "Extract local document content (PDF, DOCX, spreadsheets, CSV/TSV, JSON, Markdown, markup/config text). Full output is stored as a typed artifact for bounded follow-up reads.",
            json!({"type":"object","properties":{"path":{"type":"string"},"maxChars":{"type":"integer","minimum":1000,"maximum":64000}},"required":["path"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "analyze_data",
            "Analyze local CSV/TSV/JSON-array/Excel/ODS data without code or writes: describe, value_counts, group_by, or Pearson correlation.",
            json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string"},
                    "sheet":{"type":"string"},
                    "operation":{"type":"string","enum":["describe","value_counts","group_by","correlation"]},
                    "column":{"type":"string"},
                    "with":{"type":"string"},
                    "groupBy":{"type":"string"},
                    "valueColumn":{"type":"string"},
                    "aggregation":{"type":"string","enum":["count","sum","mean","min","max"]},
                    "maxGroups":{"type":"integer","minimum":1,"maximum":200}
                },
                "required":["path"],
                "additionalProperties":false
            }),
        ),
        ToolDefinition::new(
            "artifact_info",
            "Inspect artifact type, source, size, and parser/analysis metadata.",
            json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "read_artifact",
            "Read artifact lines (default 160, max 8192 chars). endLine past EOF is clamped safely. For source-read artifacts, line ranges address the directly readable anchored source view; exactArtifactId in the externalization envelope preserves raw JSON. If truncated by characters, repeat the same line range with offset=nextOffset.",
            json!({"type":"object","properties":{"id":{"type":"string"},"startLine":{"type":"integer","minimum":1},"endLine":{"type":"integer","minimum":1},"offset":{"type":"integer","minimum":0},"maxChars":{"type":"integer","minimum":1,"maximum":8192}},"required":["id"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "search_artifact",
            "Search an artifact for bounded excerpts; use read_artifact on matching lines for full text.",
            json!({"type":"object","properties":{"id":{"type":"string"},"query":{"type":"string"},"maxResults":{"type":"integer","minimum":1,"maximum":50}},"required":["id","query"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "list_sessions",
            "List current-workspace Yeet sessions from metadata, newest first. Current session is excluded by default; debateOnly filters to sessions with a debate topic.",
            json!({"type":"object","properties":{"debateOnly":{"type":"boolean"},"includeCurrent":{"type":"boolean"},"limit":{"type":"integer","minimum":1,"maximum":100}},"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "export_session",
            "Create and verify a clean tar archive of one Yeet session. Default destination is ~/.yeet/exports/<sessionId>.tar; deleteSource defaults false and never deletes the active session.",
            json!({"type":"object","properties":{"sessionId":{"type":"string"},"destination":{"type":"string"},"deleteSource":{"type":"boolean"}},"required":["sessionId"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "run_shell",
            "Run a shell command from the session cwd by default. workingDirectory may target any active context root; other locations follow approval rules. For builds/tests likely to outlive a normal tool turn, prefer background=true and then shell_job action=wait instead of repeated status checks. Background jobs default to a 4-hour execution window; set timeoutSeconds explicitly for other long jobs. Sandboxed commands still obey the workspace wall-time policy. Unlimited mode uses normal user access. mode=actor summarizes builds/tests/noisy output and stores the bounded captured log.",
            json!({"type":"object","properties":{"command":{"type":"string"},"purpose":{"type":"string"},"workingDirectory":{"type":"string"},"mode":{"type":"string","enum":["auto","direct","actor"]},"timeoutSeconds":{"type":"integer","minimum":1,"maximum":86400},"background":{"type":"boolean"}},"required":["command"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "shell_job",
            "Check, wait for, list, stop, or forget detached shell jobs. action=wait suspends the model-side tool call until the job terminates without model polling. Set reportEverySeconds only when periodic model wakeups are wanted; cadence stays anchored across repeated waits and interval wakeups include only bounded live stdout/stderr tails plus byte counts. Running is not success; reuse jobId instead of rerunning. Forget only completed jobs.",
            json!({"type":"object","properties":{"action":{"type":"string","enum":["check","wait","list","stop","forget"]},"jobId":{"type":"string"},"reportEverySeconds":{"type":"integer","minimum":1,"maximum":86400}},"required":["action"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "request_shell_permission",
            "Request one-time unrestricted permission for an exact command when sandbox access cannot be inferred. Auto-approval/unlimited mode resolves immediately.",
            json!({"type":"object","properties":{"command":{"type":"string"},"reason":{"type":"string"}},"required":["command","reason"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "computer_use",
            "Control native macOS apps through the installed Codex Computer Use runtime. The JavaScript session persists across calls. On the first call after start/reset, execute exactly one entry-point call such as `await cua.getState()` or `let app = await cua.getApp(\"App Name\")`, then read the returned runtime documentation before using further APIs. Prefer purpose-built APIs/connectors when available.",
            json!({
                "type":"object",
                "properties":{
                    "code":{"type":"string","description":"JavaScript to execute using Codex's initialized Computer Use runtime."},
                    "timeout_ms":{"type":"integer","minimum":1,"description":"Optional execution timeout in milliseconds. Codex defaults to 30000 when omitted."},
                    "title":{"type":"string","minLength":1,"maxLength":80,"description":"Short user-facing description of what the Computer Use action does."}
                },
                "required":["code"],
                "additionalProperties":false
            }),
        ),
        ToolDefinition::new(
            "computer_use_reset",
            "Reset Yeet's persistent Codex Computer Use JavaScript session. This discards JavaScript bindings but does not close apps or erase their state.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "apply_file_edits",
            "Apply atomic structured file edits. Relative paths resolve from the session cwd. Use JSON edit objects, never patch strings. replace/delete use range:{start,end}; insert uses at:{kind:start|end|before|after,line?}. The runtime binds the cached read snapshot and validates stale edits; reread the necessary range if it rejects an edit. Sandboxed existing files require read_file coverage; creates do not. Paths outside active context roots follow approval rules.",
            json!({
                "type":"object",
                "properties":{
                    "changes":{
                        "type":"array","minItems":1,
                        "items":{
                            "type":"object",
                            "properties":{
                                "path":{"type":"string"},
                                "edits":{
                                    "type":"array","minItems":1,
                                    "items":{
                                        "oneOf":[
                                            {"type":"object","properties":{"kind":{"const":"replace"},"range":{"type":"object","properties":{"start":{"type":"integer","minimum":1},"end":{"type":"integer","minimum":1}},"required":["start","end"],"additionalProperties":false},"text":{"type":"string"}},"required":["kind","range","text"],"additionalProperties":false},
                                            {"type":"object","properties":{"kind":{"const":"delete"},"range":{"type":"object","properties":{"start":{"type":"integer","minimum":1},"end":{"type":"integer","minimum":1}},"required":["start","end"],"additionalProperties":false}},"required":["kind","range"],"additionalProperties":false},
                                            {"type":"object","properties":{"kind":{"const":"insert"},"at":{"oneOf":[{"type":"object","properties":{"kind":{"const":"start"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"end"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"before"},"line":{"type":"integer","minimum":1}},"required":["kind","line"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"after"},"line":{"type":"integer","minimum":1}},"required":["kind","line"],"additionalProperties":false}]},"text":{"type":"string"}},"required":["kind","at","text"],"additionalProperties":false}
                                        ]
                                    }
                                },
                                "fileOp":{
                                    "oneOf":[
                                        {"type":"object","properties":{"kind":{"const":"create"},"text":{"type":"string"},"mode":{"type":"integer","minimum":0}},"required":["kind","text"],"additionalProperties":false},
                                        {"type":"object","properties":{"kind":{"const":"delete"}},"required":["kind"],"additionalProperties":false},
                                        {"type":"object","properties":{"kind":{"const":"move"},"destination":{"type":"string"}},"required":["kind","destination"],"additionalProperties":false}
                                    ]
                                }
                            },
                            "required":["path"],
                            "anyOf":[{"required":["edits"]},{"required":["fileOp"]}],
                            "additionalProperties":false
                        }
                    },
                    "diagnostics":{"type":"boolean"}
                },
                "required":["changes"],"additionalProperties":false
            }),
        ),
    ]
}

/// Returns whether a built-in tool is primarily useful for general document/data tasks.
pub fn is_general_builtin_tool(name: &str) -> bool {
    matches!(name, "read_document" | "analyze_data" | "artifact_info")
}

/// Returns whether a built-in tool belongs to the coding/workspace surface.
pub fn is_coding_builtin_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "search_workspace"
            | "run_shell"
            | "shell_job"
            | "request_shell_permission"
            | "apply_file_edits"
    )
}

/// Returns the optional live-web search schema.
pub(super) fn web_search_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "web_search",
        "Search the live web. Plan the search step before calling: when 2-4 complementary queries are already foreseeable, put them in one queries batch instead of spending later search-only model rounds. Keep result sets small, reuse returned sources, and search again only for a concrete new gap. Use backend=searxng for language/category/time filters or pagination.",
        json!({
            "type":"object",
            "properties":{
                "query":{"type":"string","description":"One narrow search query. Prefer queries when multiple complementary searches are already foreseeable."},
                "queries":{"type":"array","description":"Batch 2-4 complementary searches that are already inferable from the request or current evidence in one tool call; use later searches only for concrete new gaps.","items":{"type":"string"},"minItems":1,"maxItems":4},
                "maxResults":{"type":"integer","minimum":1,"maximum":8},
                "backend":{"type":"string","enum":["auto","agent-reach","searxng"]},
                "language":{"type":"string"},
                "category":{"type":"string"},
                "timeRange":{"type":"string","enum":["day","month","year"]},
                "safeSearch":{"type":"integer","minimum":0,"maximum":2},
                "page":{"type":"integer","minimum":1,"maximum":10}
            },
            "anyOf":[{"required":["query"]},{"required":["queries"]}],
            "additionalProperties":false
        }),
    )
}

/// Returns the bounded web-source reader schema.
pub(super) fn web_read_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "web_read",
        "Read one URL returned by web_search. Use original-source text for material claims. Reads return a cache-friendly preview of at most 4,000 characters and preserve larger fetched text as an artifact for targeted search/read follow-up. Duplicate URLs are skipped regardless of maxChars.",
        json!({
            "type":"object",
            "properties":{
                "url":{"type":"string","minLength":1},
                "maxChars":{"type":"integer","minimum":2000,"maximum":4000}
            },
            "required":["url"],
            "additionalProperties":false
        }),
    )
}

/// Native Yeet tools exported directly over MCP, excluding lazy extensions.
pub(crate) fn direct_mcp_tool_definitions() -> Vec<ToolDefinition> {
    let mut tools = base_tool_definitions()
        .into_iter()
        .filter(|tool| {
            !matches!(
                tool.name.as_str(),
                "find_capabilities"
                    | "activate_capability"
                    | "request_shell_permission"
                    | "deploy_agent"
            )
        })
        .collect::<Vec<_>>();
    let mut aliases = Vec::new();
    if let Some(tool) = tools
        .iter()
        .find(|tool| tool.name == "computer_use")
        .cloned()
    {
        let mut alias = tool;
        alias.name = "desktop_control".into();
        aliases.push(alias);
    }
    if let Some(tool) = tools
        .iter()
        .find(|tool| tool.name == "computer_use_reset")
        .cloned()
    {
        let mut alias = tool;
        alias.name = "desktop_control_reset".into();
        aliases.push(alias);
    }
    tools.extend(aliases);
    tools.push(web_search_tool_definition());
    tools.push(web_read_tool_definition());
    tools.extend(crate::memory::tool_definitions());
    tools
}
