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
    #[test]
    fn exact_name_lists_do_not_load_tools_that_only_mention_them() {
        let catalog = vec![
            tool("read_file", "Read a file"),
            tool("run_shell", "Execute a command"),
            tool("activate_capability", "Enables run_shell and read_file"),
            tool("search_workspace", "Find paths for read_file"),
        ];
        let mut discovery = ToolDiscovery::default();
        let result: Value = serde_json::from_str(&discovery.search(
            &json!({"query":"read_file, run_shell, read_file"}),
            &catalog,
        ))
        .unwrap();
        assert_eq!(result["loaded"], json!(["read_file", "run_shell"]));
        assert_eq!(result["moreMatches"], false);
        assert_eq!(discovery.attached(&catalog).len(), 3);
    }

    #[test]
    fn weak_description_only_matches_do_not_expand_tool_schema() {
        let catalog = vec![
            tool(
                "apply_file_edits",
                "Apply structured file edits in the current workspace",
            ),
            tool(
                "list_sessions",
                "List current-workspace Yeet sessions from metadata",
            ),
            tool(
                "export_session",
                "Export one Yeet session from this workspace",
            ),
        ];
        let mut discovery = ToolDiscovery::default();
        let result: Value = serde_json::from_str(&discovery.search(
            &json!({"query":"workspace file edit patch apply"}),
            &catalog,
        ))
        .unwrap();
        assert_eq!(result["loaded"], json!(["apply_file_edits"]));
        assert!(!discovery.loaded.contains("list_sessions"));
        assert!(!discovery.loaded.contains("export_session"));
    }

    #[test]
    fn fuzzy_discovery_does_not_attach_memory_or_artifact_recovery_tools() {
        let catalog = vec![
            tool("read_file", "Read a local file from the workspace"),
            tool(
                "project_memory_recall",
                "Recall project memory, source evidence, and prior task context",
            ),
            tool(
                "context_history",
                "Search prior context history and task evidence",
            ),
            tool(
                "read_artifact",
                "Read an externalized artifact containing file output",
            ),
        ];
        let mut discovery = ToolDiscovery::default();
        let result: Value = serde_json::from_str(&discovery.search(
            &json!({"query":"inspect workspace file and prior evidence"}),
            &catalog,
        ))
        .unwrap();
        assert_eq!(result["loaded"], json!(["read_file"]));
        assert_eq!(discovery.loaded_count(), 1);
    }

    #[test]
    fn exact_recovery_tool_name_can_still_be_loaded() {
        let catalog = vec![tool(
            "project_memory_recall",
            "Recall project memory and prior task context",
        )];
        let mut discovery = ToolDiscovery::default();
        let result: Value = serde_json::from_str(
            &discovery.search(&json!({"query":"project_memory_recall"}), &catalog),
        )
        .unwrap();
        assert_eq!(result["loaded"], json!(["project_memory_recall"]));
    }

    #[test]
    fn implementation_surface_preloads_complete_inspect_edit_verify_abi() {
        let catalog = [
            "context_status",
            "search_artifact",
            "read_file",
            "apply_file_edits",
            "shell_job",
            "artifact_info",
            "search_workspace",
            "run_shell",
            "task_notes",
            "list_files",
            "context_history",
            "read_artifact",
        ]
        .into_iter()
        .map(|name| tool(name, name))
        .collect::<Vec<_>>();
        let discovery = ToolDiscovery::coding(true);
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
                "apply_file_edits",
                "run_shell",
            ]
        );
    }

    #[test]
    fn bounded_analysis_surface_stays_on_the_core_wire_surface() {
        let mut catalog = crate::tools::direct_mcp_tool_definitions();
        catalog.extend(super::super::context::tools());

        let full = ToolDiscovery::agent().attached(&catalog);
        let bounded = ToolDiscovery::bounded_analysis().attached(&catalog);
        let bounded_names = bounded
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            bounded_names,
            vec!["read_file", "search_workspace", "run_shell"]
        );
        assert_eq!(full, bounded);

        let full_bytes = serde_json::to_vec(&full).unwrap().len();
        let bounded_bytes = serde_json::to_vec(&bounded).unwrap().len();
        assert_eq!(bounded_bytes, full_bytes);
        eprintln!("bounded analysis tool-schema bytes: {full_bytes} -> {bounded_bytes} (stable)");
    }

    #[test]
    fn direct_file_lookup_surface_excludes_discovery_and_workspace_search() {
        let catalog = [
            "search_tools",
            "read_file",
            "search_workspace",
            "read_artifact",
            "run_shell",
        ]
        .into_iter()
        .map(|name| tool(name, name))
        .collect::<Vec<_>>();
        let names = ToolDiscovery::direct_file_lookup()
            .attached(&catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["run_shell", "read_file"]);
    }

    #[test]
    fn recovery_tools_promote_only_after_their_state_exists() {
        let catalog = [
            "read_file",
            "search_workspace",
            "run_shell",
            "shell_job",
            "read_artifact",
            "search_artifact",
        ]
        .into_iter()
        .map(|name| tool(name, name))
        .collect::<Vec<_>>();
        let mut discovery = ToolDiscovery::agent();
        assert_eq!(
            discovery
                .attached(&catalog)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>(),
            vec!["read_file", "search_workspace", "run_shell"]
        );

        let before_artifact: Value =
            serde_json::from_str(&discovery.search(&json!({"query":"search_artifact"}), &catalog))
                .unwrap();
        assert_eq!(before_artifact["loaded"], json!([]));

        discovery.load(["read_artifact"]);
        let after_artifact = discovery
            .attached(&catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(
            after_artifact,
            vec![
                "read_file",
                "search_workspace",
                "run_shell",
                "read_artifact"
            ]
        );
        assert!(!discovery.search_enabled());

        discovery.enable_search();
        assert!(
            discovery
                .attached(&catalog)
                .iter()
                .any(|tool| tool.name == SEARCH_TOOL)
        );
        let result: Value =
            serde_json::from_str(&discovery.search(&json!({"query":"search_artifact"}), &catalog))
                .unwrap();
        assert_eq!(result["loaded"], json!(["search_artifact"]));
    }

    #[test]
    fn provider_deferred_matches_return_references_without_promoting_schema() {
        let catalog = vec![
            tool("read_file", "Read source"),
            tool("mcp_krpc_observe", "Read kRPC telemetry"),
        ];
        let mut discovery = ToolDiscovery::default();
        let deferred = HashSet::from(["mcp_krpc_observe".to_owned()]);
        let result: Value = serde_json::from_str(&discovery.search_with_deferred(
            &json!({"query":"mcp_krpc_observe"}),
            &catalog,
            &deferred,
        ))
        .unwrap();
        assert_eq!(result["loaded"], json!([]));
        assert_eq!(result["deferred"], json!(["mcp_krpc_observe"]));
        assert_eq!(discovery.attached(&catalog).len(), 1);
    }

    #[test]
    fn exact_lists_load_all_matches_and_satisfied_keywords_do_not_expand_the_envelope() {
        let catalog: Vec<_> = (0..7)
            .map(|i| tool(&format!("tool_{i}"), "Read file"))
            .collect();
        let mut discovery = ToolDiscovery::default();
        let query = catalog
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let result: Value =
            serde_json::from_str(&discovery.search(&json!({"query":query}), &catalog)).unwrap();
        assert_eq!(result["loaded"].as_array().unwrap().len(), 7);
        assert_eq!(result["moreMatches"], false);
        let before = discovery.attached(&catalog);
        let result: Value =
            serde_json::from_str(&discovery.search(&json!({"query":"Read file"}), &catalog))
                .unwrap();
        assert_eq!(result["loaded"], json!([]));
        assert_eq!(result["alreadyLoaded"].as_array().unwrap().len(), 5);
        assert_eq!(result["moreMatches"], true);
        assert_eq!(discovery.attached(&catalog), before);
    }

    #[test]
    fn research_fuzzy_search_for_attached_web_tools_keeps_schema_envelope_stable() {
        let catalog = vec![
            tool("web_search", "Search the live web for current information"),
            tool("web_read", "Read a web search source"),
            tool("search_artifact", "Search a stored artifact"),
            tool("search_workspace", "Search workspace text"),
            tool(
                "activate_capability",
                "Activate a capability for more tools",
            ),
            tool("read_artifact", "Read stored artifact text"),
        ];
        let mut discovery = ToolDiscovery::research();
        let before = discovery.attached(&catalog);
        let result: Value = serde_json::from_str(&discovery.search(
            &json!({"query":"web search current news internet search"}),
            &catalog,
        ))
        .unwrap();
        assert_eq!(result["loaded"], json!([]));
        assert!(
            result["alreadyLoaded"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
        assert_eq!(discovery.attached(&catalog), before);
        assert!(!discovery.loaded.contains("search_workspace"));
        assert!(!discovery.loaded.contains("activate_capability"));
    }
    #[test]
    fn coding_tools_are_stable_and_respect_catalog_permissions() {
        let catalog = vec![
            tool("apply_file_edits", "Edit"),
            tool("mcp_browser", "Browse"),
            tool("read_file", "Read"),
        ];
        let mut discovery = ToolDiscovery::coding(false);
        let initial = discovery.attached(&catalog);
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].name, "read_file");
        discovery.search(&json!({"query":"apply_file_edits"}), &catalog);
        assert_eq!(discovery.attached(&catalog).len(), 2);
        assert_eq!(discovery.attached(&catalog[..1]).len(), 1);
        assert_eq!(discovery.attached(&[]).len(), 0);
    }

    #[test]
    fn registry_reordering_cannot_reorder_attached_schemas() {
        let first_catalog = vec![
            tool("search_workspace", "Search"),
            tool("read_file", "Read"),
        ];
        let second_catalog = vec![
            tool("read_file", "Read"),
            tool("search_workspace", "Search"),
        ];
        let discovery = ToolDiscovery::coding(false);
        let names = |catalog: &[ToolDefinition]| {
            discovery
                .attached(catalog)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };
        let expected = vec!["read_file", "search_workspace"];
        assert_eq!(names(&first_catalog), expected);
        assert_eq!(names(&second_catalog), expected);
    }

    #[test]
    fn newly_loaded_schema_is_appended_to_the_existing_cache_prefix() {
        let catalog = vec![tool("alpha_tool", "Alpha"), tool("omega_tool", "Omega")];
        let mut discovery = ToolDiscovery::default();
        discovery.search(&json!({"query":"omega_tool"}), &catalog);
        assert_eq!(
            discovery
                .attached(&catalog)
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["search_tools", "omega_tool"]
        );
        discovery.search(&json!({"query":"alpha_tool"}), &catalog);
        assert_eq!(
            discovery
                .attached(&catalog)
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["search_tools", "omega_tool", "alpha_tool"]
        );
    }

    #[test]
    fn preattached_web_research_surface_avoids_a1b3_promotion_and_preserves_wire_order() {
        let catalog = vec![
            tool("read_file", "Read"),
            tool("search_workspace", "Search"),
            tool("run_shell", "Shell"),
            tool("shell_job", "Poll shell"),
            tool("context_status", "Context"),
            tool("context_history", "History"),
            tool("task_notes", "Notes"),
            tool("artifact_info", "Artifact info"),
            tool("read_artifact", "Read artifact"),
            tool("search_artifact", "Search artifact"),
            tool("web_search", "Web search"),
            tool("web_read", "Web read"),
            tool("activate_capability", "Activate"),
        ];
        let mut discovery = ToolDiscovery::agent();
        assert_eq!(discovery.attached(&catalog).len(), 3);
        discovery.carry_web_research_surface();
        let preattached = discovery
            .attached(&catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(
            preattached,
            vec![
                "read_file",
                "search_workspace",
                "run_shell",
                "web_search",
                "web_read",
                "activate_capability",
            ]
        );
        let result: Value = serde_json::from_str(&discovery.search(
            &json!({"query":"web_search web_read activate_capability"}),
            &catalog,
        ))
        .unwrap();
        assert_eq!(result["loaded"], json!([]));
        assert_eq!(
            discovery
                .attached(&catalog)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>(),
            preattached
        );
    }

    #[test]
    fn general_agent_preloads_common_workspace_tools_without_discovery_churn() {
        let catalog = vec![
            tool("read_file", "Read"),
            tool("search_workspace", "Search"),
            tool("run_shell", "Shell"),
            tool("shell_job", "Poll shell"),
            tool("context_status", "Context"),
            tool("apply_file_edits", "Edit"),
            tool("read_artifact", "Artifact read"),
            tool("search_artifact", "Artifact search"),
            tool("context_history", "History"),
            tool("task_notes", "Notes"),
            tool("project_memory_recall", "Project memory"),
        ];
        let discovery = ToolDiscovery::agent();
        assert_eq!(
            discovery
                .attached(&catalog)
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["read_file", "search_workspace", "run_shell"]
        );
    }

    #[test]
    fn general_search_for_preloaded_tools_keeps_attachment_order_stable() {
        let catalog = vec![
            tool("read_file", "Read"),
            tool("list_files", "List"),
            tool("search_workspace", "Search"),
            tool("run_shell", "Shell"),
            tool("shell_job", "Jobs"),
            tool("context_status", "Status"),
        ];
        let mut discovery = ToolDiscovery::agent();
        let before = discovery
            .attached(&catalog)
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        discovery.search(&json!({"query":"workspace"}), &catalog);
        let after = discovery
            .attached(&catalog)
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(before, after);
    }

    #[test]
    fn coding_analysis_keeps_stable_execution_without_edit_schema() {
        let catalog = vec![
            tool("read_file", "Read"),
            tool("apply_file_edits", "Edit"),
            tool("run_shell", "Shell"),
        ];
        let mut discovery = ToolDiscovery::coding(false);
        assert_eq!(discovery.attached(&catalog).len(), 2);
        discovery.load(["apply_file_edits"]);
        assert_eq!(discovery.attached(&catalog).len(), 3);
        discovery.load(["run_shell"]);
        assert_eq!(discovery.attached(&catalog).len(), 3);
    }

    #[test]
    fn coding_implementation_preloads_the_common_edit_verify_bundle() {
        let catalog = vec![
            tool("run_shell", "Shell"),
            tool("read_file", "Read"),
            tool("apply_file_edits", "Edit"),
            tool("search_workspace", "Search"),
        ];
        let discovery = ToolDiscovery::coding(true);
        assert_eq!(
            discovery
                .attached(&catalog)
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "read_file",
                "search_workspace",
                "apply_file_edits",
                "run_shell",
            ]
        );
    }

    #[test]
    fn research_preloads_web_without_artifact_recovery() {
        let catalog = vec![
            tool("web_search", "Search"),
            tool("web_read", "Read source"),
            tool("read_artifact", "Read artifact"),
            tool("search_artifact", "Search artifact"),
        ];
        let mut discovery = ToolDiscovery::research();
        let attached = discovery.attached(&catalog);
        assert_eq!(attached.len(), 2);
        assert_eq!(attached[0].name, "web_search");
        assert_eq!(attached[1].name, "web_read");
        discovery.load(["web_read"]);
        assert_eq!(discovery.attached(&catalog).len(), 2);
    }

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition::new(
            name,
            description,
            json!({"type":"object","properties":{"payload":{"type":"string"}}}),
        )
    }
    #[test]
    fn schemas_load_on_demand_and_reset_for_new_turn() {
        let catalog = vec![
            tool("read_file", "Read source"),
            tool("run_shell", "Execute command"),
        ];
        let mut discovery = ToolDiscovery::default();
        assert_eq!(discovery.attached(&catalog).len(), 1);
        discovery.search(&json!({"query":"read_file"}), &catalog);
        let attached = discovery.attached(&catalog);
        assert_eq!(attached.len(), 2);
        assert_eq!(attached[1], catalog[0]);
        discovery.search(&json!({"query":"run_shell"}), &catalog);
        assert_eq!(discovery.attached(&catalog).len(), 3);
        assert_eq!(ToolDiscovery::default().attached(&catalog).len(), 1);
        // A removed/disabled tool cannot leak back through loaded state.
        assert_eq!(discovery.attached(&[]).len(), 1);
    }
    #[test]
    fn search_handles_invalid_missing_and_dynamic_tools() {
        let mut discovery = ToolDiscovery::default();
        assert!(
            discovery
                .search(&json!({"query":" "}), &[])
                .contains("error")
        );
        assert!(
            discovery
                .search(&json!({"query":"unknown"}), &[])
                .contains("\"loaded\":[]")
        );
        let catalog = vec![tool("mcp_browser", "Navigate website")];
        discovery.search(&json!({"query":"navigate website"}), &catalog);
        assert_eq!(discovery.attached(&catalog)[1], catalog[0]);
    }
    #[test]
    fn focused_fuzzy_search_is_bounded_and_exact_match_ranks_first() {
        let mut catalog: Vec<_> = (0..20)
            .map(|i| tool(&format!("tool_{i}"), "tool_19 keyword"))
            .collect();
        let mut discovery = ToolDiscovery::default();
        let result: Value =
            serde_json::from_str(&discovery.search(&json!({"query":"tool_19"}), &catalog)).unwrap();
        assert_eq!(result["loaded"][0], "tool_19");
        assert_eq!(result["loaded"].as_array().unwrap().len(), 1);
        let result: Value =
            serde_json::from_str(&discovery.search(&json!({"query":"tool"}), &catalog)).unwrap();
        assert_eq!(result["loaded"].as_array().unwrap().len(), 5);
        assert_eq!(result["moreMatches"], true);
        catalog.clear();
        assert_eq!(discovery.attached(&catalog).len(), 1);
    }
}
