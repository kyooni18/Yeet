//! Built-in tool schemas exposed to the model runtime.
//!
//! Keeping JSON schemas here prevents execution logic from becoming coupled to
//! long provider-facing descriptions and makes capability review straightforward.

use serde_json::json;

use crate::core::ToolDefinition;

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
            "Read one UTF-8 file or batch up to 8 file ranges with line/hash edit anchors and snapshots. Use path for one file or requests for a batch. Outside-project access follows approval rules. Reuse covered ranges; refresh=true forces fresh contents/anchors.",
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
            "list_files",
            "List workspace files/directories without reading contents. Prefer shallow focused listings.",
            json!({"type":"object","properties":{"path":{"type":"string"},"maxResults":{"type":"integer","minimum":1,"maximum":500},"maxDepth":{"type":"integer","minimum":0,"maximum":12}},"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "search_workspace",
            "Search workspace text. Matching is literal unless regex=true; narrow path when possible.",
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
            "Read artifact lines (default 160, max 8192 chars). If truncated, repeat the same range with offset=nextOffset.",
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
            "Run a shell command. background=true returns a jobId for shell_job. Sandboxed commands request approval when needed; unlimited mode uses normal user access. mode=actor summarizes builds/tests/noisy output and stores the full log.",
            json!({"type":"object","properties":{"command":{"type":"string"},"purpose":{"type":"string"},"workingDirectory":{"type":"string"},"mode":{"type":"string","enum":["auto","direct","actor"]},"timeoutSeconds":{"type":"integer","minimum":1,"maximum":900},"background":{"type":"boolean"}},"required":["command"],"additionalProperties":false}),
        ),
        ToolDefinition::new(
            "shell_job",
            "Check/list/stop/forget detached shell jobs. Running is not success; reuse jobId instead of rerunning. Forget only completed jobs.",
            json!({"type":"object","properties":{"action":{"type":"string","enum":["check","list","stop","forget"]},"jobId":{"type":"string"}},"required":["action"],"additionalProperties":false}),
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
            "Apply atomic structured file edits. Use JSON edit objects, never patch strings. replace/delete use range:{start,end,startHash?,endHash?}; never top-level start/end. Hash fields are the 4-character token from read_file's `line:hash|text` anchor (for example `9b12`, not the source text). Sandboxed existing files require a read_file snapshot; creates do not. Outside-project access follows approval rules; unlimited mode may edit without a snapshot.",
            json!({
                "type":"object",
                "properties":{
                    "changes":{
                        "type":"array","minItems":1,
                        "items":{
                            "type":"object",
                            "properties":{
                                "path":{"type":"string"},
                                "snapshot":{"type":"string"},
                                "edits":{
                                    "type":"array","minItems":1,
                                    "items":{
                                        "oneOf":[
                                            {"type":"object","properties":{"kind":{"const":"replace"},"range":{"type":"object","properties":{"start":{"type":"integer","minimum":1},"end":{"type":"integer","minimum":1},"startHash":{"type":"string"},"endHash":{"type":"string"}},"required":["start","end"],"additionalProperties":false},"text":{"type":"string"}},"required":["kind","range","text"],"additionalProperties":false},
                                            {"type":"object","properties":{"kind":{"const":"delete"},"range":{"type":"object","properties":{"start":{"type":"integer","minimum":1},"end":{"type":"integer","minimum":1},"startHash":{"type":"string"},"endHash":{"type":"string"}},"required":["start","end"],"additionalProperties":false}},"required":["kind","range"],"additionalProperties":false},
                                            {"type":"object","properties":{"kind":{"const":"insert"},"at":{"oneOf":[{"type":"object","properties":{"kind":{"const":"start"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"end"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"before"},"line":{"type":"integer","minimum":1},"hash":{"type":"string"}},"required":["kind","line"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"after"},"line":{"type":"integer","minimum":1},"hash":{"type":"string"}},"required":["kind","line"],"additionalProperties":false}]},"text":{"type":"string"}},"required":["kind","at","text"],"additionalProperties":false}
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
            | "list_files"
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
        "Search the live web. Keep result sets small, batch only complementary queries, and reuse returned sources. Use another search only to resolve a concrete gap. Use backend=searxng for language/category/time filters or pagination.",
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
        "Read one URL returned by web_search. Use original-source text for material claims. Reads return a bounded preview and preserve larger fetched text as an artifact for targeted follow-up. Duplicate URLs are skipped regardless of maxChars.",
        json!({
            "type":"object",
            "properties":{
                "url":{"type":"string","minLength":1},
                "maxChars":{"type":"integer","minimum":2000,"maximum":12000}
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
                "find_capabilities" | "activate_capability" | "request_shell_permission"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_read_surface_uses_one_tool_for_single_and_batch_reads() {
        let tools = base_tool_definitions();
        assert!(tools.iter().all(|tool| tool.name != "read_files"));

        let read = tools.iter().find(|tool| tool.name == "read_file").unwrap();
        let properties = read.input_schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("path"));
        assert!(properties.contains_key("requests"));
        assert_eq!(properties["requests"]["maxItems"].as_u64(), Some(8));
    }

    #[test]
    fn structured_edit_schema_only_advertises_runtime_supported_operations() {
        let tools = base_tool_definitions();
        let edit = tools
            .iter()
            .find(|tool| tool.name == "apply_file_edits")
            .unwrap();
        let rendered = serde_json::to_string(&edit.input_schema).unwrap();
        assert!(!rendered.contains("replaceBlock"));
        assert!(!rendered.contains("insertAfterBlock"));
        assert!(!rendered.contains("deleteBlock"));
        assert!(rendered.contains("replace"));
        assert!(rendered.contains("insert"));
        assert!(rendered.contains("delete"));
    }

    #[test]
    fn web_research_schemas_keep_evidence_bounded() {
        let search = web_search_tool_definition();
        let read = web_read_tool_definition();

        assert_eq!(
            search.input_schema["properties"]["maxResults"]["maximum"].as_u64(),
            Some(8)
        );
        assert_eq!(
            read.input_schema["properties"]["maxChars"]["maximum"].as_u64(),
            Some(12_000)
        );
    }

    #[test]
    fn computer_use_matches_codex_runtime_surface_and_is_mcp_exported() {
        let tools = base_tool_definitions();
        let computer = tools
            .iter()
            .find(|tool| tool.name == "computer_use")
            .unwrap();
        let properties = computer.input_schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("code"));
        assert!(properties.contains_key("timeout_ms"));
        assert!(properties.contains_key("title"));
        assert_eq!(computer.input_schema["required"], json!(["code"]));

        let direct = direct_mcp_tool_definitions();
        assert!(direct.iter().any(|tool| tool.name == "computer_use"));
        assert!(direct.iter().any(|tool| tool.name == "computer_use_reset"));
        assert!(direct.iter().any(|tool| tool.name == "desktop_control"));
        assert!(
            direct
                .iter()
                .any(|tool| tool.name == "desktop_control_reset")
        );
    }
}
