//! Provider, settings, sandbox, and related popup interaction behavior.

use super::*;

impl App {
    pub(super) fn handle_providers_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => self.close_popup(),
            KeyCode::Up => {
                self.pending_provider_delete_id = None;
                self.popup_index = self.popup_index.saturating_sub(1);
            }
            KeyCode::Down => {
                self.pending_provider_delete_id = None;
                self.popup_index = cmp::min(
                    self.popup_index + 1,
                    self.state.provider_configurations.len().saturating_sub(1),
                );
            }
            KeyCode::Char('r') => {
                self.pending_provider_delete_id = None;
                backend.send(FrontendCommand::RequestProviders)?;
            }
            KeyCode::Char('n') if !self.state.providers_working => {
                self.pending_provider_delete_id = None;
                self.open_provider_editor(None);
            }
            KeyCode::Enter if !self.state.providers_working => {
                self.pending_provider_delete_id = None;
                if let Some(provider) = self
                    .state
                    .provider_configurations
                    .get(self.popup_index)
                    .cloned()
                {
                    self.open_provider_editor(Some(provider));
                }
            }
            KeyCode::Char('d') | KeyCode::Delete if !self.state.providers_working => {
                if let Some(id) = self.confirm_provider_delete() {
                    backend.send(FrontendCommand::RemoveProvider { id })?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn confirm_provider_delete(&mut self) -> Option<String> {
        let id = self
            .state
            .provider_configurations
            .get(self.popup_index)?
            .id
            .clone();
        if self.pending_provider_delete_id.as_deref() == Some(id.as_str()) {
            self.pending_provider_delete_id = None;
            Some(id)
        } else {
            self.pending_provider_delete_id = Some(id);
            None
        }
    }

    pub(super) fn handle_provider_edit_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => {
                self.clear_editor();
                self.mode = Mode::Providers;
            }
            KeyCode::Tab | KeyCode::Down => self.editor_index = (self.editor_index + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => self.editor_index = (self.editor_index + 2) % 3,
            KeyCode::Char(' ') if self.editor_index == 2 => {
                self.editor_toggle = !self.editor_toggle
            }
            KeyCode::Enter => {
                let id = self.editor_fields.first().cloned().unwrap_or_default();
                let base_url = self.editor_fields.get(1).cloned().unwrap_or_default();
                if id.trim().is_empty() || base_url.trim().is_empty() {
                    self.backend_message = Some("Provider ID and base URL are required".into());
                } else {
                    backend.send(FrontendCommand::SaveProvider {
                        id,
                        base_url,
                        require_api_key: self.editor_toggle,
                    })?;
                    self.clear_editor();
                    self.mode = Mode::Providers;
                }
            }
            KeyCode::Backspace if self.provider_field_editable() => {
                if let Some(field) = self.editor_fields.get_mut(self.editor_index) {
                    field.pop();
                }
            }
            KeyCode::Char('u')
                if event.modifiers.contains(KeyModifiers::CONTROL)
                    && self.provider_field_editable() =>
            {
                if let Some(field) = self.editor_fields.get_mut(self.editor_index) {
                    field.clear();
                }
            }
            KeyCode::Char(character)
                if self.provider_field_editable()
                    && !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::SUPER) =>
            {
                if let Some(field) = self.editor_fields.get_mut(self.editor_index) {
                    field.push(character);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn sync_settings(&mut self) {
        use crate::shared_ui::settings::*;
        self.application.configure_settings(
            SettingsEnvironment {
                available: true,
                supported_features: vec![
                    SettingsFeature::ServiceBackends,
                    SettingsFeature::JevLoop,
                ],
                supported_destinations: vec![
                    SettingsDestination::Models,
                    SettingsDestination::Reasoning,
                    SettingsDestination::Agents,
                    SettingsDestination::Permissions,
                    SettingsDestination::Auth,
                    SettingsDestination::Providers,
                    SettingsDestination::Capabilities,
                ],
                supported_editors: vec![
                    SettingsEditorKind::ContextLength,
                    SettingsEditorKind::DarkTheme,
                    SettingsEditorKind::LightTheme,
                ],
            },
            &self.state,
        );
        let view = self.application.settings_projection().view;
        if view.selected.is_none() {
            let first = view.controls().next().map(|control| control.id.clone());
            let prepared = self
                .application
                .prepare_settings(SettingsAction::Select(first), &self.state);
            self.application.commit_settings(prepared, &self.state);
        }
        if self.mode == Mode::Settings {
            let view = self.application.settings_projection().view;
            self.popup_index = view
                .controls()
                .position(|control| Some(&control.id) == view.selected.as_ref())
                .unwrap_or(0);
        }
    }
    fn send_settings_action(
        &mut self,
        backend: &mut Backend,
        action: crate::shared_ui::settings::SettingsAction,
    ) -> anyhow::Result<()> {
        use crate::shared_ui::settings::*;
        self.sync_settings();
        let editor_fields = self.editor_fields.clone();
        let prepared = self.application.prepare_settings(action, &self.state);
        if let Some(command) = prepared.effect.command.clone() {
            let refresh_theme = matches!(command, FrontendCommand::RequestSettings);
            backend.send(command)?;
            if refresh_theme {
                crate::tui::ui::refresh_runtime_theme(&self.state.runtime_settings);
            }
        }
        if let Some(destination) = prepared.effect.ui.destination {
            match destination {
                SettingsDestination::Models => self.open_models(backend)?,
                SettingsDestination::Reasoning => self.open_reasoning(),
                SettingsDestination::Agents => {
                    let command = self.open_agent_group();
                    backend.send(command)?;
                }
                SettingsDestination::Permissions => self.open_sandbox_presets(),
                SettingsDestination::Auth => self.open_auth(backend)?,
                SettingsDestination::Providers => self.open_providers(backend)?,
                SettingsDestination::Capabilities => self.open_capabilities(backend)?,
            }
        }
        let (_, effect) = self.application.commit_settings(prepared, &self.state);
        if effect.ui.accepted_editor.is_some() && self.editor_fields == editor_fields {
            self.clear_editor();
            self.mode = Mode::Settings;
        } else if let Some(editor) = self.application.settings_projection().view.editor {
            let kind = match editor.kind {
                SettingsEditorKind::ContextLength => SettingsEditKind::ContextLength,
                SettingsEditorKind::DarkTheme => SettingsEditKind::ThemeDark,
                SettingsEditorKind::LightTheme => SettingsEditKind::ThemeLight,
            };
            if self.mode != Mode::SettingsEdit {
                self.open_settings_editor(kind, vec![editor.value]);
            }
        }
        self.sync_settings();
        Ok(())
    }
    pub(super) fn handle_settings_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        use crate::shared_ui::settings::SettingsAction;
        self.sync_settings();
        match event.code {
            KeyCode::Esc => self.close_popup(),
            KeyCode::Up => self.send_settings_action(backend, SettingsAction::MoveSelection(-1))?,
            KeyCode::Down => {
                self.send_settings_action(backend, SettingsAction::MoveSelection(1))?
            }
            KeyCode::Char('r') => self.send_settings_action(backend, SettingsAction::Refresh)?,
            KeyCode::Enter | KeyCode::Char(' ') => {
                let view = self.application.settings_projection().view;
                if let Some(id) = view.selected {
                    self.send_settings_action(backend, SettingsAction::Activate(id))?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_sandbox_presets_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => {
                self.mode = Mode::Chat;
                self.popup_index = 0;
            }
            KeyCode::Up => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down => {
                self.popup_index = cmp::min(self.popup_index + 1, SANDBOX_PRESET_ROW_COUNT - 1)
            }
            KeyCode::Char('r') => backend.send(FrontendCommand::RequestSandbox)?,
            KeyCode::Enter | KeyCode::Char(' ') => match self.popup_index {
                0 => self.apply_sandbox_preset("safe", backend)?,
                1 => self.apply_sandbox_preset("balanced", backend)?,
                2 => self.apply_sandbox_preset("unlimited", backend)?,
                3 => self.open_sandbox_policy(),
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_sandbox_policy_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        let section = self.settings_section;
        let reset_confirmation_key = matches!(event.code, KeyCode::Enter | KeyCode::Char(' '))
            && section == Some(SettingsSection::Core)
            && self.popup_index == 3;
        if !reset_confirmation_key {
            self.pending_sandbox_reset = false;
        }
        match event.code {
            KeyCode::Esc => {
                self.popup_index = 3;
                self.mode = Mode::SandboxPresets;
            }
            KeyCode::Tab | KeyCode::Right => self.cycle_settings_section(1),
            KeyCode::BackTab | KeyCode::Left => self.cycle_settings_section(-1),
            KeyCode::Up => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down => {
                let count = self.settings_detail_count();
                self.popup_index = cmp::min(self.popup_index + 1, count.saturating_sub(1));
            }
            KeyCode::Char('n') => match section {
                Some(SettingsSection::Workspace) => {
                    self.open_settings_editor(SettingsEditKind::WorkspacePath, vec![String::new()])
                }
                Some(SettingsSection::Network) => self.open_settings_editor(
                    SettingsEditKind::Network,
                    vec![String::new(), "*".into()],
                ),
                Some(SettingsSection::Environment) => self.open_settings_editor(
                    SettingsEditKind::Environment { original_key: None },
                    vec![String::new(), String::new()],
                ),
                Some(SettingsSection::Secrets) => {
                    self.open_settings_editor(SettingsEditKind::Secret, vec![String::new()])
                }
                _ => {}
            },
            KeyCode::Enter | KeyCode::Char(' ') if section == Some(SettingsSection::Core) => {
                let confirm_reset = self.popup_index == 3 && self.confirm_sandbox_reset();
                match self.popup_index {
                    0 => self.toggle_execution_mode(backend)?,
                    1 => self.toggle_auto_approve(backend)?,
                    2 => self.toggle_scratch(backend)?,
                    3 if confirm_reset => {
                        backend.send(FrontendCommand::UpdateSandbox {
                            action: SandboxAction::Reset,
                        })?;
                    }
                    _ => {}
                }
            }
            KeyCode::Enter | KeyCode::Char(' ')
                if section == Some(SettingsSection::Workspace) && self.popup_index == 0 =>
            {
                if let Some(settings) = self.state.sandbox_settings.as_ref() {
                    let mode = if settings.workspace_mode == "all" {
                        "none"
                    } else {
                        "all"
                    };
                    backend.send(FrontendCommand::UpdateSandbox {
                        action: SandboxAction::SetWorkspaceMode { mode: mode.into() },
                    })?;
                }
            }
            KeyCode::Enter if section == Some(SettingsSection::Environment) => {
                if let Some(settings) = self.state.sandbox_settings.as_ref()
                    && let Some(item) = settings.environment.get(self.popup_index).cloned()
                {
                    self.open_settings_editor(
                        SettingsEditKind::Environment {
                            original_key: Some(item.key.clone()),
                        },
                        vec![item.key, item.value],
                    );
                }
            }
            KeyCode::Enter if section == Some(SettingsSection::Limits) => {
                if let Some((name, value)) = self.selected_limit() {
                    self.open_settings_editor(
                        SettingsEditKind::Limit { name: name.into() },
                        vec![value.to_string()],
                    );
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => self.delete_settings_item(backend)?,
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_settings_edit_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => {
                let runtime_editor = matches!(
                    self.settings_edit_kind,
                    Some(
                        SettingsEditKind::ContextLength
                            | SettingsEditKind::ThemeDark
                            | SettingsEditKind::ThemeLight
                    )
                );
                if runtime_editor {
                    self.send_settings_action(
                        backend,
                        crate::shared_ui::settings::SettingsAction::CancelEditor,
                    )?;
                }
                self.clear_editor();
                self.mode = if runtime_editor {
                    Mode::Settings
                } else {
                    Mode::SandboxPolicy
                };
            }
            KeyCode::Tab | KeyCode::Down if self.editor_fields.len() > 1 => {
                self.editor_index = (self.editor_index + 1) % self.editor_fields.len();
            }
            KeyCode::BackTab | KeyCode::Up if self.editor_fields.len() > 1 => {
                self.editor_index =
                    (self.editor_index + self.editor_fields.len() - 1) % self.editor_fields.len();
            }
            KeyCode::Enter => self.submit_settings_editor(backend)?,
            KeyCode::Backspace => {
                if let Some(field) = self.editor_fields.get_mut(self.editor_index) {
                    field.pop();
                }
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(field) = self.editor_fields.get_mut(self.editor_index) {
                    field.clear();
                }
            }
            KeyCode::Char(character)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::SUPER) =>
            {
                if let Some(field) = self.editor_fields.get_mut(self.editor_index) {
                    field.push(character);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn open_models(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Models;
        self.popup_filter.clear();
        self.popup_index = self.active_model_picker_index();
        backend.send(FrontendCommand::RequestModels)
    }

    pub(super) fn active_model_picker_index(&self) -> usize {
        self.filtered_models()
            .iter()
            .position(|model| *model == self.state.active_model.as_str())
            .unwrap_or(0)
    }

    pub(super) fn open_goal(&mut self) {
        self.mode = Mode::Goal;
        self.popup_filter.clear();
        self.popup_index = if self.state.goal_mode { 0 } else { 1 };
    }

    pub(super) fn open_sessions(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Sessions;
        self.popup_filter.clear();
        self.popup_index = self
            .session_picker_items()
            .iter()
            .position(|item| {
                Some(item.session.id.as_str()) == self.state.current_session_id.as_deref()
            })
            .unwrap_or(0);
        backend.send(FrontendCommand::RequestSessions)
    }

    pub(super) fn open_reasoning(&mut self) {
        self.mode = Mode::Reasoning;
        self.popup_filter.clear();
        self.popup_index = reasoning_levels_for_model(&self.state.active_model)
            .iter()
            .position(|level| *level == self.state.active_reasoning_level)
            .unwrap_or(0);
    }

    pub(super) fn open_capabilities(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Capabilities;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.capability_detail_id = None;
        backend.send(FrontendCommand::RequestCapabilities)
    }

    pub(super) fn open_auth(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Auth;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.active_auth_provider = None;
        backend.send(FrontendCommand::RequestAuth)
    }

    pub(super) fn open_providers(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Providers;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.pending_provider_delete_id = None;
        self.clear_editor();
        backend.send(FrontendCommand::RequestProviders)
    }

    pub(super) fn open_provider_editor(&mut self, provider: Option<ProviderConfigurationItem>) {
        self.pending_provider_delete_id = None;
        let (id, base_url, require_api_key, existing) = match provider {
            Some(provider) => (
                provider.id.clone(),
                provider.base_url,
                provider.require_api_key,
                Some(provider.id),
            ),
            None => (String::new(), String::new(), false, None),
        };
        self.editor_fields = vec![id, base_url];
        self.editor_index = if existing.is_some() { 1 } else { 0 };
        self.editor_toggle = require_api_key;
        self.editing_provider_id = existing;
        self.mode = Mode::ProviderEdit;
    }

    pub(super) fn provider_field_editable(&self) -> bool {
        self.editor_index < 2 && !(self.editor_index == 0 && self.editing_provider_id.is_some())
    }

    pub(super) fn open_settings(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Settings;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.settings_section = None;
        self.settings_edit_kind = None;
        self.send_settings_action(backend, crate::shared_ui::settings::SettingsAction::CancelEditor)?;
        self.send_settings_action(backend, crate::shared_ui::settings::SettingsAction::Refresh)
    }

    pub(super) fn open_status(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Status;
        self.popup_filter.clear();
        self.popup_index = 0;
        backend.send(FrontendCommand::RequestAuth)
    }

    pub(super) fn open_sandbox_presets(&mut self) {
        self.mode = Mode::SandboxPresets;
        self.popup_index = self.sandbox_preset_picker_index();
    }

    pub(super) fn sandbox_preset_picker_index(&self) -> usize {
        match self
            .state
            .sandbox_settings
            .as_ref()
            .map(|settings| settings.preset.as_str())
        {
            Some("safe") => 0,
            Some("balanced") => 1,
            Some("unlimited") => 2,
            Some(_) => 3,
            None => 0,
        }
    }

    pub(super) fn open_sandbox_policy(&mut self) {
        self.settings_section = Some(SettingsSection::Core);
        self.settings_edit_kind = None;
        self.pending_sandbox_reset = false;
        self.popup_index = 0;
        self.mode = Mode::SandboxPolicy;
    }

    pub(super) fn confirm_sandbox_reset(&mut self) -> bool {
        if self.pending_sandbox_reset {
            self.pending_sandbox_reset = false;
            true
        } else {
            self.pending_sandbox_reset = true;
            false
        }
    }

    pub(super) fn open_settings_editor(&mut self, kind: SettingsEditKind, fields: Vec<String>) {
        self.settings_edit_kind = Some(kind);
        self.editor_fields = fields;
        self.editor_index = 0;
        self.mode = Mode::SettingsEdit;
    }

    pub(super) fn selected_auth_provider(&self) -> Option<String> {
        self.state
            .auth_providers
            .get(self.popup_index)
            .map(|item| item.provider.clone())
    }

    pub(super) fn toggle_scratch(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        if let Some(settings) = self.state.sandbox_settings.as_ref() {
            backend.send(FrontendCommand::UpdateSandbox {
                action: SandboxAction::SetScratchWritable {
                    enabled: !settings.scratch_writable,
                },
            })?;
        }
        Ok(())
    }

    pub(super) fn apply_sandbox_preset(
        &mut self,
        preset: &str,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        backend.send(FrontendCommand::UpdateSandbox {
            action: SandboxAction::ApplyPreset {
                preset: preset.into(),
            },
        })
    }

    pub(super) fn toggle_execution_mode(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        if let Some(settings) = self.state.sandbox_settings.as_ref() {
            let mode = if settings.execution_mode == "unlimited" {
                "sandboxed"
            } else {
                "unlimited"
            };
            backend.send(FrontendCommand::UpdateSandbox {
                action: SandboxAction::SetExecutionMode { mode: mode.into() },
            })?;
        }
        Ok(())
    }

    pub(super) fn toggle_auto_approve(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        if let Some(settings) = self.state.sandbox_settings.as_ref() {
            backend.send(FrontendCommand::UpdateSandbox {
                action: SandboxAction::SetAutoApprove {
                    enabled: !settings.auto_approve,
                },
            })?;
        }
        Ok(())
    }

    pub(super) fn cycle_settings_section(&mut self, delta: isize) {
        const SECTIONS: [SettingsSection; 6] = [
            SettingsSection::Core,
            SettingsSection::Workspace,
            SettingsSection::Network,
            SettingsSection::Environment,
            SettingsSection::Secrets,
            SettingsSection::Limits,
        ];
        let current = self
            .settings_section
            .and_then(|section| SECTIONS.iter().position(|value| *value == section))
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(SECTIONS.len() as isize) as usize;
        self.settings_section = Some(SECTIONS[next]);
        self.popup_index = 0;
    }

    pub fn settings_detail_count(&self) -> usize {
        let Some(settings) = self.state.sandbox_settings.as_ref() else {
            return 0;
        };
        match self.settings_section {
            Some(SettingsSection::Core) => 4,
            Some(SettingsSection::Workspace) => 1 + settings.workspace_paths.len(),
            Some(SettingsSection::Network) => settings.network_allow.len(),
            Some(SettingsSection::Environment) => settings.environment.len(),
            Some(SettingsSection::Secrets) => settings.secret_ids.len(),
            Some(SettingsSection::Limits) => 5,
            None => 0,
        }
    }

    pub(crate) fn settings_row_count(&self) -> usize {
        self.application
            .settings_projection()
            .view
            .controls()
            .count()
    }

    pub(super) fn selected_limit(&self) -> Option<(&'static str, u64)> {
        let settings = self.state.sandbox_settings.as_ref()?;
        Some(match self.popup_index {
            0 => ("wall_time_seconds", settings.limits.wall_time_seconds),
            1 => ("max_stdout_bytes", settings.limits.max_stdout_bytes as u64),
            2 => ("max_stderr_bytes", settings.limits.max_stderr_bytes as u64),
            3 => ("max_memory_bytes", settings.limits.max_memory_bytes),
            4 => ("max_processes", settings.limits.max_processes as u64),
            _ => return None,
        })
    }

    pub(super) fn delete_settings_item(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        let Some(settings) = self.state.sandbox_settings.as_ref() else {
            return Ok(());
        };
        let action = match self.settings_section {
            Some(SettingsSection::Workspace) if self.popup_index > 0 => settings
                .workspace_paths
                .get(self.popup_index - 1)
                .cloned()
                .map(|path| SandboxAction::RemoveWorkspacePath { path }),
            Some(SettingsSection::Network) => settings
                .network_allow
                .get(self.popup_index)
                .cloned()
                .map(|item| SandboxAction::RemoveNetwork {
                    host: item.host,
                    port: item.port,
                }),
            Some(SettingsSection::Environment) => settings
                .environment
                .get(self.popup_index)
                .cloned()
                .map(|item| SandboxAction::RemoveEnvironment { key: item.key }),
            Some(SettingsSection::Secrets) => settings
                .secret_ids
                .get(self.popup_index)
                .cloned()
                .map(|id| SandboxAction::RemoveSecret { id }),
            _ => None,
        };
        if let Some(action) = action {
            backend.send(FrontendCommand::UpdateSandbox { action })?;
        }
        Ok(())
    }

    pub(super) fn submit_settings_editor(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        let Some(kind) = self.settings_edit_kind.clone() else {
            return Ok(());
        };
        if matches!(
            kind,
            SettingsEditKind::ContextLength
                | SettingsEditKind::ThemeDark
                | SettingsEditKind::ThemeLight
        ) {
            let value = self.editor_fields.first().cloned().unwrap_or_default();
            return self.send_settings_action(
                backend,
                crate::shared_ui::settings::SettingsAction::SubmitEditor(value),
            );
        }

        let action = match kind {
            SettingsEditKind::WorkspacePath => {
                let path = self.editor_fields.first().cloned().unwrap_or_default();
                if path.trim().is_empty() {
                    self.backend_message = Some("Workspace path cannot be empty".into());
                    return Ok(());
                }
                SandboxAction::AddWorkspacePath { path }
            }
            SettingsEditKind::Network => {
                let host = self.editor_fields.first().cloned().unwrap_or_default();
                let port_text = self
                    .editor_fields
                    .get(1)
                    .map(String::as_str)
                    .unwrap_or("*")
                    .trim();
                if host.trim().is_empty() {
                    self.backend_message = Some("Network host cannot be empty".into());
                    return Ok(());
                }
                let port = if port_text.is_empty() || port_text == "*" {
                    None
                } else {
                    match port_text.parse::<u16>() {
                        Ok(0) | Err(_) => {
                            self.backend_message =
                                Some("Network port must be 1...65535 or *".into());
                            return Ok(());
                        }
                        Ok(value) => Some(value),
                    }
                };
                SandboxAction::AddNetwork { host, port }
            }
            SettingsEditKind::Environment { original_key } => {
                let key = self.editor_fields.first().cloned().unwrap_or_default();
                let value = self.editor_fields.get(1).cloned().unwrap_or_default();
                if key.trim().is_empty() {
                    self.backend_message = Some("Environment key cannot be empty".into());
                    return Ok(());
                }
                if let Some(original) = original_key.filter(|original| original != key.trim()) {
                    backend.send(FrontendCommand::UpdateSandbox {
                        action: SandboxAction::RemoveEnvironment { key: original },
                    })?;
                }
                SandboxAction::SetEnvironment { key, value }
            }
            SettingsEditKind::Secret => {
                let id = self.editor_fields.first().cloned().unwrap_or_default();
                if id.trim().is_empty() {
                    self.backend_message = Some("Secret ID cannot be empty".into());
                    return Ok(());
                }
                SandboxAction::AddSecret { id }
            }
            SettingsEditKind::Limit { name } => {
                let value = match self
                    .editor_fields
                    .first()
                    .map(String::as_str)
                    .unwrap_or_default()
                    .trim()
                    .parse::<u64>()
                {
                    Ok(value) => value,
                    Err(_) => {
                        self.backend_message = Some("Limit must be a positive integer".into());
                        return Ok(());
                    }
                };
                SandboxAction::SetLimit { name, value }
            }
            SettingsEditKind::ContextLength
            | SettingsEditKind::ThemeDark
            | SettingsEditKind::ThemeLight => unreachable!(),
        };
        backend.send(FrontendCommand::UpdateSandbox { action })?;
        self.clear_editor();
        self.mode = Mode::SandboxPolicy;
        Ok(())
    }
}

#[cfg(test)]
mod shared_settings_adapter_tests {
    use super::*;
    use crate::shared_ui::settings::SettingsAction;
    #[test]
    fn settings_selection_survives_inserted_capability_rows() {
        let mut app = App::default();
        app.mode = Mode::Settings;
        app.sync_settings();
        let prepared = app
            .application
            .prepare_settings(SettingsAction::Select(Some("agents".into())), &app.state);
        app.application.commit_settings(prepared, &app.state);
        app.sync_settings();
        let before = app.popup_index;
        app.state
            .available_capabilities
            .push(crate::model::CapabilityToggleItem {
                id: "test-capability".into(),
                name: "Test capability".into(),
                description: "Dynamic runtime capability".into(),
                enabled: true,
                kind: "skill".into(),
                source: None,
            });
        app.sync_settings();
        assert_eq!(
            app.application.settings_state().selected.as_deref(),
            Some("agents")
        );
        assert_eq!(app.popup_index, before + 1);
        assert_eq!(
            app.settings_row_count(),
            app.application
                .settings_projection()
                .view
                .controls()
                .count()
        );
    }
}
