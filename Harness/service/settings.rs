//! Authentication, provider, project-setting, and sandbox commands.
//!
//! These operations mutate configuration around a session but do not execute
//! an agent turn, so they live outside the backend runtime dispatcher.

use super::*;

impl HarnessService {
    /// Refreshes the authentication providers exposed by the runtime bridge.
    pub(super) fn request_auth(&self) {
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.auth_working = true;
            shared.state.auth_notice = None;
        }
        self.publish_state();
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = load_auth_providers(&bridge);
            if let Ok(mut state) = shared.lock() {
                state.state.auth_working = false;
                match result {
                    Ok(items) => state.state.auth_providers = items,
                    Err(error) => {
                        state.state.auth_notice =
                            Some(format!("Unable to load authentication: {error}"))
                    }
                }
                let _ = tx.send(ServiceEvent::Envelope(state_envelope(&state.state)));
            }
        });
    }

    /// Starts browser-based sign-in for one provider.
    pub(super) fn auth_login(&self, provider: String) {
        self.start_auth_action(format!("Signing in to {provider}…"));
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = bridge
                .login_browser(&provider, auth_login_options(&provider))
                .map(|_| format!("Signed in to {provider}."));
            finish_auth_action(&bridge, &shared, &tx, result);
        });
    }

    /// Signs out from one provider and refreshes authentication state.
    pub(super) fn auth_logout(&self, provider: String) {
        self.start_auth_action(format!("Signing out of {provider}…"));
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = bridge
                .logout(&provider)
                .map(|_| format!("Signed out of {provider}."));
            finish_auth_action(&bridge, &shared, &tx, result);
        });
    }

    /// Stores a provider API key through the runtime bridge.
    pub(super) fn auth_set_api_key(&self, provider: String, key: String) {
        self.start_auth_action(format!("Saving API key for {provider}…"));
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = if key.trim().is_empty() {
                Err(anyhow!("API key cannot be empty"))
            } else {
                bridge
                    .set_api_key(&provider, key.trim())
                    .map(|_| format!("Saved API key for {provider}."))
            };
            finish_auth_action(&bridge, &shared, &tx, result);
        });
    }

    /// Marks authentication UI state busy before an asynchronous operation.
    fn start_auth_action(&self, message: String) {
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.auth_working = true;
            shared.state.auth_notice = Some(message);
        }
        self.publish_state();
    }

    /// Refreshes custom provider configurations.
    pub(super) fn request_providers(&self) {
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.providers_working = true;
            shared.state.providers_notice = None;
        }
        self.publish_state();
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = load_provider_configurations(&bridge);
            if let Ok(mut state) = shared.lock() {
                state.state.providers_working = false;
                match result {
                    Ok(items) => state.state.provider_configurations = items,
                    Err(error) => {
                        state.state.providers_notice =
                            Some(format!("Unable to load providers: {error}"))
                    }
                }
                let _ = tx.send(ServiceEvent::Envelope(state_envelope(&state.state)));
            }
        });
    }

    /// Creates or updates an OpenAI-compatible provider configuration.
    pub(super) fn save_provider(&self, id: String, base_url: String, require_api_key: bool) {
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.providers_working = true;
            shared.state.providers_notice = Some(format!("Saving {id}…"));
        }
        self.publish_state();
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = (|| -> Result<String> {
                let id = id.trim();
                let base_url = base_url.trim();
                if id.is_empty() {
                    return Err(anyhow!("Provider ID cannot be empty"));
                }
                if base_url.is_empty() {
                    return Err(anyhow!("Base URL cannot be empty"));
                }
                let mut provider = bridge
                    .list_provider_configurations()?
                    .into_iter()
                    .find(|provider| provider.id == id)
                    .unwrap_or(crate::core::OpenAiCompatibleProvider {
                        kind: "openai-compatible".into(),
                        id: id.into(),
                        base_url: base_url.into(),
                        api_key: None,
                        headers: None,
                        require_api_key: Some(require_api_key),
                    });
                provider.id = id.into();
                provider.base_url = base_url.into();
                provider.require_api_key = Some(require_api_key);
                bridge.save_provider_configuration(&provider)?;
                Ok(format!("Saved provider {id}."))
            })();
            finish_provider_action(&bridge, &shared, &tx, result);
        });
    }

    /// Removes a custom provider configuration.
    pub(super) fn remove_provider(&self, id: String) {
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.providers_working = true;
            shared.state.providers_notice = Some(format!("Removing {id}…"));
        }
        self.publish_state();
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = bridge
                .remove_provider_configuration(&id)
                .and_then(|removed| {
                    if removed {
                        Ok(format!("Removed provider {id}."))
                    } else {
                        Err(anyhow!("Provider {id} was not found"))
                    }
                });
            finish_provider_action(&bridge, &shared, &tx, result);
        });
    }

    /// Reloads only the sandbox policy into UI state.
    pub(super) fn request_sandbox(&self) {
        let result = SandboxStore::new(&self.workspace_root).and_then(|store| store.load());
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.sandbox_working = false;
            match result {
                Ok(policy) => {
                    shared.state.sandbox_settings = Some(sandbox_settings_state(&policy));
                    shared.state.sandbox_notice = None;
                }
                Err(error) => {
                    shared.state.sandbox_notice =
                        Some(format!("Unable to load sandbox settings: {error}"))
                }
            }
        }
        self.publish_state();
    }

    /// Reloads project toggles and sandbox policy together.
    pub(super) fn request_settings(&self) {
        let project = self.project_settings.load();
        let sandbox = SandboxStore::new(&self.workspace_root).and_then(|store| store.load());
        let active_model = self.shared.lock_or_recover().state.active_model.clone();
        let runtime = runtime_settings_state(&self.config, &active_model);
        if let Ok(project) = project.as_ref()
            && let Ok(mut coordinator) = self.coordinator.lock()
        {
            coordinator.configure_web_backend(project.web.backend, project.web.server.clone());
            coordinator.configure_searxng_url(project.web.searxng_url.clone());
        }
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.settings_working = false;
            shared.state.sandbox_working = false;
            match project {
                Ok(project) => {
                    shared.state.openai_flex = project.openai_flex;
                    shared.state.web_backend = project.web.backend.as_str().into();
                    shared.state.web_server = project.web.server.clone();
                    shared.state.web_searxng_url = project.web.searxng_url.clone();
                    shared.state.web_setup_plan = None;
                    shared.state.web_status = None;
                    shared.state.web_test_result = None;
                    shared.state.settings_notice = None;
                    self.bridge.set_openai_flex(project.openai_flex);
                }
                Err(error) => {
                    shared.state.settings_notice =
                        Some(format!("Unable to load project settings: {error}"));
                }
            }
            match sandbox {
                Ok(policy) => {
                    shared.state.sandbox_settings = Some(sandbox_settings_state(&policy));
                    shared.state.sandbox_notice = None;
                }
                Err(error) => {
                    shared.state.sandbox_notice =
                        Some(format!("Unable to load sandbox settings: {error}"));
                }
            }
            match runtime {
                Ok(runtime) => shared.state.runtime_settings = runtime,
                Err(error) => {
                    shared.state.settings_notice =
                        Some(format!("Unable to load runtime settings: {error}"));
                }
            }
        }
        self.publish_state();
    }

    pub(super) fn set_appearance(&self, appearance: String) {
        let result = self.config.set_appearance(&appearance);
        let active_model = self.shared.lock_or_recover().state.active_model.clone();
        self.finish_runtime_setting(result, &active_model, "Appearance updated.");
    }

    pub(super) fn set_theme(&self, mode: String, value: String) {
        let result = self.config.set_theme(&mode, &value);
        let active_model = self.shared.lock_or_recover().state.active_model.clone();
        self.finish_runtime_setting(result, &active_model, "Theme updated.");
    }

    pub(super) fn set_context_length(&self, length: Option<u64>) {
        let model = self.shared.lock_or_recover().state.active_model.clone();
        let result = if model.is_empty() {
            Err(anyhow!("Select a model before setting its context window"))
        } else {
            self.config.set_context_length_override(&model, length)
        };
        if result.is_ok() {
            if let Some(length) = length {
                self.shared
                    .lock_or_recover()
                    .state
                    .active_model_context_length = Some(length);
            } else {
                self.refresh_context_length();
            }
        }
        self.finish_runtime_setting(
            result,
            &model,
            if length.is_some() {
                "Context override updated."
            } else {
                "Context override returned to automatic."
            },
        );
    }

    /// Saves Group Agent settings and applies them to the live group.
    pub(super) fn set_agent_group_settings(&self, settings: crate::model::AgentGroupSettings) {
        let result = self
            .config
            .set_agent_group_settings(settings)
            .map(|settings| {
                self.agent_groups
                    .set_policy(crate::agents::group::AgentGroupPolicy::from(&settings));
            });
        let active_model = self.shared.lock_or_recover().state.active_model.clone();
        self.finish_runtime_setting(result, &active_model, "Agent Group settings updated.");
    }

    fn finish_runtime_setting(&self, result: Result<()>, active_model: &str, success: &str) {
        let refresh = result.and_then(|()| runtime_settings_state(&self.config, active_model));
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.settings_working = false;
            match refresh {
                Ok(runtime) => {
                    shared.state.runtime_settings = runtime;
                    shared.state.settings_notice = Some(success.into());
                }
                Err(error) => {
                    shared.state.settings_notice = Some(format!("Unable to save setting: {error}"));
                }
            }
        }
        self.publish_state();
    }

    pub(super) fn set_service_backend(
        &self,
        service: String,
        backend: String,
        server: Option<String>,
    ) {
        let service = service.trim().to_ascii_lowercase();
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.settings_working = true;
            shared.state.settings_notice = None;
        }
        self.publish_state();

        let result = ServiceBackend::parse(&backend).and_then(|backend| match service.as_str() {
            "web" | "search" => {
                if backend == ServiceBackend::Mcp {
                    let server = server
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .ok_or_else(|| anyhow!("Select an MCP server"))?;
                    let tools = self.bridge.client()?.list_mcp_tools(Some(server))?;
                    for name in ["web_search", "web_read"] {
                        if !tools.iter().any(|tool| tool.name == name) {
                            return Err(anyhow!("MCP server {server} is missing {name}"));
                        }
                    }
                }
                self.project_settings
                    .save_web_backend(backend, server.as_deref())
            }
            other => Err(anyhow!("Unknown service '{other}'; expected web")),
        });
        if let Err(error) = result {
            let mut shared = self.shared.lock_or_recover();
            shared.state.settings_working = false;
            shared.state.settings_notice = Some(format!("Unable to save service backend: {error}"));
            drop(shared);
            self.publish_state();
            return;
        }

        self.request_settings();
        let mut shared = self.shared.lock_or_recover();
        let selected = shared.state.web_backend.clone();
        shared.state.settings_notice = Some(format!("Web backend set to {selected}."));
        drop(shared);
        self.publish_state();
    }

    pub(super) fn set_web_search_endpoint(&self, url: Option<String>) {
        let result = (|| -> Result<()> {
            let mut settings = self.project_settings.load()?;
            let url = url.map(|u| u.trim().to_owned()).filter(|u| !u.is_empty());
            if let Some(ref value) = url {
                let parsed = url::Url::parse(value)?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err(anyhow!(
                        "Enter an HTTP or HTTPS endpoint without credentials"
                    ));
                }
            }
            settings.web.searxng_url = url;
            self.project_settings.save(&settings)
        })();
        self.request_settings();
        if let Err(error) = result {
            self.shared.lock_or_recover().state.settings_notice =
                Some(format!("Unable to save endpoint: {error}"));
            self.publish_state();
        }
    }

    pub(super) fn web_search_action(&self, action: &'static str, target: Option<String>) {
        {
            let mut shared = self.shared.lock_or_recover();
            if shared.state.web_search_working {
                return;
            }
            if action == "install"
                && shared
                    .state
                    .web_setup_plan
                    .as_ref()
                    .and_then(|p| p.install.as_deref())
                    != target.as_deref()
            {
                return;
            }
            shared.state.web_search_working = true;
            shared.state.settings_notice = Some(
                match action {
                    "test" => "Testing web search…",
                    "install" => "Installing web search…",
                    "plan" => "Checking setup…",
                    _ => "Checking web search status…",
                }
                .into(),
            );
        }
        self.publish_state();
        let store = self.project_settings.clone();
        let workspace = self.workspace_root.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = (|| -> Result<_> {
                let settings = store.load()?.web;
                let mut plan = None;
                let notice = match action {
                    "plan" => {
                        let mut p = crate::web_search::setup_plan(&settings, Some(&workspace));
                        if p.install.is_none() {
                            match crate::web_search::test_web_search(&settings, Some(&workspace)) {
                                Ok(message) => message,
                                Err(error) if settings.backend == ServiceBackend::Mcp => {
                                    return Err(error);
                                }
                                Err(error) => {
                                    p = crate::web_search::repair_plan(&settings, Some(&workspace));
                                    p.message =
                                        format!("Search test failed: {error:#}. {}", p.message);
                                    let message = p.message.clone();
                                    plan = Some(p);
                                    message
                                }
                            }
                        } else {
                            let message = p.message.clone();
                            plan = Some(p);
                            message
                        }
                    }
                    "install" => {
                        let p = crate::web_search::repair_plan(&settings, Some(&workspace));
                        let target = target
                            .as_deref()
                            .ok_or_else(|| anyhow!("Missing install target"))?;
                        if p.install.as_deref() != Some(target) {
                            return Err(anyhow!("Setup plan changed; check setup again"));
                        }
                        if !p.missing_dependencies.is_empty() {
                            return Err(anyhow!(
                                "Install these prerequisites first: {}",
                                p.missing_dependencies.join(", ")
                            ));
                        }
                        crate::web_search::install_web_search(target)?;
                        crate::web_search::test_web_search(&settings, Some(&workspace))?
                    }
                    "test" => crate::web_search::test_web_search(&settings, Some(&workspace))?,
                    _ => "Web search status refreshed.".into(),
                };
                let status =
                    crate::web_search::web_search_status_for_workspace(&settings, Some(&workspace));
                let discovery = crate::core::BridgeClient::start_for_workspace(&workspace)
                    .and_then(|bridge| {
                        let result = bridge.list_mcp_servers().map(|servers| {
                            servers
                                .into_iter()
                                .filter_map(|server| {
                                    let tools = bridge.list_mcp_tools(Some(&server.name)).ok()?;
                                    ["web_search", "web_read"]
                                        .iter()
                                        .all(|name| tools.iter().any(|tool| tool.name == *name))
                                        .then_some(server.name)
                                })
                                .collect::<Vec<_>>()
                        });
                        bridge.shutdown();
                        result
                    });
                let (servers, discovery_error) = match discovery {
                    Ok(servers) => (servers, None),
                    Err(error) => (Vec::new(), Some(format!("MCP discovery failed: {error:#}"))),
                };
                Ok((notice, plan, status, servers, discovery_error))
            })();
            if let Ok(mut shared) = shared.lock() {
                shared.state.web_search_working = false;
                match result {
                    Ok((notice, plan, status, servers, discovery_error)) => {
                        if action == "test"
                            || action == "install"
                            || (action == "plan" && plan.is_none())
                        {
                            shared.state.web_test_result =
                                Some(crate::web_search::SearchTestResult {
                                    success: true,
                                    message: notice.clone(),
                                });
                        }
                        shared.state.settings_notice = Some(notice);
                        shared.state.web_setup_plan = plan;
                        shared.state.web_status = Some(status);
                        shared.state.web_mcp_servers = servers;
                        shared.state.web_mcp_error = discovery_error;
                    }
                    Err(error) => {
                        let message = format!("Web search: {error:#}");
                        if action != "status" {
                            shared.state.web_test_result =
                                Some(crate::web_search::SearchTestResult {
                                    success: false,
                                    message: message.clone(),
                                });
                        }
                        shared.state.settings_notice = Some(message);
                        shared.state.web_setup_plan = None;
                    }
                }
                let _ = tx.send(ServiceEvent::Envelope(state_envelope(&shared.state)));
            }
        });
    }

    /// Enables or disables OpenAI Flex requests for this workspace.
    pub(super) fn set_openai_flex(&self, enabled: bool) {
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.settings_working = true;
            shared.state.settings_notice = None;
        }
        self.publish_state();

        let result = self.project_settings.save_openai_flex(enabled);
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.settings_working = false;
            match result {
                Ok(()) => {
                    self.bridge.set_openai_flex(enabled);
                    shared.state.openai_flex = enabled;
                    shared.state.settings_notice = Some(format!(
                        "OpenAI Flex {}.",
                        if enabled { "enabled" } else { "disabled" }
                    ));
                }
                Err(error) => {
                    shared.state.settings_notice =
                        Some(format!("Unable to save OpenAI Flex setting: {error}"));
                }
            }
        }
        self.publish_state();
    }

    /// Applies one sandbox mutation and publishes the resulting policy.
    pub(super) fn update_sandbox(&self, action: SandboxAction) {
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.sandbox_working = true;
            shared.state.sandbox_notice = None;
        }
        self.publish_state();
        let result = apply_sandbox_action(&self.workspace_root, action);
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.sandbox_working = false;
            match result {
                Ok(policy) => shared.state.sandbox_settings = Some(sandbox_settings_state(&policy)),
                Err(error) => shared.state.sandbox_notice = Some(error.to_string()),
            }
        }
        self.publish_state();
    }
}
