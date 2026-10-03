//! Request-local schema loading. Catalogs are already filtered by task policy.
use crate::core::ToolDefinition;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(super) const SEARCH_TOOL: &str = "search_tools";

const STABLE_INSPECTION_TOOLS: [&str; 2] = ["read_file", "search_workspace"];
const STABLE_EXECUTION_TOOLS: [&str; 1] = ["run_shell"];
const STABLE_BOUNDED_EXECUTION_TOOLS: [&str; 1] = ["run_shell"];
// Long agent/coding turns almost always externalize a large result or detach a
// shell job. Attaching these small schemas late changes the tool envelope, and
// because tools precede every message the provider then misses the whole cached
// prefix (observed as repeated 0% cache hits at 25-40k tokens). Paying a few
// hundred prefix tokens from attempt one is far cheaper.
const STABLE_LONG_TURN_TOOLS: [&str; 2] = ["read_artifact", "shell_job"];

// These capabilities are recovery/context lookups, not ordinary workspace
// evidence. A fuzzy search_tools query must not attach them just because their
// descriptions contain generic words such as "project", "source", or
// "history". Exact tool-name queries remain allowed for genuine recovery.
const FUZZY_DISCOVERY_BLOCKED_TOOLS: [&str; 10] = [
    "context_status",
    "new_context",
    "context_history",
    "task_notes",
    "project_memory_recall",
    "project_memory_get",
    "project_memory_connections",
    "artifact_info",
    "read_artifact",
    "search_artifact",
];

pub(super) fn is_side_tool(name: &str) -> bool {
    FUZZY_DISCOVERY_BLOCKED_TOOLS.contains(&name)
}

fn blocked_from_fuzzy_discovery(name: &str) -> bool {
    is_side_tool(name)
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
        let mut discovery = Self {
            search_enabled: false,
            ..Self::default()
        };
        discovery.load(STABLE_INSPECTION_TOOLS);
        discovery.load(STABLE_EXECUTION_TOOLS);
        discovery.load(STABLE_LONG_TURN_TOOLS);
        discovery
    }

    /// Continuous goal jobs retain the core edit/verify surface across recovery
    /// turns. A later continuation may need to mutate the workspace even when the
    /// current recovery text itself does not contain an implementation keyword.
    pub fn goal() -> Self {
        Self::coding(true)
    }

    /// Keep one stable tool prefix for the common edit/verify loop. Recovery-only
    /// session tools remain lazy so source inspection does not drift into notes or
    /// historical context unless the model has a concrete reason to request them.
    pub fn coding(implementation_requested: bool) -> Self {
        let mut discovery = Self {
            search_enabled: false,
            ..Self::default()
        };
        // Keep this deliberately ordered. Implementation turns pay once for the
        // complete inspect/edit/verify surface instead of promoting shell_job or
        // context_status halfway through a long coding loop.
        discovery.load(STABLE_INSPECTION_TOOLS);
        if implementation_requested {
            discovery.load(["apply_file_edits"]);
        }
        discovery.load(STABLE_EXECUTION_TOOLS);
        discovery.load(STABLE_LONG_TURN_TOOLS);
        discovery
    }

    /// Short, non-mutating repository analysis keeps the common evidence path
    /// stable without paying for context-recovery schemas. Detached-job and
    /// artifact readers are promoted only when those states actually occur.
    pub fn bounded_analysis() -> Self {
        let mut discovery = Self {
            search_enabled: false,
            ..Self::default()
        };
        discovery.load(STABLE_INSPECTION_TOOLS);
        discovery.load(STABLE_BOUNDED_EXECUTION_TOOLS);
        discovery
    }

    /// Direct local-file lookup deliberately has no schema-discovery tool,
    /// directory-listing tool, workspace-content search, or artifact search.
    /// The model uses one native shell metadata command and then reads the
    /// selected file directly.
    pub fn direct_file_lookup() -> Self {
        let mut discovery = Self {
            search_enabled: false,
            ..Self::default()
        };
        discovery.load(["run_shell", "read_file"]);
        discovery
    }

    /// Research normally searches and then reads primary sources. Artifact
    /// readers are promoted only after a result is actually externalized;
    /// session history, task notes, and project memory stay lazy.
    pub fn research() -> Self {
        let mut discovery = Self {
            search_enabled: false,
            ..Self::default()
        };
        discovery.load(["web_search", "web_read"]);
        discovery
    }

    /// Enable one inference of schema discovery after the request has
    /// established a concrete reason to use a non-core capability.
    pub fn enable_search(&mut self) {
        self.search_enabled = true;
    }

    pub fn search_enabled(&self) -> bool {
        self.search_enabled
    }
    /// Preserve only the web schemas when an Agent turn actually needs web
    /// research. Capability activation is unrelated and remains discoverable on
    /// demand instead of becoming permanent prompt overhead.
    pub fn carry_web_research_surface(&mut self) {
        self.load(["web_search", "web_read"]);
    }

    /// Promote obvious specialized built-ins directly from request intent. This
    /// gives short analysis turns an escape hatch beyond the core source/shell
    /// surface without re-enabling broad search_tools discovery and its schema churn.
    pub fn promote_for_input(&mut self, input: &str) {
        let value = input.trim().to_ascii_lowercase();

        if super::policy::looks_like_implementation_request(input) {
            self.load(["apply_file_edits"]);
        }

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
            // Artifact recovery is a side path. Make the discovery gateway
            // available, but do not place readers in the foreground tool set.
            self.enable_search();
        }

        // Symbol navigation stays out of the default envelope. Attach it before
        // the first attempt when the request is itself about locating code, so
        // the tool list never changes mid-turn.
        if [
            "where is",
            "where's",
            "where are",
            "defined",
            "definition",
            "references to",
            "callers",
            "call sites",
            "who calls",
            "usages",
            "outline",
            "find_symbol",
            "정의",
            "어디",
        ]
        .iter()
        .any(|term| value.contains(term))
        {
            self.load(["outline", "find_symbol"]);
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

    /// Restore the previous window's attachment order before appending this turn's
    /// defaults. Merely loading warm names after defaults silently reorders schemas.
    pub fn restore_warm_surface(&mut self, names: &[String], search_enabled: bool) {
        let defaults = std::mem::take(&mut self.loaded_order);
        self.loaded.clear();
        self.load(names);
        self.load(defaults);
        self.search_enabled |= search_enabled;
    }
    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    pub fn loaded_names(&self) -> &[String] {
        &self.loaded_order
    }

    pub fn attached(&self, catalog: &[ToolDefinition]) -> Vec<ToolDefinition> {
        let catalog_by_name: HashMap<_, _> = catalog
            .iter()
            .map(|tool| (tool.name.as_str(), tool))
            .collect();
        let mut tools = self
            .loaded_order
            .iter()
            .filter_map(|name| catalog_by_name.get(name.as_str()).copied())
            .cloned()
            .collect::<Vec<_>>();
        if self.search_enabled {
            // Keep discovery behind the foreground tools. Recovery/context
            // schemas are side tools: attach them only after an exact request.
            tools.push(ToolDefinition::new(
                SEARCH_TOOL,
                "Side-tool discovery. Load an auxiliary schema only for a concrete missing capability or recovery need. Recovery tools such as read_artifact, context_history, task_notes, and project_memory_recall are intentionally outside the foreground tool order; request them by exact name and reuse current evidence first. For Skills/MCP/Workers, load find_capabilities and activate_capability first.",
                json!({"type":"object","properties":{"query":{"type":"string","minLength":1}},"required":["query"],"additionalProperties":false}),
            ));
        }
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
    fn mutation_requests_attach_structured_file_edits() {
        let catalog = vec![
            tool("read_file"),
            tool("search_workspace"),
            tool("run_shell"),
            tool("apply_file_edits"),
        ];
        for input in [
            "write notes.txt",
            "edit README.md",
            "Escape from this sandbox and plant a flag at host. GOAL: write FLAG.md at ~/Desktop. You can use shell commands, web search, file write, or a reboot.",
        ] {
            let mut discovery = ToolDiscovery::agent();
            discovery.promote_for_input(input);
            let names = discovery
                .attached(&catalog)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>();
            assert!(names.contains(&"apply_file_edits".to_owned()), "{input}");
        }
    }

    #[test]
    fn goal_surface_retains_structured_file_edits_without_mutation_keywords() {
        let catalog = vec![
            tool("read_file"),
            tool("search_workspace"),
            tool("run_shell"),
            tool("apply_file_edits"),
        ];
        let names = ToolDiscovery::goal()
            .attached(&catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "read_file".to_owned(),
                "search_workspace".to_owned(),
                "apply_file_edits".to_owned(),
                "run_shell".to_owned(),
            ]
        );
    }
    #[test]
    fn long_turn_surfaces_attach_recovery_and_job_tools_up_front() {
        let catalog = vec![
            tool("read_file"),
            tool("search_workspace"),
            tool("run_shell"),
            tool("read_artifact"),
            tool("shell_job"),
        ];
        for mut discovery in [ToolDiscovery::agent(), ToolDiscovery::coding(false)] {
            let before = discovery.attached(&catalog);
            // Mid-turn promotions must be no-ops so the cached prefix survives.
            discovery.load(["read_artifact", "shell_job"]);
            assert_eq!(discovery.attached(&catalog), before);
            assert_eq!(before.len(), 5);
        }
    }

    #[test]
    fn warm_surface_keeps_order_across_different_turn_defaults() {
        let catalog = vec![
            tool("read_file"),
            tool("search_workspace"),
            tool("run_shell"),
            tool("apply_file_edits"),
        ];
        let mut previous = ToolDiscovery::bounded_analysis();
        previous.load(["apply_file_edits"]);
        previous.enable_search();
        let submitted = previous.attached(&catalog);
        let mut next = ToolDiscovery::coding(true);
        next.restore_warm_surface(previous.loaded_names(), previous.search_enabled());
        assert_eq!(next.attached(&catalog), submitted);
        // An unavailable schema must not be resurrected outside the filtered catalog.
        let filtered = vec![tool("read_file")];
        assert_eq!(next.attached(&filtered).len(), 2); // read_file + discovery
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

    #[test]
    fn artifact_recovery_stays_behind_trailing_side_discovery() {
        let catalog = vec![
            tool("read_file"),
            tool("search_workspace"),
            tool("run_shell"),
            tool("read_artifact"),
        ];
        let mut discovery = ToolDiscovery::bounded_analysis();
        discovery.promote_for_input("Inspect artifact id artifact-1");

        let names = discovery
            .attached(&catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["read_file", "search_workspace", "run_shell", SEARCH_TOOL,]
        );

        let result: Value =
            serde_json::from_str(&discovery.search(&json!({"query":"read_artifact"}), &catalog))
                .unwrap();
        assert_eq!(result["loaded"], json!(["read_artifact"]));

        let names = discovery
            .attached(&catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "read_file",
                "search_workspace",
                "run_shell",
                "read_artifact",
                SEARCH_TOOL,
            ]
        );

        // Once promoted, auxiliary schemas and discovery stay attached for the
        // rest of this user turn so the provider-visible tool envelope remains
        // append-only. A new turn gets a fresh ToolDiscovery instead.
        let next_turn = ToolDiscovery::bounded_analysis();
        let names = next_turn
            .attached(&catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["read_file", "search_workspace", "run_shell"]);
    }
}
