//! Authentication, provider, project-setting, and sandbox commands.
//!
//! These operations mutate configuration around a session but do not execute
//! an agent turn, so they live outside the backend runtime dispatcher.

use super::*;

impl BackendService {
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
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
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
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
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
            coordinator.configure_foundation_memory(
                project.foundation_memory.enabled,
                project.foundation_memory.backend,
                project.foundation_memory.server.clone(),
                self.project_identity.clone(),
            );
            coordinator.configure_web_backend(project.web.backend, project.web.server.clone());
        }
        let foundation_connected = project.as_ref().ok().is_some_and(|project| {
            project.foundation_memory.enabled
                && foundation_server_ready(
                    &self.bridge,
                    project.foundation_memory.backend,
                    &project.foundation_memory.server,
                )
        });
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.settings_working = false;
            shared.state.sandbox_working = false;
            match project {
                Ok(project) => {
                    shared.state.openai_flex = project.openai_flex;
                    shared.state.foundation_memory_enabled = project.foundation_memory.enabled;
                    shared.state.foundation_memory_backend =
                        project.foundation_memory.backend.as_str().into();
                    shared.state.foundation_memory_server =
                        project.foundation_memory.server.clone();
                    shared.state.foundation_memory_connected = foundation_connected;
                    shared.state.web_backend = project.web.backend.as_str().into();
                    shared.state.web_server = project.web.server.clone();
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

    pub(super) fn set_jev_loop_mode(&self, mode: String) {
        let result = self.config.set_jev_loop_mode(&mode).map(|_| ());
        let active_model = self.shared.lock_or_recover().state.active_model.clone();
        self.finish_runtime_setting(result, &active_model, "Jev loop policy updated.");
    }

    /// Saves Group Agent settings and applies them to the live group.
    pub(super) fn set_agent_group_settings(&self, settings: crate::model::AgentGroupSettings) {
        let result = self
            .config
            .set_agent_group_settings(settings)
            .map(|settings| {
                self.agent_groups
                    .set_limits(crate::agents::group::AgentLimits::from(&settings));
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

    /// Enables or disables Foundation memory for this workspace.
    pub(super) fn set_foundation_memory(&self, enabled: bool) {
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.settings_working = true;
            shared.state.settings_notice = None;
        }
        self.publish_state();

        let result = self.project_settings.save_foundation_memory(enabled, None);
        let refreshed = result.and_then(|()| self.project_settings.load());
        if let Ok(project) = refreshed.as_ref()
            && let Ok(mut coordinator) = self.coordinator.lock()
        {
            coordinator.configure_foundation_memory(
                project.foundation_memory.enabled,
                project.foundation_memory.backend,
                project.foundation_memory.server.clone(),
                self.project_identity.clone(),
            );
            coordinator.configure_web_backend(project.web.backend, project.web.server.clone());
        }
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.settings_working = false;
            match refreshed {
                Ok(project) => {
                    shared.state.foundation_memory_enabled = project.foundation_memory.enabled;
                    shared.state.foundation_memory_backend =
                        project.foundation_memory.backend.as_str().into();
                    shared.state.foundation_memory_server =
                        project.foundation_memory.server.clone();
                    shared.state.foundation_memory_connected = project.foundation_memory.enabled
                        && foundation_server_ready(
                            &self.bridge,
                            project.foundation_memory.backend,
                            &project.foundation_memory.server,
                        );
                    shared.state.web_backend = project.web.backend.as_str().into();
                    shared.state.web_server = project.web.server.clone();
                    shared.state.settings_notice = Some(format!(
                        "Foundation memory {}.",
                        if enabled { "enabled" } else { "disabled" }
                    ));
                }
                Err(error) => {
                    shared.state.settings_notice =
                        Some(format!("Unable to save Foundation memory setting: {error}"));
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
            "memory" | "foundation" => self
                .project_settings
                .save_foundation_backend(backend, server.as_deref()),
            "web" | "search" => self
                .project_settings
                .save_web_backend(backend, server.as_deref()),
            other => Err(anyhow!("Unknown service '{other}'; expected memory or web")),
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
        let selected = if matches!(service.as_str(), "memory" | "foundation") {
            shared.state.foundation_memory_backend.clone()
        } else {
            shared.state.web_backend.clone()
        };
        let label = if matches!(service.as_str(), "memory" | "foundation") {
            "Memory"
        } else {
            "Web"
        };
        shared.state.settings_notice = Some(format!("{label} backend set to {selected}."));
        drop(shared);
        self.publish_state();
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
