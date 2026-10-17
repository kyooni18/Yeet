//! Request-local schema loading. Catalogs are already filtered by task policy.
use crate::core::ToolDefinition;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(super) const SEARCH_TOOL: &str = "search_tools";

/// The basic surface every turn starts with, in a fixed order so the provider
/// sees one stable tool prefix. Web tools drop out when the catalog lacks them
/// (web search disabled); everything else stays reachable through search_tools.
const BASIC_TOOLS: [&str; 8] = [
    "read_file",
    "search_workspace",
    "apply_file_edits",
    "run_shell",
    "shell_job",
    "read_artifact",
    "web_search",
    "web_read",
];

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
    /// Every turn gets the same basic tools plus the search_tools gateway.
    /// Request wording never narrows this surface; it can only add tools.
    pub fn basic() -> Self {
        let mut discovery = Self::default();
        discovery.load(BASIC_TOOLS);
        discovery
    }

    pub fn search_enabled(&self) -> bool {
        self.search_enabled
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

    fn names(discovery: &ToolDiscovery, catalog: &[ToolDefinition]) -> Vec<String> {
        discovery
            .attached(catalog)
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    #[test]
    fn basic_surface_attaches_core_tools_and_discovery_up_front() {
        let catalog: Vec<_> = BASIC_TOOLS.iter().map(|name| tool(name)).collect();
        let mut expected: Vec<String> = BASIC_TOOLS.iter().map(|name| name.to_string()).collect();
        expected.push(SEARCH_TOOL.to_owned());
        let discovery = ToolDiscovery::basic();
        assert_eq!(names(&discovery, &catalog), expected);

        // Web tools disappear only when the catalog lacks them.
        let offline: Vec<_> = catalog
            .into_iter()
            .filter(|tool| !tool.name.starts_with("web_"))
            .collect();
        assert!(!names(&discovery, &offline).iter().any(|name| name.starts_with("web_")));
    }

    #[test]
    fn warm_surface_keeps_order_across_turns() {
        let catalog = vec![tool("read_file"), tool("outline"), tool("run_shell")];
        let mut previous = ToolDiscovery::basic();
        previous.load(["outline"]);
        let submitted = previous.attached(&catalog);
        let mut next = ToolDiscovery::basic();
        next.restore_warm_surface(previous.loaded_names(), previous.search_enabled());
        assert_eq!(next.attached(&catalog), submitted);
    }
}
