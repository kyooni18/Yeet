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
        id: "builtin:code-symbols",
        name: "Code Symbols",
        description: "Navigate Rust/TypeScript/JavaScript code by declaration: outline lists a file's declarations with line numbers; find_symbol finds definitions and identifier references by exact name (best-effort, not type-aware).",
        tools: &["outline", "find_symbol"],
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
            "Read one UTF-8 file or up to 8 ranges with requests as numbered lines. Use inclusive startLine/endLine or startLine/lineCount. Reads always use current contents; nextStartLine continues this page and firstUnreadLine identifies earlier gaps. Relative paths resolve from the cwd.",
            native_read_file_schema(),
        ),
        ToolDefinition::new(
            "search_workspace",
            "Search file contents under the cwd. Literal unless regex=true.",
            json!({"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string"},"maxResults":{"type":"integer","minimum":1,"maximum":100},"caseSensitive":{"type":"boolean"},"regex":{"type":"boolean"}},"required":["query"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "outline",
            "List one Rust/TypeScript/JavaScript file's declarations (items and impl/class members) as line: signature, without bodies. Other languages return no symbols.",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "find_symbol",
            "Find definitions and references of one identifier across Rust/TypeScript/JavaScript sources under path (default: session cwd). Exact-name, syntax-based; same-named items in other modules are not distinguished.",
            json!({"type":"object","properties":{"name":{"type":"string"},"path":{"type":"string"}},"required":["name"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "read_document",
            "Extract local document content (PDF, DOCX, spreadsheets, CSV/TSV, JSON, Markdown, markup/config text). Full output is stored as a typed artifact for bounded follow-up reads.",
            json!({"type":"object","properties":{"path":{"type":"string"},"maxChars":{"type":"integer","minimum":1000,"maximum":16000}},"required":["path"],"additionalProperties":false}),
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
            "Read lines from a stored artifact, such as truncated tool output.",
            json!({"type":"object","properties":{"id":{"type":"string"},"startLine":{"type":"integer","minimum":1},"endLine":{"type":"integer","minimum":1},"offset":{"type":"integer","minimum":0},"maxChars":{"type":"integer","minimum":1,"maximum":8192}},"required":["id"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "search_artifact",
            "Search an artifact for bounded excerpts; use read_artifact on matching lines for full text.",
            json!({"type":"object","properties":{"id":{"type":"string"},"query":{"type":"string"},"maxResults":{"type":"integer","minimum":1,"maximum":50}},"required":["id","query"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "run_shell",
            "Run exactly one shell command or executable with literal args (no shell). Shell defaults to clean non-interactive initialization; loadConfig/login are opt-ins. Direct execution retains sandbox checks. Only cwd persists, not environment variables or functions; workingDirectory selects this call's directory. yieldSeconds (default 120) ends the foreground wait while the process keeps running; timeoutSeconds is a hard deadline. Completed and running output is retained under jobId; use shell_job action=output to page it, or action=wait for event-driven completion. Inspect capture completeness and retention limits. mode controls output routing, not how the process launches.",
            shell_execution_schema(),
        ),
        ToolDefinition::new(
            "shell_job",
            "Inspect, wait for, list, stop, or forget running and retained completed jobs. output returns decoded text pages by byte offset; format=events preserves raw base64 events. Continue with nextOffset and inspect hasMore/captureComplete. For Unix PTY jobs, input sends text, resize changes terminal dimensions, and interrupt sends Ctrl-C. A stop request is distinct from confirmed process exit.",
            shell_job_schema(),
        ),
        ToolDefinition::new(
            "request_shell_permission",
            "Request one-time unrestricted permission for an exact command when sandbox access cannot be inferred. Auto-approval/unlimited mode resolves immediately.",
            json!({"type":"object","properties":{"command":{"type":"string"},"reason":{"type":"string"}},"required":["command","reason"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "computer_use",
            "Control native desktop apps on macOS, Linux, and Windows through the installed Open Computer Use runtime. The JavaScript session persists across calls. On the first call after start/reset, execute exactly one entry-point call such as `await cua.getState()` or `let app = await cua.getApp(\"App Name\")`, then read the returned runtime documentation before using further APIs. Prefer purpose-built APIs/connectors when available.",
            json!({
                "type":"object",
                "properties":{
                    "code":{"type":"string","description":"JavaScript to execute using Open Computer Use’s initialized Computer Use runtime."},
                    "timeout_ms":{"type":"integer","minimum":1,"description":"Optional execution timeout in milliseconds. The runtime defaults to 30000 when omitted."},
                    "title":{"type":"string","minLength":1,"maxLength":80,"description":"Short user-facing description of what the Computer Use action does."}
                },
                "required":["code"],
                "additionalProperties":false
            }),
        ),
        ToolDefinition::new(
            "computer_use_reset",
            "Reset Yeet's persistent Open Computer Use JavaScript session. This discards JavaScript bindings but does not close apps or erase their state.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "apply_file_edits",
            "Create, edit, move, or delete files. Read a file before editing it. Pass either `patch` or `changes`. `patch` takes a patch document: `*** Begin Patch`, then file sections `*** Add File: PATH` (every line prefixed `+`), `*** Delete File: PATH`, or `*** Update File: PATH` (optionally followed by `*** Move to: NEW_PATH`) with hunks introduced by `@@` or `@@ <line near the change>`, where ` ` lines are unchanged context, `-` lines are removed and `+` lines are added; include about 3 context lines around each change; end with `*** End Patch`. In `changes`, replaceText defaults to exact matching (oldText must match once unless replaceAll). Explicit matchPolicy=relaxed permits only unique whole-line whitespace fallback; replaceAll uses exact matches. Other edits operate by line range (replace/delete/insert), or by named declaration (replaceNode/replaceBody/deleteNode/insertBefore/insertAfter).",
            json!({
                "type":"object",
                "properties":{
                    "patch":{"type":"string"},
                    "changes":{
                        "type":"array","minItems":1,
                        "items":{
                            "type":"object",
                            "properties":{
                                "path":{"type":"string"},
                                "edits":{
                                    "type":"array","minItems":1,
                                    "items":{
                                        "oneOf": file_edit_variants()
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
                "oneOf":[{"required":["patch"]},{"required":["changes"]}],
                "additionalProperties":false
            }),
        ),
    ]
}

/// One range contract shared by native batches and the single-file MCP tool.
fn read_file_range_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "properties":{
            "path":{"type":"string","minLength":1},
            "startLine":{"type":"integer","minimum":1},
            "endLine":{"type":"integer","minimum":1,"description":"Inclusive absolute line number; must be at least startLine."},
            "lineCount":{"type":"integer","minimum":1,"description":"Number of lines from startLine; cannot be combined with endLine."},
            "refresh":{"type":"boolean","description":"Compatibility option; reads always use current file contents."}
        },
        "required":["path"],
        "allOf":[{"not":{"required":["endLine","lineCount"]}}],
        "additionalProperties":false
    })
}

fn native_read_file_schema() -> serde_json::Value {
    let range = read_file_range_schema();
    let mut schema = range.clone();
    let object = schema.as_object_mut().expect("read schema object");
    object.remove("required");
    object
        .get_mut("properties")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(
            "requests".into(),
            json!({"type":"array","minItems":1,"maxItems":8,"items":range}),
        );
    object.insert(
        "oneOf".into(),
        json!([
            {"required":["path"],"not":{"required":["requests"]}},
            {"required":["requests"],"not":{"anyOf":[
                {"required":["path"]},{"required":["startLine"]},{"required":["endLine"]},
                {"required":["lineCount"]},{"required":["refresh"]}
            ]}}
        ]),
    );
    schema
}

/// Launch schemas are shared by native and MCP tools to prevent contract drift.
fn shell_execution_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "properties":{
            "command":{"type":"string","minLength":1},
            "executable":{"type":"string","minLength":1},
            "args":{"type":"array","items":{"type":"string"}},
            "shell":{"type":"string","enum":["auto","bash","zsh","sh","powershell","cmd"]},
            "loadConfig":{"type":"boolean"},
            "login":{"type":"boolean"},
            "purpose":{"type":"string"},
            "workingDirectory":{"type":"string"},
            "mode":{"type":"string","enum":["auto","direct","actor"]},
            "timeoutSeconds":{"type":"integer","minimum":1,"maximum":86400},
            "yieldSeconds":{"type":"integer","minimum":1,"maximum":86400},
            "background":{"type":"boolean"},
            "pty":{"type":"boolean"}
        },
        "oneOf":[
            {"required":["command"],"not":{"anyOf":[{"required":["executable"]},{"required":["args"]}]}},
            {"required":["executable"],"not":{"anyOf":[
                {"required":["command"]},{"required":["shell"]},
                {"required":["loadConfig"]},{"required":["login"]}
            ]}}
        ],
        "additionalProperties":false
    })
}

fn shell_job_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "properties":{
            "action":{"type":"string","enum":["check","wait","list","stop","forget","output","input","resize","interrupt"]},
            "jobId":{"type":"string","minLength":1},
            "reportEverySeconds":{"type":"integer","minimum":1,"maximum":86400},
            "offset":{"type":"integer","minimum":0},
            "maxBytes":{"type":"integer","minimum":1,"maximum":8192},
            "format":{"type":"string","enum":["text","events"],"default":"text","description":"output returns decoded text by default; events preserves raw base64 bytes for terminal clients."},
            "waitMilliseconds":{"type":"integer","minimum":0,"maximum":30000},
            "data":{"type":"string"},
            "rows":{"type":"integer","minimum":1,"maximum":65535},
            "cols":{"type":"integer","minimum":1,"maximum":65535}
        },
        "required":["action"],
        "oneOf":[
            {"properties":{"action":{"const":"list"}}},
            {"properties":{"action":{"enum":["check","wait","stop","forget","output","input","resize","interrupt"]}},"required":["jobId"]}
        ],
        "additionalProperties":false
    })
}

fn file_edit_variants() -> serde_json::Value {
    json!([
        replace_text_edit_schema(),
        {"type":"object","properties":{"kind":{"const":"replace"},"range":{"type":"object","properties":{"start":{"type":"integer","minimum":1},"end":{"type":"integer","minimum":1}},"required":["start","end"],"additionalProperties":false},"text":{"type":"string"}},"required":["kind","range","text"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"delete"},"range":{"type":"object","properties":{"start":{"type":"integer","minimum":1},"end":{"type":"integer","minimum":1}},"required":["start","end"],"additionalProperties":false}},"required":["kind","range"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"insert"},"at":{"oneOf":[{"type":"object","properties":{"kind":{"const":"start"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"end"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"before"},"line":{"type":"integer","minimum":1}},"required":["kind","line"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"after"},"line":{"type":"integer","minimum":1}},"required":["kind","line"],"additionalProperties":false}]},"text":{"type":"string"}},"required":["kind","at","text"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"replaceNode"},"target":{"type":"object","properties":{"kind":{"type":"string","enum":["function","method","type"]},"name":{"type":"string","minLength":1},"container":{"oneOf":[{"type":"string","minLength":1},{"type":"array","minItems":1,"items":{"type":"string","minLength":1}}]},"signature":{"type":"string","minLength":1}},"required":["name"],"additionalProperties":false},"text":{"type":"string"}},"required":["kind","target","text"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"replaceBody"},"target":{"type":"object","properties":{"kind":{"type":"string","enum":["function","method"]},"name":{"type":"string","minLength":1},"container":{"oneOf":[{"type":"string","minLength":1},{"type":"array","minItems":1,"items":{"type":"string","minLength":1}}]},"signature":{"type":"string","minLength":1}},"required":["name"],"additionalProperties":false},"text":{"type":"string"}},"required":["kind","target","text"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"deleteNode"},"target":{"type":"object","properties":{"kind":{"type":"string","enum":["function","method","type"]},"name":{"type":"string","minLength":1},"container":{"oneOf":[{"type":"string","minLength":1},{"type":"array","minItems":1,"items":{"type":"string","minLength":1}}]},"signature":{"type":"string","minLength":1}},"required":["name"],"additionalProperties":false}},"required":["kind","target"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"insertBefore"},"target":{"type":"object","properties":{"kind":{"type":"string","enum":["function","method","type"]},"name":{"type":"string","minLength":1},"container":{"oneOf":[{"type":"string","minLength":1},{"type":"array","minItems":1,"items":{"type":"string","minLength":1}}]},"signature":{"type":"string","minLength":1}},"required":["name"],"additionalProperties":false},"text":{"type":"string"}},"required":["kind","target","text"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"insertAfter"},"target":{"type":"object","properties":{"kind":{"type":"string","enum":["function","method","type"]},"name":{"type":"string","minLength":1},"container":{"oneOf":[{"type":"string","minLength":1},{"type":"array","minItems":1,"items":{"type":"string","minLength":1}}]},"signature":{"type":"string","minLength":1}},"required":["name"],"additionalProperties":false},"text":{"type":"string"}},"required":["kind","target","text"],"additionalProperties":false}
    ])
}

fn replace_text_edit_schema() -> serde_json::Value {
    json!({"type":"object","properties":{"kind":{"const":"replaceText"},"oldText":{"type":"string","minLength":1},"newText":{"type":"string"},"replaceAll":{"type":"boolean"},"matchPolicy":{"type":"string","enum":["exact","relaxed"],"default":"exact","description":"Exact by default. Relaxed explicitly permits a unique whole-line whitespace fallback; replaceAll uses exact matches only."}},"required":["kind","oldText","newText"],"additionalProperties":false})
}

/// Main-agent-only question tool; registered by `ToolRegistry::enable_user_questions`.
pub(super) fn ask_user_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "ask_user",
        "Ask the user one question and wait for the answer. Use only when a decision genuinely needs the user (ambiguous requirements, risky or irreversible choices, preferences); do not ask what you can discover yourself. Provide 1-3 short suggested options, most recommended first; the user may also type a custom answer unless allowCustom=false.",
        json!({
            "type":"object",
            "properties":{
                "question":{"type":"string","minLength":1},
                "options":{"type":"array","items":{"type":"string","minLength":1},"minItems":1,"maxItems":3},
                "allowCustom":{"type":"boolean"}
            },
            "required":["question","options"],
            "additionalProperties":false
        }),
    )
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
                "query":{"type":"string"},
                "queries":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":4},
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

fn mcp_read_file_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "read_file",
        "Read one UTF-8 file range with line/hash edit anchors and a snapshot. MCP permits one file per call. Use inclusive startLine/endLine or startLine/lineCount; reversed bounds are errors. nextStartLine continues after this page; firstUnreadLine identifies the earliest unread gap. Reads always use current contents; refresh is a compatibility option. Relative paths resolve from the selected workspace.",
        read_file_range_schema(),
    )
}

fn mcp_run_shell_tool_definition() -> ToolDefinition {
    let mut schema = shell_execution_schema();
    let object = schema.as_object_mut().expect("shell schema object");
    let execution_fields = object["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    object
        .get_mut("properties")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("job".into(), shell_job_schema());
    let variants = object.get_mut("oneOf").unwrap().as_array_mut().unwrap();
    for variant in variants.iter_mut() {
        variant["not"]["anyOf"]
            .as_array_mut()
            .unwrap()
            .push(json!({"required":["job"]}));
    }
    variants.push(json!({
        "required":["job"],
        "not":{"anyOf":execution_fields.into_iter().map(|field| json!({"required":[field]})).collect::<Vec<_>>()}
    }));
    ToolDefinition::new(
        "run_shell",
        "Run exactly one shell command, executable with literal args, or existing job operation. Shell defaults to clean non-interactive initialization; loadConfig/login are opt-ins. Direct execution retains sandbox checks. Only cwd persists between commands. yieldSeconds (default 120) ends the foreground wait while the process keeps running; timeoutSeconds is a hard deadline. Completed and running output is retained under jobId: use job:{action:output,jobId,...} to page it, or job:{action:wait,jobId} for event-driven completion. Capture limits and retention expiry can make output unavailable; inspect completeness fields. MCP does not export read_artifact. Job operations cannot be combined with execution options.",
        schema,
    )
}

fn mcp_web_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "web",
        "Search the live web or read one source through one MCP tool. Use action=search with query or queries. Use action=read with a URL returned by a prior search in the same research task.",
        json!({
            "type":"object",
            "properties":{
                "action":{"type":"string","enum":["search","read"]},
                "query":{"type":"string"},
                "queries":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":4},
                "maxResults":{"type":"integer","minimum":1,"maximum":8},
                "backend":{"type":"string","enum":["auto","agent-reach","searxng"]},
                "language":{"type":"string"},
                "category":{"type":"string"},
                "timeRange":{"type":"string","enum":["day","month","year"]},
                "safeSearch":{"type":"integer","minimum":0,"maximum":2},
                "page":{"type":"integer","minimum":1,"maximum":10},
                "url":{"type":"string","minLength":1},
                "maxChars":{"type":"integer","minimum":2000,"maximum":4000}
            },
            "required":["action"],
            "oneOf":[
                {"properties":{"action":{"const":"search"}},"anyOf":[{"required":["query"]},{"required":["queries"]}]},
                {"properties":{"action":{"const":"read"}},"required":["url"]}
            ],
            "additionalProperties":false
        }),
    )
}

/// Minimal native Yeet tool surface exported directly over MCP.
pub(crate) fn direct_mcp_tool_definitions() -> Vec<ToolDefinition> {
    let base = base_tool_definitions();
    let definition = |name: &str| {
        base.iter()
            .find(|tool| tool.name == name)
            .cloned()
            .expect("core MCP tool definition")
    };
    vec![
        mcp_read_file_tool_definition(),
        definition("apply_file_edits"),
        mcp_run_shell_tool_definition(),
        definition("computer_use"),
        mcp_web_tool_definition(),
    ]
}
