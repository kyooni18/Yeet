//! One authoritative row per built-in tool.
//!
//! Everything about a tool that is not its JSON schema or its execution lives
//! here: who may run it, whether every turn starts with it, which capability
//! toggle owns it, and how every frontend names it. The catalog, tool
//! discovery, progress events and the shared UI projection read this table
//! instead of matching tool names themselves. Schemas stay in `definitions`
//! and execution in `ToolRegistry::execute`.

/// Semantic family. Frontends choose one icon per kind, never per tool name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Shell,
    Edit,
    Read,
    Search,
    Listing,
    Web,
    Data,
    Screen,
    Capability,
    Memory,
    Question,
    Context,
    Mcp,
    Skill,
    Worker,
    Other,
}

/// What a call can affect. Scoped agents are restricted by class, not by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAccess {
    /// Inspects state only.
    Read,
    /// Mutates workspace files or durable memory; blocked by `deny_writes`.
    Write,
    /// Runs or controls processes; blocked by `deny_shell`.
    Shell,
    /// Talks to the runtime, the user or the desktop; gated elsewhere.
    Runtime,
}

/// The four phrases a frontend may show for a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolLabels {
    /// Noun for a transcript row: "Command".
    pub title: &'static str,
    /// While running: "Running command".
    pub active: &'static str,
    /// After success: "Ran command".
    pub done: &'static str,
    /// After failure: "Command failed".
    pub failed: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub kind: ToolKind,
    pub access: ToolAccess,
    /// Attached to every turn. Row order among basic tools is the provider's
    /// stable tool prefix; append, never reorder.
    pub basic: bool,
    /// Callable from read-only research.
    pub research: bool,
    /// Settings capability that can disable the tool.
    pub capability: Option<&'static str>,
    pub labels: ToolLabels,
}

const fn labels(
    title: &'static str,
    active: &'static str,
    done: &'static str,
    failed: &'static str,
) -> ToolLabels {
    ToolLabels {
        title,
        active,
        done,
        failed,
    }
}

const fn spec(
    name: &'static str,
    kind: ToolKind,
    access: ToolAccess,
    capability: Option<&'static str>,
    labels: ToolLabels,
) -> ToolSpec {
    ToolSpec {
        name,
        kind,
        access,
        basic: false,
        research: false,
        capability,
        labels,
    }
}

impl ToolSpec {
    const fn basic(mut self) -> Self {
        self.basic = true;
        self
    }
    const fn research(mut self) -> Self {
        self.research = true;
        self
    }
}

use ToolAccess::{Read, Runtime, Shell, Write};

const FILE_READ: Option<&str> = Some("builtin:file-read");
const SHELL: Option<&str> = Some("builtin:shell");
const MEMORY: Option<&str> = Some("builtin:memory");
const SYMBOLS: Option<&str> = Some("builtin:code-symbols");
const COMPUTER: Option<&str> = Some("builtin:computer-use");

const READ_FILE: ToolLabels = labels("Read file", "Reading file", "Read file", "File read failed");
const SCREEN: ToolLabels = labels(
    "Screen control",
    "Using computer",
    "Used computer",
    "Computer use failed",
);

/// Basic tools first, in their provider-visible order.
const TOOL_SPECS: &[ToolSpec] = &[
    spec("read_file", ToolKind::Read, Read, FILE_READ, READ_FILE)
        .basic()
        .research(),
    spec(
        "search_workspace",
        ToolKind::Search,
        Read,
        Some("builtin:workspace-search"),
        labels(
            "Code search",
            "Searching workspace",
            "Searched workspace",
            "Workspace search failed",
        ),
    )
    .basic()
    .research(),
    spec(
        "apply_file_edits",
        ToolKind::Edit,
        Write,
        Some("builtin:file-write"),
        labels(
            "Edit files",
            "Editing files",
            "Edited files",
            "File edits failed",
        ),
    )
    .basic(),
    spec(
        "run_shell",
        ToolKind::Shell,
        Shell,
        SHELL,
        labels(
            "Command",
            "Running command",
            "Ran command",
            "Command failed",
        ),
    )
    .basic(),
    spec(
        "shell_job",
        ToolKind::Shell,
        Shell,
        SHELL,
        labels(
            "Command",
            "Checking command",
            "Checked command",
            "Command check failed",
        ),
    )
    .basic(),
    spec(
        "read_artifact",
        ToolKind::Read,
        Read,
        None,
        labels(
            "Read output",
            "Reading output",
            "Read output",
            "Output read failed",
        ),
    )
    .basic()
    .research(),
    spec(
        "web_search",
        ToolKind::Web,
        Read,
        None,
        labels(
            "Web search",
            "Searching web",
            "Searched web",
            "Web search failed",
        ),
    )
    .basic()
    .research(),
    spec(
        "web_read",
        ToolKind::Web,
        Read,
        None,
        labels(
            "Web page",
            "Reading web page",
            "Read web page",
            "Web page read failed",
        ),
    )
    .basic()
    .research(),
    spec(
        "ask_user",
        ToolKind::Question,
        Runtime,
        None,
        labels("Question", "Asking you", "Asked you", "Question failed"),
    )
    .basic(),
    // Reached through search_tools.
    spec(
        "search_tools",
        ToolKind::Capability,
        Runtime,
        None,
        labels(
            "Tool search",
            "Finding tools",
            "Found tools",
            "Tool search failed",
        ),
    ),
    spec(
        "find_capabilities",
        ToolKind::Capability,
        Runtime,
        None,
        labels(
            "Capability search",
            "Finding capabilities",
            "Found capabilities",
            "Capability search failed",
        ),
    )
    .research(),
    spec(
        "activate_capability",
        ToolKind::Capability,
        Runtime,
        None,
        labels(
            "Capability",
            "Activating capability",
            "Activated capability",
            "Capability activation failed",
        ),
    )
    .research(),
    spec(
        "outline",
        ToolKind::Read,
        Read,
        SYMBOLS,
        labels(
            "Outline",
            "Outlining file",
            "Outlined file",
            "Outline failed",
        ),
    ),
    spec(
        "find_symbol",
        ToolKind::Search,
        Read,
        SYMBOLS,
        labels(
            "Symbol search",
            "Finding symbol",
            "Found symbol",
            "Symbol search failed",
        ),
    ),
    spec(
        "read_document",
        ToolKind::Read,
        Read,
        Some("builtin:document-read"),
        labels(
            "Read document",
            "Reading document",
            "Read document",
            "Document read failed",
        ),
    )
    .research(),
    spec(
        "analyze_data",
        ToolKind::Data,
        Read,
        Some("builtin:data-analysis"),
        labels(
            "Data analysis",
            "Analyzing data",
            "Analyzed data",
            "Data analysis failed",
        ),
    )
    .research(),
    spec(
        "artifact_info",
        ToolKind::Read,
        Read,
        None,
        labels(
            "Output info",
            "Inspecting output",
            "Inspected output",
            "Output inspection failed",
        ),
    )
    .research(),
    spec(
        "search_artifact",
        ToolKind::Search,
        Read,
        None,
        labels(
            "Output search",
            "Searching output",
            "Searched output",
            "Output search failed",
        ),
    )
    .research(),
    spec(
        "request_shell_permission",
        ToolKind::Shell,
        Shell,
        SHELL,
        labels(
            "Shell permission",
            "Requesting permission",
            "Requested permission",
            "Permission request failed",
        ),
    ),
    spec(
        "memory_save",
        ToolKind::Memory,
        Write,
        MEMORY,
        labels(
            "Memory",
            "Saving memory",
            "Saved memory",
            "Memory save failed",
        ),
    ),
    spec(
        "memory_search",
        ToolKind::Memory,
        Read,
        MEMORY,
        labels(
            "Memory",
            "Searching memory",
            "Searched memory",
            "Memory search failed",
        ),
    ),
    spec(
        "memory_get",
        ToolKind::Memory,
        Read,
        MEMORY,
        labels(
            "Memory",
            "Reading memory",
            "Read memory",
            "Memory read failed",
        ),
    ),
    spec(
        "memory_forget",
        ToolKind::Memory,
        Write,
        MEMORY,
        labels(
            "Memory",
            "Forgetting memory",
            "Forgot memory",
            "Memory forget failed",
        ),
    ),
    spec("computer_use", ToolKind::Screen, Runtime, COMPUTER, SCREEN),
    spec(
        "computer_use_reset",
        ToolKind::Screen,
        Runtime,
        COMPUTER,
        labels(
            "Screen control",
            "Resetting computer session",
            "Reset computer session",
            "Computer reset failed",
        ),
    ),
    spec(
        "context_status",
        ToolKind::Context,
        Runtime,
        None,
        labels(
            "Context",
            "Checking context",
            "Checked context",
            "Context check failed",
        ),
    ),
    spec(
        "new_context",
        ToolKind::Context,
        Runtime,
        None,
        labels(
            "Context",
            "Starting new context",
            "Started new context",
            "Context rollover failed",
        ),
    ),
    spec(
        "context_history",
        ToolKind::Context,
        Runtime,
        None,
        labels(
            "Context history",
            "Reading context",
            "Read context",
            "Context read failed",
        ),
    ),
    // Names kept only so restored sessions execute and render as before.
    spec("read_files", ToolKind::Read, Read, FILE_READ, READ_FILE),
    spec(
        "list_files",
        ToolKind::Listing,
        Read,
        None,
        labels(
            "File list",
            "Listing files",
            "Listed files",
            "File listing failed",
        ),
    )
    .research(),
    spec("desktop_control", ToolKind::Screen, Runtime, None, SCREEN),
];

const MCP: ToolLabels = labels("MCP", "Using MCP", "Used MCP", "MCP call failed");
const SKILL: ToolLabels = labels("Skill", "Reading skill", "Read skill", "Skill failed");
const WORKER: ToolLabels = labels("Worker", "Running worker", "Ran worker", "Worker failed");
const OTHER: ToolLabels = labels("Tool", "Running", "Tool completed", "Tool failed");

pub fn tool_specs() -> &'static [ToolSpec] {
    TOOL_SPECS
}

/// The spec of a built-in tool; `None` for Skill, MCP, Worker and group tools.
pub fn tool_spec(name: &str) -> Option<&'static ToolSpec> {
    TOOL_SPECS.iter().find(|spec| spec.name == name)
}

/// Names attached to every turn, in provider-visible order.
pub fn basic_tool_names() -> impl Iterator<Item = &'static str> {
    TOOL_SPECS
        .iter()
        .filter(|spec| spec.basic)
        .map(|spec| spec.name)
}

/// Kind and labels for any tool name, including activated external tools.
/// A provider namespace (`functions.read_file`) does not change the tool.
pub fn tool_presentation(name: &str) -> (ToolKind, &'static ToolLabels) {
    let name = name.trim();
    let name = name.rsplit('.').next().unwrap_or(name);
    if let Some(spec) = tool_spec(name) {
        (spec.kind, &spec.labels)
    } else if name.starts_with("mcp_") {
        (ToolKind::Mcp, &MCP)
    } else if name.starts_with("skill_") {
        (ToolKind::Skill, &SKILL)
    } else if name.starts_with("worker_") {
        (ToolKind::Worker, &WORKER)
    } else {
        (ToolKind::Other, &OTHER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_surface_order_and_policy_classes_are_pinned() {
        // Provider prompt-cache prefix: changing this order invalidates caches.
        assert_eq!(
            basic_tool_names().collect::<Vec<_>>(),
            [
                "read_file",
                "search_workspace",
                "apply_file_edits",
                "run_shell",
                "shell_job",
                "read_artifact",
                "web_search",
                "web_read",
                "ask_user",
            ]
        );
        let names = |access| {
            TOOL_SPECS
                .iter()
                .filter(|spec| spec.access == access)
                .map(|spec| spec.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(Write),
            ["apply_file_edits", "memory_save", "memory_forget"]
        );
        assert_eq!(
            names(Shell),
            ["run_shell", "shell_job", "request_shell_permission"]
        );
        // No mutating or process tool is reachable from research.
        assert!(
            TOOL_SPECS
                .iter()
                .all(|spec| !spec.research || matches!(spec.access, Read | Runtime))
        );
    }

    #[test]
    fn every_model_visible_builtin_has_exactly_one_spec() {
        let mut seen = std::collections::HashSet::new();
        assert!(TOOL_SPECS.iter().all(|spec| seen.insert(spec.name)));
        let mut definitions = super::super::definitions::base_tool_definitions();
        definitions.push(super::super::definitions::ask_user_tool_definition());
        definitions.push(super::super::definitions::web_search_tool_definition());
        definitions.push(super::super::definitions::web_read_tool_definition());
        for tool in definitions {
            assert!(tool_spec(&tool.name).is_some(), "{} has no spec", tool.name);
        }
        for capability in super::super::builtin_capabilities() {
            assert!(
                TOOL_SPECS
                    .iter()
                    .any(|spec| spec.capability == Some(capability.id)),
                "{} owns no tool",
                capability.id
            );
        }
    }

    #[test]
    fn external_and_namespaced_names_resolve_to_a_presentation() {
        assert_eq!(
            tool_presentation("functions.run_shell").1.done,
            "Ran command"
        );
        assert_eq!(tool_presentation("mcp_docs_search").0, ToolKind::Mcp);
        assert_eq!(tool_presentation("skill_review").1.title, "Skill");
        assert_eq!(tool_presentation("propose_plan").0, ToolKind::Other);
    }
}
