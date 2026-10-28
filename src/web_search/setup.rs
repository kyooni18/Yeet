//! Web Search health, setup planning, and provider connection tests.

use super::*;

/// A provider's installation and connectivity are deliberately separate: a
/// detected executable or a responding configuration endpoint is not a search test.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub installed: Option<bool>,
    pub connectivity: Connectivity,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Connectivity {
    Unknown,
    Connected,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchStatus {
    pub backend: ServiceBackend,
    pub server: String,
    pub selected_provider: Option<String>,
    pub fallback_available: Option<bool>,
    pub agent_reach: ProviderHealth,
    pub searxng: ProviderHealth,
    pub searxng_url: String,
    pub mcp: Option<ProviderHealth>,
    pub path: PathBuf,
}

/// Inspect local installations and configured connections without installing or
/// starting a provider. Connectivity is only known where it was actually checked.
pub fn web_search_status(
    settings: &crate::project_settings::WebBackendSettings,
) -> WebSearchStatus {
    web_search_status_for_workspace(settings, None)
}

pub fn web_search_status_for_workspace(
    settings: &crate::project_settings::WebBackendSettings,
    workspace: Option<&Path>,
) -> WebSearchStatus {
    let paths = SearchPaths::discover();
    let client = WebSearchClient::default();
    let url = settings
        .searxng_url
        .clone()
        .or_else(|| env::var("YEET_SEARXNG_URL").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_URL.into());
    let running = client.probe(&url);
    let agent_installed = client.agent_reach_invocation().is_some();
    let searx_installed = paths.installed();
    let mcp = if settings.backend == ServiceBackend::Mcp {
        let result = start_search_bridge(workspace).and_then(|bridge| {
            let result = bridge.list_mcp_tools(Some(&settings.server));
            bridge.shutdown();
            let tools = result?;
            let missing = missing_web_tools(tools.iter().map(|tool| tool.name.as_str()));
            if !missing.is_empty() {
                bail!(
                    "MCP server is missing required tools: {}",
                    missing.join(", ")
                );
            }
            Ok(())
        });
        Some(ProviderHealth {
            installed: None,
            connectivity: if result.is_ok() {
                Connectivity::Connected
            } else {
                Connectivity::Unavailable
            },
            error: result.err().map(|e| format!("{e:#}")),
        })
    } else {
        None
    };
    WebSearchStatus {
        backend: settings.backend,
        server: settings.server.clone(),
        selected_provider: if settings.backend == ServiceBackend::Mcp {
            Some("mcp".into())
        } else if settings.backend == ServiceBackend::AgentReach
            || (settings.backend == ServiceBackend::Builtin && agent_installed)
        {
            Some("agent-reach".into())
        } else if settings.backend == ServiceBackend::Searxng || running || searx_installed {
            Some("searxng".into())
        } else {
            None
        },
        fallback_available: if settings.backend != ServiceBackend::Builtin {
            Some(false)
        } else if running || searx_installed {
            None
        } else {
            Some(false)
        },
        agent_reach: ProviderHealth {
            installed: Some(agent_installed),
            connectivity: Connectivity::Unknown,
            error: if agent_installed {
                None
            } else {
                Some("Install Agent-Reach or configure another search provider.".into())
            },
        },
        searxng: ProviderHealth {
            installed: Some(searx_installed),
            connectivity: if running {
                Connectivity::Connected
            } else {
                Connectivity::Unavailable
            },
            error: if running {
                None
            } else if searx_installed {
                Some("SearXNG is installed but not running; it will start on demand.".into())
            } else {
                Some("No SearXNG instance responded; install it or configure an endpoint.".into())
            },
        },
        searxng_url: url,
        mcp,
        path: paths.root,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchTestResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupPlan {
    pub install: Option<String>,
    pub missing_dependencies: Vec<String>,
    pub message: String,
}

/// Plan setup without modifying the system or contacting an installer.
pub fn setup_plan(
    settings: &crate::project_settings::WebBackendSettings,
    workspace: Option<&Path>,
) -> SetupPlan {
    setup_plan_inner(settings, workspace, false)
}

pub fn repair_plan(
    settings: &crate::project_settings::WebBackendSettings,
    workspace: Option<&Path>,
) -> SetupPlan {
    setup_plan_inner(settings, workspace, true)
}

fn setup_plan_inner(
    settings: &crate::project_settings::WebBackendSettings,
    workspace: Option<&Path>,
    repair: bool,
) -> SetupPlan {
    let status = web_search_status_for_workspace(settings, workspace);
    if settings.backend == ServiceBackend::Mcp {
        return SetupPlan {
            install: None,
            missing_dependencies: vec![],
            message: "Test the selected MCP server; Yeet does not install MCP servers.".into(),
        };
    }
    if repair && settings.searxng_url.is_some() && settings.backend == ServiceBackend::Searxng {
        return SetupPlan { install: None, missing_dependencies: vec![], message: "The configured SearXNG endpoint failed. Check the endpoint and its JSON search configuration.".into() };
    }
    if !repair
        && match settings.backend {
            ServiceBackend::AgentReach => status.agent_reach.installed == Some(true),
            ServiceBackend::Searxng => {
                status.searxng.installed == Some(true)
                    || status.searxng.connectivity == Connectivity::Connected
            }
            ServiceBackend::Builtin => {
                status.agent_reach.installed == Some(true)
                    || status.searxng.installed == Some(true)
                    || status.searxng.connectivity == Connectivity::Connected
            }
            ServiceBackend::Mcp => false,
        }
    {
        return SetupPlan {
            install: None,
            missing_dependencies: vec![],
            message: "An existing provider was detected. Test its connection.".into(),
        };
    }
    let target = if settings.backend == ServiceBackend::Searxng
        || (settings.backend == ServiceBackend::Builtin
            && (!command_available("node")
                || !command_available(&env::var("YEET_NPM").unwrap_or_else(|_| "npm".into())))
            && command_available("git"))
    {
        "searxng"
    } else {
        "agent-reach"
    };
    let mut missing = Vec::new();
    let python = env::var("YEET_PYTHON").unwrap_or_else(|_| "python3".into());
    if !command_available(&python) {
        missing.push("Python 3".into());
    }
    if target == "agent-reach" {
        let npm = env::var("YEET_NPM").unwrap_or_else(|_| "npm".into());
        if !command_available("node") {
            missing.push("Node.js".into());
        }
        if !command_available(&npm) {
            missing.push("npm".into());
        }
    } else if !command_available("git") {
        missing.push("Git".into());
    }
    SetupPlan {
        install: Some(target.into()),
        missing_dependencies: missing,
        message: format!(
            "Install {target} using the existing Yeet installer, then run a search test."
        ),
    }
}

pub fn missing_web_tools<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let names = names.into_iter().collect::<Vec<_>>();
    ["web_search", "web_read"]
        .into_iter()
        .filter(|name| !names.contains(name))
        .collect()
}

fn start_search_bridge(workspace: Option<&Path>) -> Result<BridgeClient> {
    match workspace {
        Some(path) => BridgeClient::start_for_workspace(path),
        None => BridgeClient::start(),
    }
}

/// Run a real search with the configured provider. No installation is performed.
pub fn test_web_search(
    settings: &crate::project_settings::WebBackendSettings,
    workspace: Option<&Path>,
) -> Result<String> {
    if settings.backend == ServiceBackend::Mcp {
        let bridge = start_search_bridge(workspace)?;
        let result = (|| {
            let tools = bridge.list_mcp_tools(Some(&settings.server))?;
            let missing = missing_web_tools(tools.iter().map(|tool| tool.name.as_str()));
            if !missing.is_empty() {
                bail!(
                    "MCP server {} is missing {}",
                    settings.server,
                    missing.join(", ")
                );
            }
            let args = json!({"query":"Yeet web search connection test", "maxResults":1})
                .as_object()
                .unwrap()
                .clone();
            let value = bridge.call_mcp_tool(&settings.server, "web_search", &args)?;
            if value.get("isError").and_then(Value::as_bool) == Some(true) {
                bail!("MCP search test failed: {value}");
            }
            if !value
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|content| !content.is_empty())
                && value.get("structuredContent").is_none()
            {
                bail!("MCP search test returned no content");
            }
            Ok(format!(
                "MCP server {} answered the search test.",
                settings.server
            ))
        })();
        bridge.shutdown();
        return result;
    }
    let client = WebSearchClient::default();
    client.set_searxng_url(settings.searxng_url.clone());
    let backend = match settings.backend {
        ServiceBackend::AgentReach => WebSearchBackend::AgentReach,
        ServiceBackend::Searxng => WebSearchBackend::Searxng,
        _ => WebSearchBackend::Auto,
    };
    let result = client.search(WebSearchRequest {
        query: "Yeet web search connection test",
        max_results: 1,
        language: None,
        category: None,
        time_range: None,
        safe_search: None,
        page: None,
        backend,
    })?;
    Ok(format!(
        "Search test succeeded using {}{}.",
        result
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("the configured provider"),
        result
            .get("fallbackReason")
            .and_then(Value::as_str)
            .map(|s| format!(" (fallback: {s})"))
            .unwrap_or_default()
    ))
}

/// Invoke the CLI's installer after the caller has obtained user approval.
pub fn install_web_search(target: &str) -> Result<String> {
    install(target)
}
