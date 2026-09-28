use std::{
    cmp,
    path::Path,
    time::{Duration, Instant},
};

mod selection;
mod settings;
#[cfg(test)]
mod tests;
pub use selection::TranscriptContextMenu;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    backend::Backend,
    model::{
        BridgeState, CapabilityToggleItem, ConversationEntry, FrontendCommand, ModelCatalogItem,
        ProviderConfigurationItem, SandboxAction, SessionSummary, reasoning_levels_for_model,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chat,
    Debate,
    Models,
    Reasoning,
    Goal,
    Sessions,
    Capabilities,
    CapabilityDetail,
    Auth,
    AuthKey,
    Providers,
    ProviderEdit,
    Settings,
    SandboxPresets,
    SandboxPolicy,
    SettingsEdit,
    Status,
    Help,
}

#[derive(Debug, Clone)]
pub struct SessionPickerItem<'a> {
    pub workspace_name: String,
    pub workspace_current: bool,
    pub session: &'a SessionSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Core,
    Workspace,
    Network,
    Environment,
    Secrets,
    Limits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsEditKind {
    ContextLength,
    ThemeDark,
    ThemeLight,
    WorkspacePath,
    Network,
    Environment { original_key: Option<String> },
    Secret,
    Limit { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionPromptAction {
    Allow,
    Deny,
    Interrupt,
}

pub struct App {
    pub debate_models: crate::debate::DebateModels,
    pub debate_field: usize,
    pub debate_model_picker: bool,
    pub debate_topic_draft: String,
    pub conversation: Vec<ConversationEntry>,
    pub state: BridgeState,
    pub input: String,
    pub cursor: usize,
    pub mode: Mode,
    pub popup_filter: String,
    pub popup_index: usize,
    pub capability_detail_id: Option<String>,
    pub editor_fields: Vec<String>,
    pub editor_index: usize,
    pub editor_toggle: bool,
    pub editing_provider_id: Option<String>,
    pub pending_provider_delete_id: Option<String>,
    pub pending_sandbox_reset: bool,
    pub active_auth_provider: Option<String>,
    pub settings_section: Option<SettingsSection>,
    pub settings_edit_kind: Option<SettingsEditKind>,
    pub command_index: usize,
    pub input_history: Vec<String>,
    pub history_index: Option<usize>,
    pub history_draft: String,
    pub follow_tail: bool,
    pub scroll_y: u16,
    pub max_scroll: u16,
    pub transcript_area: (u16, u16, u16, u16),
    pub(crate) composer_area: (u16, u16, u16, u16),
    pub(crate) composer_width: u16,
    pub(crate) composer_scroll: usize,
    pub transcript_cells: Vec<Vec<String>>,
    pub selection_start: Option<(u16, u16)>,
    pub selection_end: Option<(u16, u16)>,
    pub transcript_context_menu: Option<TranscriptContextMenu>,
    pub transcript_context_menu_area: (u16, u16, u16, u16),
    pub(crate) clipboard_request: Option<String>,
    pub(crate) sidebar_area: (u16, u16, u16, u16),
    pub(crate) sidebar_session_targets: Vec<(u16, String)>,
    pub(crate) sidebar_load_request: Option<String>,
    pub quit: bool,
    pub backend_message: Option<String>,
    pub stream_started_at: Option<Instant>,
    pub last_stream_duration: Option<Duration>,
    pub activity_label_from: String,
    pub activity_label_to: String,
    pub activity_label_started_at: Option<Instant>,
    pub activity_label_transition_ms: u16,
}

impl Default for App {
    fn default() -> Self {
        Self {
            debate_models: Default::default(),
            debate_field: 0,
            debate_model_picker: false,
            debate_topic_draft: String::new(),
            conversation: Vec::new(),
            state: BridgeState::default(),
            input: String::new(),
            cursor: 0,
            mode: Mode::Chat,
            popup_filter: String::new(),
            popup_index: 0,
            capability_detail_id: None,
            editor_fields: Vec::new(),
            editor_index: 0,
            editor_toggle: false,
            editing_provider_id: None,
            pending_provider_delete_id: None,
            pending_sandbox_reset: false,
            active_auth_provider: None,
            settings_section: None,
            settings_edit_kind: None,
            command_index: 0,
            input_history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
            follow_tail: true,
            scroll_y: 0,
            max_scroll: 0,
            transcript_area: (0, 0, 0, 0),
            composer_area: (0, 0, 0, 0),
            composer_width: 0,
            composer_scroll: 0,
            transcript_cells: Vec::new(),
            selection_start: None,
            selection_end: None,
            transcript_context_menu: None,
            transcript_context_menu_area: (0, 0, 0, 0),
            clipboard_request: None,
            sidebar_area: (0, 0, 0, 0),
            sidebar_session_targets: Vec::new(),
            sidebar_load_request: None,
            quit: false,
            backend_message: None,
            stream_started_at: None,
            last_stream_duration: None,
            activity_label_from: String::new(),
            activity_label_to: String::new(),
            activity_label_started_at: None,
            activity_label_transition_ms: 420,
        }
    }
}

impl App {
    pub fn sync_activity_label(&mut self, label: &str) {
        if self.activity_label_to == label {
            return;
        }
        let in_transition = self.activity_label_started_at.is_some_and(|started| {
            started.elapsed().as_millis() < u128::from(self.activity_label_transition_ms)
        });
        if in_transition && same_activity_pattern(&self.activity_label_to, label) {
            // Keep one stable morph window for a family such as
            // "Reading core.rs" -> "Reading agent.rs" instead of restarting
            // the animation for every streamed update.
            self.activity_label_to = label.to_owned();
            return;
        }
        self.activity_label_from = self.activity_label_to.clone();
        self.activity_label_to = label.to_owned();
        self.activity_label_transition_ms = if self.activity_label_from.is_empty() {
            420
        } else if same_activity_pattern(&self.activity_label_from, label) {
            900
        } else {
            520
        };
        self.activity_label_started_at = Some(Instant::now());
    }

    pub fn merge_state(&mut self, mut next: BridgeState) {
        let was_streaming = self.state.is_streaming;
        let is_streaming = next.is_streaming;
        let provider_configurations_changed =
            self.state.provider_configurations != next.provider_configurations;
        let sandbox_settings_changed = self.state.sandbox_settings != next.sandbox_settings;
        let runtime_settings_changed = self.state.runtime_settings != next.runtime_settings;
        if let Some(conversation) = next.conversation.take() {
            self.conversation = conversation;
            self.clear_transcript_selection();
        }
        self.state = next;
        if provider_configurations_changed {
            self.pending_provider_delete_id = None;
        }
        if sandbox_settings_changed {
            self.pending_sandbox_reset = false;
        }
        if runtime_settings_changed {
            crate::ui::apply_runtime_theme(&self.state.runtime_settings);
        }
        match (was_streaming, is_streaming) {
            (false, true) => {
                self.stream_started_at = Some(Instant::now());
                self.last_stream_duration = None;
            }
            (true, false) => {
                self.last_stream_duration = self.stream_started_at.map(|started| started.elapsed());
                self.stream_started_at = None;
            }
            (false, false) => self.stream_started_at = None,
            _ => {}
        }
        if self.follow_tail {
            self.scroll_y = self.max_scroll;
        }
        self.clamp_popup_selection();
    }

    pub fn stream_elapsed(&self) -> Option<Duration> {
        self.stream_started_at.map(|started| started.elapsed())
    }

    pub fn latest_turn_duration(&self) -> Option<Duration> {
        self.stream_elapsed().or(self.last_stream_duration)
    }

    pub fn handle_paste(&mut self, text: &str) {
        if text.is_empty()
            || self.state.pending_shell_permission.is_some()
            || self.state.pending_native_app_permission.is_some()
        {
            return;
        }

        if self.mode == Mode::Chat {
            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
            self.insert_text(&normalized);
            return;
        }

        let single_line = text
            .chars()
            .filter(|character| !character.is_control())
            .collect::<String>();
        if single_line.is_empty() {
            return;
        }
        match self.mode {
            Mode::Debate if !self.state.is_streaming => {
                self.debate_field_mut().push_str(&single_line)
            }
            Mode::Models | Mode::Sessions | Mode::Capabilities => {
                self.popup_filter.push_str(&single_line);
                self.popup_index = 0;
            }
            Mode::AuthKey => {
                if let Some(field) = self.editor_fields.first_mut() {
                    field.push_str(&single_line);
                }
            }
            Mode::ProviderEdit | Mode::SettingsEdit => {
                if let Some(field) = self.editor_fields.get_mut(self.editor_index) {
                    field.push_str(&single_line);
                }
            }
            _ => {}
        }
    }

    fn permission_prompt_action(
        event: &KeyEvent,
        is_streaming: bool,
    ) -> Option<PermissionPromptAction> {
        if is_streaming
            && event.modifiers == KeyModifiers::CONTROL
            && event.code == KeyCode::Char('c')
        {
            return Some(PermissionPromptAction::Interrupt);
        }
        if !event.modifiers.is_empty() {
            return None;
        }
        match event.code {
            KeyCode::Char('y') | KeyCode::Enter => Some(PermissionPromptAction::Allow),
            KeyCode::Char('n') | KeyCode::Esc => Some(PermissionPromptAction::Deny),
            _ => None,
        }
    }

    fn should_interrupt_active_non_chat(event: &KeyEvent, is_streaming: bool, mode: Mode) -> bool {
        is_streaming
            && mode != Mode::Chat
            && event.modifiers == KeyModifiers::CONTROL
            && event.code == KeyCode::Char('c')
    }

    pub fn handle_key(&mut self, event: KeyEvent, backend: &mut Backend) -> anyhow::Result<()> {
        let mut event = event;
        // Preserve native macOS text-editing semantics before mapping the remaining
        // Command shortcuts onto the TUI's Control bindings.
        #[cfg(target_os = "macos")]
        if event.modifiers == KeyModifiers::SUPER {
            match event.code {
                KeyCode::Left => {
                    event.code = KeyCode::Home;
                    event.modifiers = KeyModifiers::NONE;
                }
                KeyCode::Right => {
                    event.code = KeyCode::End;
                    event.modifiers = KeyModifiers::NONE;
                }
                KeyCode::Backspace => {
                    event.code = KeyCode::Char('u');
                    event.modifiers = KeyModifiers::CONTROL;
                }
                KeyCode::Delete => {
                    event.code = KeyCode::Char('k');
                    event.modifiers = KeyModifiers::CONTROL;
                }
                _ => {
                    event.modifiers.remove(KeyModifiers::SUPER);
                    event.modifiers.insert(KeyModifiers::CONTROL);
                }
            }
        } else if event.modifiers.contains(KeyModifiers::SUPER) {
            event.modifiers.remove(KeyModifiers::SUPER);
            event.modifiers.insert(KeyModifiers::CONTROL);
        }
        if self.vim_navigation_active() && event.modifiers.is_empty() {
            event.code = match event.code {
                KeyCode::Char('j') => KeyCode::Down,
                KeyCode::Char('k') => KeyCode::Up,
                code => code,
            };
        }

        if self.state.pending_shell_permission.is_some()
            || self.state.pending_native_app_permission.is_some()
        {
            if let Some(action) = Self::permission_prompt_action(&event, self.state.is_streaming) {
                let command = match action {
                    PermissionPromptAction::Allow => {
                        if self.state.pending_native_app_permission.is_some() {
                            FrontendCommand::AllowNativeApp
                        } else {
                            FrontendCommand::AllowShell
                        }
                    }
                    PermissionPromptAction::Deny => {
                        if self.state.pending_native_app_permission.is_some() {
                            FrontendCommand::DenyNativeApp
                        } else {
                            FrontendCommand::DenyShell
                        }
                    }
                    PermissionPromptAction::Interrupt => FrontendCommand::Interrupt,
                };
                backend.send(command)?;
            }

            return Ok(());
        }

        if Self::should_interrupt_active_non_chat(&event, self.state.is_streaming, self.mode) {
            backend.send(FrontendCommand::Interrupt)?;
            return Ok(());
        }

        match self.mode {
            Mode::Debate => {
                match event.code {
                    KeyCode::Esc => self.close_popup(),
                    KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        backend.send(FrontendCommand::Interrupt)?
                    }
                    KeyCode::Enter
                        if !self.state.is_streaming
                            && self.debate_field == 0
                            && !self.popup_filter.trim().is_empty() =>
                    {
                        backend.send(FrontendCommand::StartDebate {
                            topic: self.popup_filter.clone(),
                            models: Some(self.debate_models.clone()),
                        })?;
                        self.popup_filter.clear();
                        self.popup_index = 0;
                    }
                    KeyCode::PageDown => self.popup_index = self.popup_index.saturating_add(8),
                    KeyCode::PageUp => self.popup_index = self.popup_index.saturating_sub(8),
                    _ if !self.state.is_streaming => self.edit_debate_form(event),
                    _ => {}
                }
                Ok(())
            }
            Mode::Chat => self.handle_chat_key(event, backend),
            Mode::Models => self.handle_model_key(event, backend),
            Mode::Reasoning => self.handle_reasoning_key(event, backend),
            Mode::Goal => self.handle_goal_key(event, backend),
            Mode::Sessions => self.handle_session_key(event, backend),
            Mode::Capabilities => self.handle_capability_key(event, backend),
            Mode::CapabilityDetail => self.handle_capability_detail_key(event, backend),
            Mode::Auth => self.handle_auth_key(event, backend),
            Mode::AuthKey => self.handle_auth_key_editor(event, backend),
            Mode::Providers => self.handle_providers_key(event, backend),
            Mode::ProviderEdit => self.handle_provider_edit_key(event, backend),
            Mode::Settings => self.handle_settings_key(event, backend),
            Mode::SandboxPresets => self.handle_sandbox_presets_key(event, backend),
            Mode::SandboxPolicy => self.handle_sandbox_policy_key(event, backend),
            Mode::SettingsEdit => self.handle_settings_edit_key(event, backend),
            Mode::Status => match event.code {
                KeyCode::Char('r') => backend.send(FrontendCommand::RequestAuth),
                KeyCode::Esc | KeyCode::Enter => {
                    self.close_popup();
                    Ok(())
                }
                _ => Ok(()),
            },
            Mode::Help => {
                if matches!(
                    event.code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?')
                ) {
                    self.mode = Mode::Chat;
                }
                Ok(())
            }
        }
    }

    pub fn command_suggestions(&self) -> Vec<(String, String)> {
        if !self.input.starts_with('/') || self.input.chars().any(char::is_whitespace) {
            return Vec::new();
        }
        let query = self.input.to_ascii_lowercase();
        let mut commands = COMMANDS
            .iter()
            .map(|(name, description)| ((*name).to_owned(), (*description).to_owned()))
            .collect::<Vec<_>>();
        for item in &self.state.extension_commands {
            if !commands
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(&item.command))
            {
                commands.push((item.command.clone(), item.description.clone()));
            }
        }
        commands.sort_by(|left, right| left.0.cmp(&right.0));
        commands
            .into_iter()
            .filter(|(name, _)| name.to_ascii_lowercase().starts_with(&query))
            .take(6)
            .collect()
    }

    fn extension_command_invocation(&self, text: &str) -> Option<(String, Vec<String>)> {
        let mut parts = text.split_whitespace();
        let command = parts.next()?;
        self.state
            .extension_commands
            .iter()
            .any(|item| item.command.eq_ignore_ascii_case(command))
            .then(|| {
                (
                    command.trim_start_matches('/').to_owned(),
                    parts.map(str::to_owned).collect(),
                )
            })
    }

    pub fn filtered_models(&self) -> Vec<&str> {
        let query = self.popup_filter.to_ascii_lowercase();
        let source = if !self.state.model_catalog.is_empty() {
            self.state
                .model_catalog
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>()
        } else {
            self.state
                .available_models
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        };
        source
            .into_iter()
            .filter(|id| {
                if query.is_empty() || id.to_ascii_lowercase().contains(&query) {
                    return true;
                }
                self.model_catalog_item(id).is_some_and(|item| {
                    item.provider.to_ascii_lowercase().contains(&query)
                        || item.model.to_ascii_lowercase().contains(&query)
                })
            })
            .collect()
    }

    pub fn model_catalog_item(&self, id: &str) -> Option<&ModelCatalogItem> {
        self.state.model_catalog.iter().find(|item| item.id == id)
    }

    pub fn sessions(&self) -> &[SessionSummary] {
        &self.state.saved_sessions
    }

    pub fn session_picker_items(&self) -> Vec<SessionPickerItem<'_>> {
        let mut items = Vec::new();
        let mut current_workspace_seen = false;

        for workspace in &self.state.known_workspaces {
            current_workspace_seen |= workspace.is_current;
            let grouped = self
                .state
                .workspace_session_groups
                .iter()
                .find(|group| group.workspace_id == workspace.id)
                .map(|group| group.sessions.as_slice());
            let sessions = grouped.unwrap_or_else(|| {
                if workspace.is_current {
                    self.sessions()
                } else {
                    &[]
                }
            });
            let workspace_name = if workspace.display_name.trim().is_empty() {
                Path::new(&workspace.path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| workspace.path.clone())
            } else {
                workspace.display_name.trim().to_owned()
            };
            items.extend(sessions.iter().map(|session| SessionPickerItem {
                workspace_name: workspace_name.clone(),
                workspace_current: workspace.is_current,
                session,
            }));
        }

        if !current_workspace_seen {
            let workspace_name = std::env::current_dir()
                .ok()
                .and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "Current workspace".to_owned());
            items.extend(self.sessions().iter().map(|session| SessionPickerItem {
                workspace_name: workspace_name.clone(),
                workspace_current: true,
                session,
            }));
        }

        items
    }

    pub fn filtered_session_picker_items(&self) -> Vec<SessionPickerItem<'_>> {
        let query = self.popup_filter.trim().to_ascii_lowercase();
        let items = self.session_picker_items();
        if query.is_empty() {
            return items;
        }
        items
            .into_iter()
            .filter(|item| {
                item.workspace_name.to_ascii_lowercase().contains(&query)
                    || item.session.title.to_ascii_lowercase().contains(&query)
                    || item.session.model.to_ascii_lowercase().contains(&query)
                    || item.session.id.to_ascii_lowercase().contains(&query)
            })
            .collect()
    }

    fn session_requires_load(&self, session_id: &str) -> bool {
        self.state.current_session_id.as_deref() != Some(session_id)
    }

    pub fn capability_detail(&self) -> Option<&CapabilityToggleItem> {
        let id = self.capability_detail_id.as_deref()?;
        self.state
            .available_capabilities
            .iter()
            .find(|item| item.id == id)
    }

    fn capability_toggle_available(&self) -> bool {
        !self.state.is_streaming
    }

    pub fn filtered_capabilities(&self) -> Vec<&crate::model::CapabilityToggleItem> {
        let query = self.popup_filter.to_ascii_lowercase();
        self.state
            .available_capabilities
            .iter()
            .filter(|item| {
                query.is_empty()
                    || item.name.to_ascii_lowercase().contains(&query)
                    || item.id.to_ascii_lowercase().contains(&query)
                    || item.kind.to_ascii_lowercase().contains(&query)
                    || item.description.to_ascii_lowercase().contains(&query)
                    || item
                        .source
                        .as_deref()
                        .is_some_and(|source| source.to_ascii_lowercase().contains(&query))
            })
            .collect()
    }

    fn vim_navigation_active(&self) -> bool {
        matches!(
            self.mode,
            Mode::Reasoning
                | Mode::Goal
                | Mode::Providers
                | Mode::Settings
                | Mode::SandboxPresets
                | Mode::SandboxPolicy
        )
    }

    fn handle_chat_key(&mut self, event: KeyEvent, backend: &mut Backend) -> anyhow::Result<()> {
        if event.code == KeyCode::Esc {
            if self.transcript_context_menu.is_some() {
                self.transcript_context_menu = None;
                return Ok(());
            }
            if self.selection_start.is_some() || self.selection_end.is_some() {
                self.clear_transcript_selection();
                return Ok(());
            }
        }
        if self.handle_chat_editing_key(&event) {
            return Ok(());
        }
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            match event.code {
                KeyCode::Char('c') => {
                    if self.copy_transcript_selection() {
                        return Ok(());
                    }
                    if self.state.is_streaming {
                        backend.send(FrontendCommand::Interrupt)?;
                    } else {
                        self.quit = true;
                    }
                    return Ok(());
                }
                KeyCode::Char('d') if self.input.is_empty() => {
                    self.quit = true;
                    return Ok(());
                }
                KeyCode::Char('n') => {
                    backend.send(FrontendCommand::NewSession)?;
                    self.follow_tail = true;
                    return Ok(());
                }
                KeyCode::Char('b') if self.input.is_empty() => {
                    self.scroll_up(10);
                    return Ok(());
                }
                KeyCode::Char('f') if self.input.is_empty() => {
                    self.scroll_down(10);
                    return Ok(());
                }
                KeyCode::Char('p') => {
                    self.history_up();
                    return Ok(());
                }
                _ => {}
            }
        }

        match event.code {
            KeyCode::Esc => {
                if self.state.is_streaming {
                    backend.send(FrontendCommand::Interrupt)?;
                }
            }
            KeyCode::Up if event.modifiers.contains(KeyModifiers::ALT) => self.history_up(),
            KeyCode::Down if event.modifiers.contains(KeyModifiers::ALT) => self.history_down(),
            KeyCode::F(2) | KeyCode::Char('m')
                if event.code == KeyCode::F(2) || event.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.open_models(backend)?;
            }
            KeyCode::F(3) | KeyCode::Char('s')
                if event.code == KeyCode::F(3) || event.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.open_sessions(backend)?;
            }
            KeyCode::F(4) | KeyCode::Char('r')
                if event.code == KeyCode::F(4) || event.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.open_reasoning();
            }
            KeyCode::F(5) | KeyCode::Char('k')
                if event.code == KeyCode::F(5) || event.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.open_capabilities(backend)?;
            }
            KeyCode::Char('?') if self.input.is_empty() => {
                self.mode = Mode::Help;
            }
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            KeyCode::Char('k') if self.input.is_empty() => self.scroll_up(3),
            KeyCode::Char('j') if self.input.is_empty() => self.scroll_down(3),
            KeyCode::Up if self.input.is_empty() => self.scroll_up(3),
            KeyCode::Down if self.input.is_empty() => self.scroll_down(3),
            KeyCode::Char('g') if self.input.is_empty() => self.jump_to_transcript_start(),
            KeyCode::Char('G') if self.input.is_empty() => self.jump_to_transcript_end(),
            KeyCode::End if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.jump_to_transcript_end();
            }
            KeyCode::Home if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.jump_to_transcript_start();
            }
            KeyCode::Up if !self.command_suggestions().is_empty() => {
                self.command_index = self.command_index.saturating_sub(1);
            }
            KeyCode::Down if !self.command_suggestions().is_empty() => {
                let count = self.command_suggestions().len();
                self.command_index = cmp::min(self.command_index + 1, count.saturating_sub(1));
            }
            KeyCode::Tab if !self.command_suggestions().is_empty() => {
                let suggestions = self.command_suggestions();
                let index = cmp::min(self.command_index, suggestions.len() - 1);
                self.input = format!("{} ", suggestions[index].0);
                self.cursor = self.input.chars().count();
                self.command_index = 0;
                self.reset_history_navigation();
            }
            KeyCode::Enter if event.modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_char('\n');
            }
            KeyCode::Enter => {
                let text = self.input.trim().to_owned();
                if text.is_empty() {
                    return Ok(());
                }
                if text == "/new" {
                    self.record_input_history(&text);
                    backend.send(FrontendCommand::NewSession)?;
                    self.input.clear();
                    self.cursor = 0;
                    self.command_index = 0;
                    self.follow_tail = true;
                    return Ok(());
                }
                if self.state.is_streaming && !text.starts_with('/') {
                    return Ok(());
                }
                match text.as_str() {
                    "/debate" => {
                        self.open_debate();
                        backend.send(FrontendCommand::RequestModels)?;
                    }
                    "/model" => self.open_models(backend)?,
                    "/reasoning" => self.open_reasoning(),
                    "/goal" => self.open_goal(),
                    "/sessions" => self.open_sessions(backend)?,
                    "/capabilities" => self.open_capabilities(backend)?,
                    "/settings" => self.open_settings(backend)?,
                    "/permissions" => {
                        self.open_sandbox_presets();
                        backend.send(FrontendCommand::RequestSandbox)?;
                    }
                    "/status" => self.open_status(backend)?,
                    "/login" => self.open_auth(backend)?,
                    "/provider" | "/providers" => self.open_providers(backend)?,
                    _ => {
                        if let Some((command, args)) = self.extension_command_invocation(&text) {
                            backend.send(FrontendCommand::ExtensionCommand { command, args })?;
                        } else {
                            backend.send(FrontendCommand::Submit {
                                text,
                                images: Vec::new(),
                                attachment_ids: Vec::new(),
                            })?;
                        }
                    }
                }
                let submitted = self.input.clone();
                self.record_input_history(&submitted);
                self.input.clear();
                self.cursor = 0;
                self.command_index = 0;
                self.follow_tail = true;
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => {
                self.cursor = crate::text_layout::previous_grapheme_cursor(&self.input, self.cursor)
            }
            KeyCode::Right => {
                self.cursor = crate::text_layout::next_grapheme_cursor(&self.input, self.cursor);
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Char(character)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::SUPER) =>
            {
                self.insert_char(character);
                self.command_index = 0;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_chat_editing_key(&mut self, event: &KeyEvent) -> bool {
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            match event.code {
                KeyCode::Char('a') => self.cursor = 0,
                KeyCode::Char('e') => self.cursor = self.input.chars().count(),
                KeyCode::Char('b') if !self.input.is_empty() => {
                    self.cursor =
                        crate::text_layout::previous_grapheme_cursor(&self.input, self.cursor)
                }
                KeyCode::Char('f') if !self.input.is_empty() => {
                    self.cursor = crate::text_layout::next_grapheme_cursor(&self.input, self.cursor)
                }
                KeyCode::Left if !self.input.is_empty() => self.move_word_left(),
                KeyCode::Right if !self.input.is_empty() => self.move_word_right(),
                KeyCode::Char('u') => {
                    self.delete_before_cursor();
                    self.command_index = 0;
                }
                KeyCode::Char('k') => {
                    self.delete_after_cursor();
                    self.command_index = 0;
                }
                KeyCode::Char('w') => {
                    self.delete_word_before_cursor();
                    self.command_index = 0;
                }
                _ => return false,
            }
            return true;
        }

        if event.modifiers.contains(KeyModifiers::ALT) {
            match event.code {
                KeyCode::Left => self.move_word_left(),
                KeyCode::Right => self.move_word_right(),
                KeyCode::Backspace => self.delete_word_before_cursor(),
                KeyCode::Delete => self.delete_word_after_cursor(),
                _ => return false,
            }
            return true;
        }
        if event.modifiers.is_empty() {
            match event.code {
                KeyCode::Home if !self.input.is_empty() => {
                    if self.composer_width > 0 {
                        self.cursor = crate::text_layout::visual_line_edge(
                            &self.input,
                            self.cursor,
                            self.composer_width,
                            false,
                        );
                    } else {
                        self.move_line_start();
                    }
                }
                KeyCode::End if !self.input.is_empty() => {
                    if self.composer_width > 0 {
                        self.cursor = crate::text_layout::visual_line_edge(
                            &self.input,
                            self.cursor,
                            self.composer_width,
                            true,
                        );
                    } else {
                        self.move_line_end();
                    }
                }
                KeyCode::Up if !self.input.is_empty() && self.command_suggestions().is_empty() => {
                    if self.composer_width > 0 {
                        self.cursor = crate::text_layout::move_cursor_vertical(
                            &self.input,
                            self.cursor,
                            self.composer_width,
                            -1,
                        );
                    } else if self.input.contains('\n') {
                        self.move_line_up();
                    }
                }
                KeyCode::Down
                    if !self.input.is_empty() && self.command_suggestions().is_empty() =>
                {
                    if self.composer_width > 0 {
                        self.cursor = crate::text_layout::move_cursor_vertical(
                            &self.input,
                            self.cursor,
                            self.composer_width,
                            1,
                        );
                    } else if self.input.contains('\n') {
                        self.move_line_down();
                    }
                }
                _ => return false,
            }
            return true;
        }

        false
    }

    fn open_debate(&mut self) {
        self.mode = Mode::Debate;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.debate_field = 0;
        self.debate_model_picker = false;
        self.debate_models = self
            .state
            .debate
            .as_ref()
            .map(|d| d.models.clone())
            .filter(|models| models.validate().is_ok())
            .unwrap_or_else(|| crate::debate::DebateModels::same(&self.state.active_model));
    }

    fn edit_debate_form(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::BackTab => self.debate_field = (self.debate_field + 3) % 4,
            KeyCode::Tab => self.debate_field = (self.debate_field + 1) % 4,
            KeyCode::Enter if self.debate_field > 0 => {
                self.debate_model_picker = true;
                self.mode = Mode::Models;
                self.debate_topic_draft = std::mem::take(&mut self.popup_filter);
                self.popup_index = 0;
            }
            KeyCode::Up | KeyCode::Down if self.debate_field > 0 => {
                let current = match self.debate_field {
                    1 => &self.debate_models.pro,
                    2 => &self.debate_models.con,
                    _ => &self.debate_models.jury,
                };
                let models = &self.state.available_models;
                if models.is_empty() {
                    return;
                }
                let index = models.iter().position(|m| m == current);
                let next = match (index, event.code) {
                    (Some(i), KeyCode::Up) => (i + models.len() - 1) % models.len(),
                    (Some(i), _) => (i + 1) % models.len(),
                    (None, _) => 0,
                };
                let model = models[next].clone();
                *self.debate_field_mut() = model;
            }
            KeyCode::Backspace => {
                self.debate_field_mut().pop();
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.debate_field_mut().clear()
            }
            KeyCode::Char(c)
                if !event
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                self.debate_field_mut().push(c)
            }
            _ => {}
        }
    }

    fn debate_field_mut(&mut self) -> &mut String {
        match self.debate_field {
            0 => &mut self.popup_filter,
            1 => &mut self.debate_models.pro,
            2 => &mut self.debate_models.con,
            _ => &mut self.debate_models.jury,
        }
    }

    fn handle_reasoning_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        let levels = reasoning_levels_for_model(&self.state.active_model);
        match event.code {
            KeyCode::Esc | KeyCode::F(4) => self.close_popup(),
            KeyCode::Up | KeyCode::Left => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down | KeyCode::Right => {
                self.popup_index = cmp::min(self.popup_index + 1, levels.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                if let Some(level) = levels.get(self.popup_index) {
                    backend.send(FrontendCommand::SelectReasoning {
                        level: (*level).to_owned(),
                    })?;
                    self.close_popup();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_goal_key(&mut self, event: KeyEvent, backend: &mut Backend) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => self.close_popup(),
            KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                backend.send(FrontendCommand::Interrupt)?;
            }
            KeyCode::Up | KeyCode::Left => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down | KeyCode::Right => self.popup_index = cmp::min(self.popup_index + 1, 1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                backend.send(FrontendCommand::SetGoal {
                    enabled: self.popup_index == 0,
                })?;
            }
            _ => {}
        }
        Ok(())
    }

    fn navigate_paged_picker(&mut self, code: KeyCode, count: usize) -> bool {
        let last = count.saturating_sub(1);
        let current = self.popup_index.min(last);
        let next = match code {
            KeyCode::Up => current.saturating_sub(1),
            KeyCode::Down => cmp::min(current.saturating_add(1), last),
            KeyCode::Home => 0,
            KeyCode::End => last,
            KeyCode::PageUp => current.saturating_sub(8),
            KeyCode::PageDown => cmp::min(current.saturating_add(8), last),
            _ => return false,
        };
        self.popup_index = next;
        true
    }

    fn navigate_model_picker(&mut self, code: KeyCode) -> bool {
        self.navigate_paged_picker(code, self.filtered_models().len())
    }

    fn navigate_capability_picker(&mut self, code: KeyCode) -> bool {
        self.navigate_paged_picker(code, self.filtered_capabilities().len())
    }

    fn handle_model_key(&mut self, event: KeyEvent, backend: &mut Backend) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc | KeyCode::F(2) => {
                if self.debate_model_picker {
                    self.debate_model_picker = false;
                    self.mode = Mode::Debate;
                    self.popup_filter = std::mem::take(&mut self.debate_topic_draft);
                    self.popup_index = 0;
                } else {
                    self.close_popup();
                }
            }
            code @ (KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown) => {
                self.navigate_model_picker(code);
            }
            KeyCode::Enter => {
                let models = self.filtered_models();
                if let Some(model) = models.get(self.popup_index).copied() {
                    let model = model.to_owned();
                    if self.debate_model_picker {
                        match self.debate_field {
                            1 => self.debate_models.pro = model,
                            2 => self.debate_models.con = model,
                            3 => self.debate_models.jury = model,
                            _ => {}
                        }
                        self.debate_model_picker = false;
                        self.mode = Mode::Debate;
                        self.popup_filter = std::mem::take(&mut self.debate_topic_draft);
                        self.popup_index = 0;
                        return Ok(());
                    }
                    backend.send(FrontendCommand::SelectModel { model })?;
                    self.close_popup();
                }
            }
            KeyCode::Backspace => {
                self.popup_filter.pop();
                self.popup_index = 0;
            }
            KeyCode::Char(character)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::SUPER) =>
            {
                self.popup_filter.push(character);
                self.popup_index = 0;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_session_key(&mut self, event: KeyEvent, backend: &mut Backend) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc | KeyCode::F(3) => self.close_popup(),
            KeyCode::Up => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down => {
                let count = self.filtered_session_picker_items().len();
                self.popup_index = cmp::min(self.popup_index + 1, count.saturating_sub(1));
            }
            KeyCode::Home => self.popup_index = 0,
            KeyCode::End => {
                self.popup_index = self.filtered_session_picker_items().len().saturating_sub(1);
            }
            KeyCode::PageUp => self.popup_index = self.popup_index.saturating_sub(8),
            KeyCode::PageDown => {
                let count = self.filtered_session_picker_items().len();
                self.popup_index =
                    cmp::min(self.popup_index.saturating_add(8), count.saturating_sub(1));
            }
            KeyCode::Char('n') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                backend.send(FrontendCommand::NewSession)?;
                self.close_popup();
            }
            KeyCode::Enter => {
                let id = self
                    .filtered_session_picker_items()
                    .get(self.popup_index)
                    .map(|item| item.session.id.clone());
                if let Some(session_id) = id {
                    if !self.session_requires_load(&session_id) {
                        self.close_popup();
                        return Ok(());
                    }
                    backend.send(FrontendCommand::LoadSession { session_id })?;
                    self.close_popup();
                    self.follow_tail = true;
                }
            }
            KeyCode::Backspace => {
                self.popup_filter.pop();
                self.popup_index = 0;
            }
            KeyCode::Char(character)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::SUPER) =>
            {
                self.popup_filter.push(character);
                self.popup_index = 0;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_capability_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc | KeyCode::F(5) => self.close_popup(),
            code @ (KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown) => {
                self.navigate_capability_picker(code);
            }
            KeyCode::Enter => {
                let items = self.filtered_capabilities();
                if let Some(item) = items.get(self.popup_index) {
                    self.capability_detail_id = Some(item.id.clone());
                    self.mode = Mode::CapabilityDetail;
                }
            }
            KeyCode::Char(' ') if self.capability_toggle_available() => {
                let items = self.filtered_capabilities();
                if let Some(item) = items.get(self.popup_index) {
                    backend.send(FrontendCommand::ToggleCapability {
                        id: item.id.clone(),
                    })?;
                }
            }
            KeyCode::Char('r') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                backend.send(FrontendCommand::RequestCapabilities)?;
            }
            KeyCode::Backspace => {
                self.popup_filter.pop();
                self.popup_index = 0;
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.popup_filter.clear();
                self.popup_index = 0;
            }
            KeyCode::Char(character)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::SUPER) =>
            {
                self.popup_filter.push(character);
                self.popup_index = 0;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_capability_detail_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc | KeyCode::Enter => self.mode = Mode::Capabilities,
            KeyCode::F(5) => self.close_popup(),
            KeyCode::Char(' ') if self.capability_toggle_available() => {
                if let Some(id) = self.capability_detail_id.clone() {
                    backend.send(FrontendCommand::ToggleCapability { id })?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_auth_key(&mut self, event: KeyEvent, backend: &mut Backend) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => self.close_popup(),
            KeyCode::Up => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down => {
                self.popup_index = cmp::min(
                    self.popup_index + 1,
                    self.state.auth_providers.len().saturating_sub(1),
                );
            }
            KeyCode::Char('r') => backend.send(FrontendCommand::RequestAuth)?,
            KeyCode::Char('l') | KeyCode::Enter if !self.state.auth_working => {
                if let Some(provider) = self.selected_auth_provider() {
                    backend.send(FrontendCommand::AuthLogin { provider })?;
                }
            }
            KeyCode::Char('x') if !self.state.auth_working => {
                if let Some(provider) = self.selected_auth_provider() {
                    backend.send(FrontendCommand::AuthLogout { provider })?;
                }
            }
            KeyCode::Char('k') if !self.state.auth_working => {
                if let Some(provider) = self.selected_auth_provider() {
                    self.active_auth_provider = Some(provider);
                    self.editor_fields = vec![String::new()];
                    self.editor_index = 0;
                    self.mode = Mode::AuthKey;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_auth_key_editor(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => {
                self.editor_fields.clear();
                self.mode = Mode::Auth;
            }
            KeyCode::Enter => {
                if let (Some(provider), Some(key)) = (
                    self.active_auth_provider.clone(),
                    self.editor_fields.first().cloned(),
                ) {
                    if key.trim().is_empty() {
                        self.backend_message = Some("API key cannot be empty".into());
                    } else {
                        backend.send(FrontendCommand::AuthSetApiKey { provider, key })?;
                        self.editor_fields.clear();
                        self.mode = Mode::Auth;
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(field) = self.editor_fields.first_mut() {
                    field.pop();
                }
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(field) = self.editor_fields.first_mut() {
                    field.clear();
                }
            }
            KeyCode::Char(character)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::SUPER) =>
            {
                if let Some(field) = self.editor_fields.first_mut() {
                    field.push(character);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn clear_editor(&mut self) {
        self.editor_fields.clear();
        self.editor_index = 0;
        self.editor_toggle = false;
        self.editing_provider_id = None;
        self.settings_edit_kind = None;
    }

    fn close_popup(&mut self) {
        self.mode = Mode::Chat;
        self.debate_model_picker = false;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.capability_detail_id = None;
        self.active_auth_provider = None;
        self.pending_provider_delete_id = None;
        self.pending_sandbox_reset = false;
        self.settings_section = None;
        self.clear_editor();
    }

    fn clamp_popup_selection(&mut self) {
        let count = match self.mode {
            Mode::Models => self.filtered_models().len(),
            Mode::Reasoning => reasoning_levels_for_model(&self.state.active_model).len(),
            Mode::Goal => 2,
            Mode::Sessions => self.filtered_session_picker_items().len(),
            Mode::Capabilities => self.filtered_capabilities().len(),
            Mode::Auth => self.state.auth_providers.len(),
            Mode::Providers => self.state.provider_configurations.len(),
            Mode::Settings => self.settings_row_count(),
            Mode::SandboxPresets => SANDBOX_PRESET_ROW_COUNT,
            Mode::SandboxPolicy => self.settings_detail_count(),
            _ => return,
        };
        self.popup_index = cmp::min(self.popup_index, count.saturating_sub(1));
    }

    fn jump_to_transcript_start(&mut self) {
        self.clear_transcript_selection();
        self.follow_tail = false;
        self.scroll_y = 0;
    }

    fn jump_to_transcript_end(&mut self) {
        self.clear_transcript_selection();
        self.follow_tail = true;
        self.scroll_y = self.max_scroll;
    }

    fn scroll_up(&mut self, amount: u16) {
        self.clear_transcript_selection();
        if self.follow_tail {
            self.scroll_y = self.max_scroll;
        }
        self.follow_tail = false;
        self.scroll_y = self.scroll_y.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: u16) {
        self.clear_transcript_selection();
        self.scroll_y = cmp::min(self.scroll_y.saturating_add(amount), self.max_scroll);
        self.follow_tail = self.scroll_y >= self.max_scroll;
    }

    fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.reset_history_navigation();
        let byte = byte_index(&self.input, self.cursor);
        self.input.insert_str(byte, text);
        self.cursor += text.chars().count();
        self.command_index = 0;
    }

    fn insert_char(&mut self, character: char) {
        self.reset_history_navigation();
        let byte = byte_index(&self.input, self.cursor);
        self.input.insert(byte, character);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.reset_history_navigation();
        let previous = crate::text_layout::previous_grapheme_cursor(&self.input, self.cursor);
        let start = byte_index(&self.input, previous);
        let end = byte_index(&self.input, self.cursor);
        self.input.replace_range(start..end, "");
        self.command_index = 0;
        self.cursor = previous;
    }

    fn delete(&mut self) {
        if self.cursor >= self.input.chars().count() {
            return;
        }
        self.reset_history_navigation();
        let next = crate::text_layout::next_grapheme_cursor(&self.input, self.cursor);
        let start = byte_index(&self.input, self.cursor);
        let end = byte_index(&self.input, next);
        self.input.replace_range(start..end, "");
        self.command_index = 0;
    }

    fn move_word_left(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let mut cursor = self.cursor.min(characters.len());
        while cursor > 0 && characters[cursor - 1].is_whitespace() {
            cursor -= 1;
        }
        while cursor > 0 && !characters[cursor - 1].is_whitespace() {
            cursor -= 1;
        }
        self.cursor = cursor;
    }

    fn move_word_right(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let mut cursor = self.cursor.min(characters.len());
        while cursor < characters.len() && characters[cursor].is_whitespace() {
            cursor += 1;
        }
        while cursor < characters.len() && !characters[cursor].is_whitespace() {
            cursor += 1;
        }
        self.cursor = cursor;
    }

    fn move_line_start(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let (start, _) = line_bounds(&characters, self.cursor);
        self.cursor = start;
    }

    fn move_line_end(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let (_, end) = line_bounds(&characters, self.cursor);
        self.cursor = end;
    }

    fn move_line_up(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(characters.len());
        let (start, _) = line_bounds(&characters, cursor);
        if start == 0 {
            return;
        }
        let column = cursor - start;
        let previous_end = start - 1;
        let (previous_start, _) = line_bounds(&characters, previous_end);
        self.cursor = previous_start + column.min(previous_end - previous_start);
    }

    fn move_line_down(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(characters.len());
        let (start, end) = line_bounds(&characters, cursor);
        if end >= characters.len() {
            return;
        }
        let column = cursor - start;
        let next_start = end + 1;
        let (_, next_end) = line_bounds(&characters, next_start);
        self.cursor = next_start + column.min(next_end - next_start);
    }

    fn delete_word_before_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.reset_history_navigation();
        let end_cursor = self.cursor.min(self.input.chars().count());
        let end = byte_index(&self.input, end_cursor);
        self.move_word_left();
        let start = byte_index(&self.input, self.cursor);
        self.input.replace_range(start..end, "");
        self.command_index = 0;
    }

    fn delete_word_after_cursor(&mut self) {
        let start_cursor = self.cursor.min(self.input.chars().count());
        if start_cursor >= self.input.chars().count() {
            return;
        }
        self.reset_history_navigation();
        self.move_word_right();
        let end_cursor = self.cursor;
        let start = byte_index(&self.input, start_cursor);
        let end = byte_index(&self.input, end_cursor);
        self.input.replace_range(start..end, "");
        self.cursor = start_cursor;
        self.command_index = 0;
    }

    fn delete_before_cursor(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(characters.len());
        let (line_start, _) = line_bounds(&characters, cursor);
        if cursor == line_start {
            return;
        }
        self.reset_history_navigation();
        let start = byte_index(&self.input, line_start);
        let end = byte_index(&self.input, cursor);
        self.input.replace_range(start..end, "");
        self.cursor = line_start;
    }

    fn delete_after_cursor(&mut self) {
        let characters = self.input.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(characters.len());
        let (_, line_end) = line_bounds(&characters, cursor);
        if cursor >= line_end {
            return;
        }
        self.reset_history_navigation();
        let start = byte_index(&self.input, cursor);
        let end = byte_index(&self.input, line_end);
        self.input.replace_range(start..end, "");
        self.cursor = cursor;
    }

    fn record_input_history(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self
            .input_history
            .last()
            .is_some_and(|previous| previous == text)
        {
            self.reset_history_navigation();
            return;
        }
        self.input_history.push(text.to_owned());
        const MAX_INPUT_HISTORY: usize = 100;
        if self.input_history.len() > MAX_INPUT_HISTORY {
            let excess = self.input_history.len() - MAX_INPUT_HISTORY;
            self.input_history.drain(..excess);
        }
        self.reset_history_navigation();
    }

    fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            self.history_draft = self.input.clone();
            self.history_index = Some(self.input_history.len() - 1);
        } else if let Some(index) = self.history_index {
            self.history_index = Some(index.saturating_sub(1));
        }
        if let Some(index) = self.history_index {
            self.input = self.input_history[index].clone();
            self.cursor = self.input.chars().count();
        }
    }

    fn history_down(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.input_history.len() {
            self.history_index = Some(index + 1);
            self.input = self.input_history[index + 1].clone();
            self.cursor = self.input.chars().count();
        } else {
            let draft = std::mem::take(&mut self.history_draft);
            self.reset_history_navigation();
            self.input = draft;
            self.cursor = self.input.chars().count();
        }
    }

    fn reset_history_navigation(&mut self) {
        self.history_index = None;
        self.history_draft.clear();
    }
}

fn same_activity_pattern(left: &str, right: &str) -> bool {
    left.split_whitespace().next().is_some_and(|left_head| {
        right
            .split_whitespace()
            .next()
            .is_some_and(|right_head| left_head.eq_ignore_ascii_case(right_head))
    })
}

pub const SANDBOX_PRESET_ROW_COUNT: usize = 4;

const COMMANDS: &[(&str, &str)] = &[
    (
        "/debate",
        "Debate with independent Pro, Con and Jury models",
    ),
    ("/new", "Start a new session"),
    ("/model", "Select model"),
    ("/reasoning", "Select reasoning level"),
    ("/login", "Manage provider authentication"),
    ("/provider", "Manage custom API endpoints"),
    ("/providers", "Manage custom API endpoints"),
    ("/settings", "Runtime and application settings"),
    ("/permissions", "Sandbox and permission settings"),
    ("/sessions", "Browse saved chats"),
    ("/capabilities", "Toggle skills, capabilities, and MCP"),
    ("/skyline", "Attach or detach Skyline coordination"),
    ("/image", "Queue an image for the next turn"),
    ("/compact", "Compact model context now"),
    ("/context", "Show or override model context length"),
    ("/status", "Show detailed runtime and usage status"),
    (
        "/goal",
        "Continue until a strict success judge accepts concrete evidence",
    ),
    ("/agents", "Opt in to adaptive multi-agent execution"),
    (
        "/autonomy",
        "Choose manual, fixed-goal, or self-directed continuation",
    ),
    ("/attach", "Attach an optional capability"),
    ("/detach", "Detach an optional capability"),
    ("/allow", "Allow pending shell command once"),
    ("/deny", "Deny pending shell command"),
    ("/help", "Show commands"),
    ("/clear", "Dismiss latest system notice"),
];

fn byte_index(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

fn line_bounds(characters: &[char], cursor: usize) -> (usize, usize) {
    let cursor = cursor.min(characters.len());
    let start = characters[..cursor]
        .iter()
        .rposition(|character| *character == '\n')
        .map_or(0, |index| index + 1);
    let end = characters[cursor..]
        .iter()
        .position(|character| *character == '\n')
        .map_or(characters.len(), |offset| cursor + offset);
    (start, end)
}
