//! Request-local schema loading. Catalogs are already filtered by task policy.
use crate::core::ToolDefinition;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(super) const SEARCH_TOOL: &str = "search_tools";

#[derive(Default)]
pub(super) struct ToolDiscovery {
    loaded: HashSet<String>,
    // Prompt-cache ABI for this turn. The registry catalog may be backed by
    // maps or extension/server enumeration whose order can change even when
    // schemas do not. Preserve first attachment order and append later tools.
    loaded_order: Vec<String>,
}

impl ToolDiscovery {
    /// Keep only small, high-frequency inspection schemas on the first coding request.
    /// Expensive mutation/shell/artifact schemas are promoted as the turn needs them.
    pub fn coding() -> Self {
        let mut discovery = Self::default();
        // Keep this deliberately ordered. Existing entries should not be
        // reshuffled when the preferred coding surface evolves; append new
        // preloaded tools instead so old cache prefixes stay reusable.
        discovery.load(["read_file", "list_files", "search_workspace"]);
        discovery
    }

    /// Research almost always starts with search, so avoid a discovery-only round.
    pub fn research() -> Self {
        let mut discovery = Self::default();
        discovery.load(["web_search"]);
        discovery
    }

    pub fn load(&mut self, names: impl IntoIterator<Item = &'static str>) {
        for name in names {
            self.load_name(name);
        }
    }

    fn load_name(&mut self, name: &str) {
        if self.loaded.insert(name.to_owned()) {
            self.loaded_order.push(name.to_owned());
        }
    }

    pub fn attached(&self, catalog: &[ToolDefinition]) -> Vec<ToolDefinition> {
        let mut tools = vec![ToolDefinition::new(
            SEARCH_TOOL,
            "Load missing tool schemas by exact names (comma-separated) or keywords. Loaded tools are callable next request and stay loaded this turn. For Skills/MCP/Workers, load find_capabilities and activate_capability first.",
            json!({"type":"object","properties":{"query":{"type":"string","minLength":1}},"required":["query"],"additionalProperties":false}),
        )];
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
                let score = if name == query || (exact_list && words.contains(name.as_str())) {
                    usize::MAX
                } else {
                    words
                        .iter()
                        .map(|word| {
                            if name.contains(word) {
                                10
                            } else if description.contains(word) {
                                1
                            } else {
                                0
                            }
                        })
                        .sum()
                };
                (score > 0).then_some((score, tool))
            })
            .collect();
        matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        if !exact_list
            && matches
                .first()
                .is_some_and(|(score, _)| *score == usize::MAX)
        {
            matches.truncate(1);
        }
        let total = matches.len();
        matches.truncate(5);
        for (_, tool) in &matches {
            self.load_name(&tool.name);
        }
        json!({
            "loaded": matches.iter().map(|(_, t)| &t.name).collect::<Vec<_>>(),
            "moreMatches": total > matches.len()
        })
        .to_string()
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
    fn exact_lists_remain_bounded_and_keywords_still_work() {
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
        assert_eq!(result["loaded"].as_array().unwrap().len(), 5);
        assert_eq!(result["moreMatches"], true);
        let result: Value =
            serde_json::from_str(&discovery.search(&json!({"query":"Read file"}), &catalog))
                .unwrap();
        assert_eq!(result["loaded"].as_array().unwrap().len(), 5);
    }
    #[test]
    fn coding_tools_are_stable_and_respect_catalog_permissions() {
        let catalog = vec![
            tool("apply_file_edits", "Edit"),
            tool("mcp_browser", "Browse"),
            tool("read_file", "Read"),
        ];
        let mut discovery = ToolDiscovery::coding();
        let initial = discovery.attached(&catalog);
        assert_eq!(initial.len(), 2);
        assert_eq!(initial[1].name, "read_file");
        discovery.search(&json!({"query":"apply_file_edits"}), &catalog);
        assert_eq!(discovery.attached(&catalog).len(), 3);
        assert_eq!(discovery.attached(&catalog[..1]).len(), 2);
        assert_eq!(discovery.attached(&[]).len(), 1);
    }

    #[test]
    fn registry_reordering_cannot_reorder_attached_schemas() {
        let first_catalog = vec![
            tool("search_workspace", "Search"),
            tool("read_file", "Read"),
            tool("list_files", "List"),
        ];
        let second_catalog = vec![
            tool("list_files", "List"),
            tool("read_file", "Read"),
            tool("search_workspace", "Search"),
        ];
        let discovery = ToolDiscovery::coding();
        let names = |catalog: &[ToolDefinition]| {
            discovery
                .attached(catalog)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };
        let expected = vec![
            "search_tools",
            "read_file",
            "list_files",
            "search_workspace",
        ];
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
    fn coding_can_promote_expensive_tools_without_a_search_round() {
        let catalog = vec![
            tool("read_file", "Read"),
            tool("apply_file_edits", "Edit"),
            tool("run_shell", "Shell"),
        ];
        let mut discovery = ToolDiscovery::coding();
        assert_eq!(discovery.attached(&catalog).len(), 2);
        discovery.load(["apply_file_edits"]);
        assert_eq!(discovery.attached(&catalog).len(), 3);
        discovery.load(["run_shell"]);
        assert_eq!(discovery.attached(&catalog).len(), 4);
    }

    #[test]
    fn research_preloads_search_only() {
        let catalog = vec![
            tool("web_search", "Search"),
            tool("web_read", "Read source"),
            tool("read_artifact", "Read artifact"),
        ];
        let mut discovery = ToolDiscovery::research();
        let attached = discovery.attached(&catalog);
        assert_eq!(attached.len(), 2);
        assert_eq!(attached[1].name, "web_search");
        discovery.load(["web_read"]);
        assert_eq!(discovery.attached(&catalog).len(), 3);
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
        discovery.search(&json!({"query":"website"}), &catalog);
        assert_eq!(discovery.attached(&catalog)[1], catalog[0]);
    }
    #[test]
    fn focused_search_is_bounded_and_exact_match_ranks_first() {
        let mut catalog: Vec<_> = (0..20)
            .map(|i| tool(&format!("tool_{i}"), "tool_19 keyword"))
            .collect();
        let mut discovery = ToolDiscovery::default();
        let result: Value =
            serde_json::from_str(&discovery.search(&json!({"query":"tool_19"}), &catalog)).unwrap();
        assert_eq!(result["loaded"][0], "tool_19");
        assert_eq!(result["loaded"].as_array().unwrap().len(), 1);
        let result: Value =
            serde_json::from_str(&discovery.search(&json!({"query":"keyword"}), &catalog)).unwrap();
        assert_eq!(result["loaded"].as_array().unwrap().len(), 5);
        assert_eq!(result["moreMatches"], true);
        catalog.clear();
        assert_eq!(discovery.attached(&catalog).len(), 1);
    }
}
