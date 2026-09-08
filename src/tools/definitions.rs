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
            "apply_file_edits",
            "Apply atomic structured file edits. Use JSON edit objects, never patch strings. replace/delete use range:{start,end,startHash?,endHash?}; never top-level start/end. Sandboxed existing files require a read_file snapshot; creates do not. Outside-project access follows approval rules; unlimited mode may edit without a snapshot.",
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
                                            {"type":"object","properties":{"kind":{"const":"insert"},"at":{"oneOf":[{"type":"object","properties":{"kind":{"const":"start"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"end"}},"required":["kind"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"before"},"line":{"type":"integer","minimum":1},"hash":{"type":"string"}},"required":["kind","line"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"after"},"line":{"type":"integer","minimum":1},"hash":{"type":"string"}},"required":["kind","line"],"additionalProperties":false}]},"text":{"type":"string"}},"required":["kind","at","text"],"additionalProperties":false},
                                            {"type":"object","properties":{"kind":{"const":"replaceBlock"},"line":{"type":"integer","minimum":1},"hash":{"type":"string"},"text":{"type":"string"}},"required":["kind","line","text"],"additionalProperties":false},
                                            {"type":"object","properties":{"kind":{"const":"insertAfterBlock"},"line":{"type":"integer","minimum":1},"hash":{"type":"string"},"text":{"type":"string"}},"required":["kind","line","text"],"additionalProperties":false},
                                            {"type":"object","properties":{"kind":{"const":"deleteBlock"},"line":{"type":"integer","minimum":1},"hash":{"type":"string"}},"required":["kind","line"],"additionalProperties":false}
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
        "Search the live web. Batch complementary searches with queries and reuse sources. Use backend=searxng for language/category/time filters or pagination.",
        json!({
            "type":"object",
            "properties":{
                "query":{"type":"string"},
                "queries":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":4},
                "maxResults":{"type":"integer","minimum":1,"maximum":20},
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

/// Returns the full-source web reader schema.
pub(super) fn web_read_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "web_read",
        "Read one URL returned by web_search. Use original-source text, not snippets, for material source-grounded claims. Duplicate reads are skipped.",
        json!({
            "type":"object",
            "properties":{
                "url":{"type":"string","minLength":1},
                "maxChars":{"type":"integer","minimum":2000,"maximum":48000}
            },
            "required":["url"],
            "additionalProperties":false
        }),
    )
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
}
