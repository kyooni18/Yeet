//! Request-local schema loading. Catalogs are already filtered by task policy.
use crate::core::ToolDefinition;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(super) const SEARCH_TOOL: &str = "search_tools";

const STABLE_INSPECTION_TOOLS: [&str; 2] = ["read_file", "search_workspace"];
const STABLE_EXECUTION_TOOLS: [&str; 1] = ["run_shell"];
const STABLE_BOUNDED_EXECUTION_TOOLS: [&str; 1] = ["run_shell"];
const ARTIFACT_RECOVERY_TOOLS: [&str; 3] = ["artifact_info", "read_artifact", "search_artifact"];

// These capabilities are recovery/context lookups, not ordinary workspace
// evidence. A fuzzy search_tools query must not attach them just because their
// descriptions contain generic words such as "project", "source", or
// "history". Exact tool-name queries remain allowed for genuine recovery.
const FUZZY_DISCOVERY_BLOCKED_TOOLS: [&str; 8] = [
    "context_history",
    "task_notes",
    "project_memory_recall",
    "project_memory_get",
    "project_memory_connections",
    "artifact_info",
    "read_artifact",
    "search_artifact",
];

fn blocked_from_fuzzy_discovery(name: &str) -> bool {
    FUZZY_DISCOVERY_BLOCKED_TOOLS.contains(&name)
}

fn artifact_recovery_is_ready(loaded: &HashSet<String>) -> bool {
    loaded.contains("read_artifact")
}

pub(super) struct ToolDiscovery {
    loaded: HashSet<String>,
    search_enabled: bool,
    // Prompt-cache ABI for this turn. The registry catalog may be backed by
    // maps or extension/server enumeration whose order can change even when
    // schemas do not. Preserve first attachment order and append later tools.
    loaded_order: Vec<String>,
}

impl Default for ToolDiscovery {
    fn default() -> Self {
        Self {
            loaded: HashSet::new(),
            search_enabled: true,
            loaded_order: Vec::new(),
        }
    }
}

impl ToolDiscovery {
    /// General agent turns keep the common workspace inspection/execution loop
    /// attached from attempt one. Session-history and durable-memory recovery stays
    /// lazy: ordinary local analysis should not be invited to search task notes or
    /// history before it has used direct workspace evidence.
    pub fn agent() -> Self {
        let mut discovery = Self::default();
        discovery.search_enabled = false;
        discovery.load(STABLE_INSPECTION_TOOLS);
        discovery.load(STABLE_EXECUTION_TOOLS);
        discovery.load(["deploy_agent"]);
        discovery
    }

    /// Keep one stable tool prefix for the common edit/verify loop. Recovery-only
    /// session tools remain lazy so source inspection does not drift into notes or
    /// historical context unless the model has a concrete reason to request them.
    pub fn coding(implementation_requested: bool) -> Self {
        let mut discovery = Self::default();
        discovery.search_enabled = false;
        // Keep this deliberately ordered. Implementation turns pay once for the
        // complete inspect/edit/verify surface instead of promoting shell_job or
        // context_status halfway through a long coding loop.
        discovery.load(STABLE_INSPECTION_TOOLS);
        if implementation_requested {
            discovery.load(["apply_file_edits"]);
        }
        discovery.load(STABLE_EXECUTION_TOOLS);
        discovery
    }

    /// Short, non-mutating repository analysis keeps the common evidence path
    /// stable without paying for context-recovery schemas. Detached-job and
    /// artifact readers are promoted only when those states actually occur.
    pub fn bounded_analysis() -> Self {
        let mut discovery = Self::default();
        discovery.search_enabled = false;
        discovery.load(STABLE_INSPECTION_TOOLS);
        discovery.load(STABLE_BOUNDED_EXECUTION_TOOLS);
        discovery
    }

    /// Direct local-file lookup deliberately has no schema-discovery tool,
    /// directory-listing tool, workspace-content search, or artifact search.
    /// The model uses one native shell metadata command and then reads the
    /// selected file directly.
    pub fn direct_file_lookup() -> Self {
        let mut discovery = Self::default();
        discovery.search_enabled = false;
        discovery.load(["run_shell", "read_file"]);
        discovery
    }

    /// Research normally searches and then reads primary sources. Artifact
    /// readers are promoted only after a result is actually externalized;
    /// session history, task notes, and project memory stay lazy.
    pub fn research() -> Self {
        let mut discovery = Self::default();
        discovery.search_enabled = false;
        discovery.load(["web_search", "web_read"]);
        discovery
    }

    /// Enable schema discovery after the request has established a concrete
    /// reason to use a non-core capability (or after an artifact exists).
    pub fn enable_search(&mut self) {
        self.search_enabled = true;
    }

    pub fn search_enabled(&self) -> bool {
        self.search_enabled
    }

    /// Preserve the canonical promotion order observed when an Agent turn moves
    /// into web research. Reusing this exact suffix on a related next turn keeps
    /// the provider-visible tool ABI byte-stable instead of bouncing 15 -> 12 -> 15.
    pub fn carry_web_research_surface(&mut self) {
        self.load(["web_search", "web_read", "activate_capability"]);
    }

    /// Promote obvious specialized built-ins directly from request intent. This
    /// gives short analysis turns an escape hatch beyond the core source/shell
    /// surface without re-enabling broad search_tools discovery and its schema churn.
    pub fn promote_for_input(&mut self, input: &str) {
        let value = input.trim().to_ascii_lowercase();

        if [
            "session",
            "sessions",
            "session history",
            "conversation history",
            "recent runs",
            "run history",
            "세션",
        ]
        .iter()
        .any(|term| value.contains(term))
        {
            self.load(["list_sessions", "export_session"]);
        }

        if [
            "artifact",
            "artifacts",
            "artifact id",
            "externalized",
            "externalised",
            "아티팩트",
        ]
        .iter()
        .any(|term| value.contains(term))
        {
            self.load(["artifact_info", "read_artifact", "search_artifact"]);
        }

        let document_request = [
            "document",
            "pdf",
            "docx",
            "xlsx",
            "spreadsheet",
            "csv",
            "tsv",
            "ods",
            "문서",
            "스프레드시트",
        ]
        .iter()
        .any(|term| value.contains(term));
        if document_request {
            self.load(["read_document"]);
        }

        if document_request
            && [
                "analyze",
                "analyse",
                "analysis",
                "summarize",
                "summarise",
                "statistics",
                "correlation",
                "group by",
                "aggregate",
                "분석",
                "통계",
            ]
            .iter()
            .any(|term| value.contains(term))
        {
            self.load(["analyze_data"]);
        }
    }

    pub fn load<I, S>(&mut self, names: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for name in names {
            self.load_name(name.as_ref());
        }
    }

    fn load_name(&mut self, name: &str) {
        if self.loaded.insert(name.to_owned()) {
            self.loaded_order.push(name.to_owned());
        }
    }

    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    pub fn attached(&self, catalog: &[ToolDefinition]) -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        if self.search_enabled {
            tools.push(ToolDefinition::new(
                SEARCH_TOOL,
                "Load missing tool schemas. Prefer exact tool names. Keyword discovery only attaches strong matches (a name match or multiple meaningful description terms), so generic words do not silently expand the provider-visible tool envelope. For Skills/MCP/Workers, load find_capabilities and activate_capability first.",
                json!({"type":"object","properties":{"query":{"type":"string","minLength":1}},"required":["query"],"additionalProperties":false}),
            ));
        }
        let catalog_by_name: HashMap<_, _> = catalog
            .iter()
            .map(|tool| (tool.name.as_str(), tool))
            .collect();
        tools.extend(
            self.loaded_order
                .iter()
                .filter_map(|name| catalog_by_name.get(name.as_str()).copied())
                .cloned(),
        );
        tools
    }

    pub fn search(&mut self, arguments: &Value, catalog: &[ToolDefinition]) -> String {
        self.search_with_deferred(arguments, catalog, &HashSet::new())
    }

    /// Provider-native deferred references can keep matching extension tools
    /// out of the ordinary schema prefix while still returning their exact
    /// names to the provider adapter. Non-deferred matches keep the existing
    /// local load semantics.
    pub fn search_with_deferred(
        &mut self,
        arguments: &Value,
        catalog: &[ToolDefinition],
        deferred_candidates: &HashSet<String>,
    ) -> String {
        let Some(query) = arguments
            .get("query")
            .and_then(Value::as_str)
            .filter(|q| !q.trim().is_empty())
        else {
            return json!({"error":"Provide a nonempty query with tool names or keywords."})
                .to_string();
        };
        let query = query.trim().to_lowercase();
        let words: HashSet<_> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty())
            .collect();
        let names: HashSet<_> = catalog
            .iter()
            .map(|tool| tool.name.to_lowercase())
            .collect();
        let exact_list = !words.is_empty() && words.iter().all(|word| names.contains(*word));
        let mut matches: Vec<_> = catalog
            .iter()
            .filter_map(|tool| {
                let name = tool.name.to_lowercase();
                let description = tool.description.as_deref().unwrap_or("").to_lowercase();
                if ARTIFACT_RECOVERY_TOOLS.contains(&name.as_str())
                    && !artifact_recovery_is_ready(&self.loaded)
                    && !self.loaded.contains(&name)
                {
                    return None;
                }
                if exact_list && !words.contains(name.as_str()) {
                    return None;
                }
                if !exact_list && blocked_from_fuzzy_discovery(&name) && name != query {
                    return None;
                }
                let (score, strong_match) =
                    if name == query || (exact_list && words.contains(name.as_str())) {
                        (usize::MAX, true)
                    } else {
                        let mut name_hits = 0usize;
                        let mut description_hits = 0usize;
                        for word in &words {
                            if name.contains(word) {
                                name_hits += 1;
                            } else if description.contains(word) {
                                description_hits += 1;
                            }
                        }
                        (
                            name_hits
                                .saturating_mul(10)
                                .saturating_add(description_hits),
                            name_hits > 0 || description_hits >= 2,
                        )
                    };
                (score > 0 && strong_match).then_some((score, tool))
            })
            .collect();
        matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        let total = matches.len();
        if !exact_list
            && matches.first().is_some_and(|(score, tool)| {
                *score == usize::MAX || self.loaded.contains(&tool.name)
            })
        {
            let best_score = matches[0].0;
            // A fuzzy query that already resolves to an attached top match is a
            // lookup, not permission to drag lower-scoring schemas into the wire
            // envelope. Ask for exact names when multiple new tools are intended.
            matches
                .retain(|(score, tool)| *score == best_score && self.loaded.contains(&tool.name));
        }
        if !exact_list && matches.len() > 5 {
            matches.truncate(5);
        }
        let mut loaded = Vec::new();
        let mut already_loaded = Vec::new();
        let mut deferred = Vec::new();
        for (_, tool) in &matches {
            if deferred_candidates.contains(&tool.name) {
                deferred.push(tool.name.clone());
            } else if self.loaded.contains(&tool.name) {
                already_loaded.push(tool.name.clone());
            } else {
                self.load_name(&tool.name);
                loaded.push(tool.name.clone());
            }
        }
        let mut result = json!({
            "loaded": loaded,
            "alreadyLoaded": already_loaded,
            "moreMatches": total > matches.len()
        });
        if !deferred.is_empty()
            && let Some(object) = result.as_object_mut()
        {
            object.insert("deferred".into(), json!(deferred));
        }
        result.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, "test", json!({"type":"object"}))
    }

    #[test]
    fn short_session_analysis_gets_structural_session_tools() {
        let catalog = vec![
            tool("read_file"),
            tool("search_workspace"),
            tool("run_shell"),
            tool("list_sessions"),
            tool("export_session"),
        ];
        let mut discovery = ToolDiscovery::bounded_analysis();
        discovery.promote_for_input("Analyze recent Yeet sessions");
        let names = discovery
            .attached(&catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"list_sessions".to_owned()));
        assert!(names.contains(&"export_session".to_owned()));
        assert!(!names.contains(&SEARCH_TOOL.to_owned()));
    }

    #[test]
    fn recent_workspace_sessions_do_not_use_direct_file_fast_path() {
        assert!(!super::super::policy::looks_like_local_file_lookup(
            "List recent Yeet sessions in this workspace"
        ));
        assert!(super::super::policy::looks_like_local_file_lookup(
            "Analyze the most recent workspace log"
        ));
    }

    #[test]
    fn document_analysis_promotes_document_and_data_tools() {
        let catalog = vec![
            tool("read_file"),
            tool("read_document"),
            tool("analyze_data"),
        ];
        let mut discovery = ToolDiscovery::bounded_analysis();
        discovery.promote_for_input("Analyze this CSV document");
        let names = discovery
            .attached(&catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"read_document".to_owned()));
        assert!(names.contains(&"analyze_data".to_owned()));
    }
}
