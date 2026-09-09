use std::{
    cmp,
    time::{Duration, Instant},
};

mod selection;
pub use selection::TranscriptContextMenu;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(test)]
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::{
    backend::Backend,
    model::{
        BridgeState, CapabilityToggleItem, ConversationEntry, FrontendCommand,
        ProviderConfigurationItem, REASONING_LEVELS, SandboxAction, SessionSummary,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chat,
    Debate,
    Models,
    Reasoning,
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
    WorkspacePath,
    Network,
    Environment { original_key: Option<String> },
    Secret,
    Limit { name: String },
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
    pub active_auth_provider: Option<String>,
    pub settings_section: Option<SettingsSection>,
    pub settings_edit_kind: Option<SettingsEditKind>,
    pub command_index: usize,
    pub follow_tail: bool,
    pub scroll_y: u16,
    pub max_scroll: u16,
    pub transcript_area: (u16, u16, u16, u16),
    pub transcript_cells: Vec<Vec<String>>,
    pub selection_start: Option<(u16, u16)>,
    pub selection_end: Option<(u16, u16)>,
    pub transcript_context_menu: Option<TranscriptContextMenu>,
    pub transcript_context_menu_area: (u16, u16, u16, u16),
    pub(crate) clipboard_request: Option<String>,
    pub quit: bool,
    pub backend_message: Option<String>,
    pub stream_started_at: Option<Instant>,
    pub last_stream_duration: Option<Duration>,
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
            active_auth_provider: None,
            settings_section: None,
            settings_edit_kind: None,
            command_index: 0,
            follow_tail: true,
            scroll_y: 0,
            max_scroll: 0,
            transcript_area: (0, 0, 0, 0),
            transcript_cells: Vec::new(),
            selection_start: None,
            selection_end: None,
            transcript_context_menu: None,
            transcript_context_menu_area: (0, 0, 0, 0),
            clipboard_request: None,
            quit: false,
            backend_message: None,
            stream_started_at: None,
            last_stream_duration: None,
        }
    }
}

impl App {
    pub fn merge_state(&mut self, mut next: BridgeState) {
        let was_streaming = self.state.is_streaming;
        let is_streaming = next.is_streaming;
        if let Some(conversation) = next.conversation.take() {
            self.conversation = conversation;
            self.clear_transcript_selection();
        }
        self.state = next;
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

    pub fn handle_key(&mut self, event: KeyEvent, backend: &mut Backend) -> anyhow::Result<()> {
        let mut event = event;
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
            match event.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    let command = if self.state.pending_native_app_permission.is_some() {
                        FrontendCommand::AllowNativeApp
                    } else {
                        FrontendCommand::AllowShell
                    };
                    backend.send(command)?;
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    let command = if self.state.pending_native_app_permission.is_some() {
                        FrontendCommand::DenyNativeApp
                    } else {
                        FrontendCommand::DenyShell
                    };
                    backend.send(command)?;
                }
                _ => {}
            }
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

    pub fn command_suggestions(&self) -> Vec<(&'static str, &'static str)> {
        if !self.input.starts_with('/') || self.input.chars().any(char::is_whitespace) {
            return Vec::new();
        }
        let query = self.input.to_ascii_lowercase();
        COMMANDS
            .iter()
            .copied()
            .filter(|(name, _)| name.starts_with(&query))
            .take(6)
            .collect()
    }

    pub fn filtered_models(&self) -> Vec<&str> {
        let query = self.popup_filter.to_ascii_lowercase();
        self.state
            .available_models
            .iter()
            .filter(|model| query.is_empty() || model.to_ascii_lowercase().contains(&query))
            .map(String::as_str)
            .collect()
    }

    pub fn sessions(&self) -> &[SessionSummary] {
        &self.state.saved_sessions
    }

    pub fn capability_detail(&self) -> Option<&CapabilityToggleItem> {
        let id = self.capability_detail_id.as_deref()?;
        self.state
            .available_capabilities
            .iter()
            .find(|item| item.id == id)
    }

    pub fn filtered_capabilities(&self) -> Vec<&crate::model::CapabilityToggleItem> {
        let query = self.popup_filter.to_ascii_lowercase();
        self.state
            .available_capabilities
            .iter()
            .filter(|item| {
                query.is_empty()
                    || item.name.to_ascii_lowercase().contains(&query)
                    || item.kind.to_ascii_lowercase().contains(&query)
                    || item.description.to_ascii_lowercase().contains(&query)
            })
            .collect()
    }

    fn vim_navigation_active(&self) -> bool {
        match self.mode {
            Mode::Models | Mode::Capabilities => self.popup_filter.is_empty(),
            Mode::Reasoning
            | Mode::Sessions
            | Mode::Providers
            | Mode::Settings
            | Mode::SandboxPresets
            | Mode::SandboxPolicy => true,
            _ => false,
        }
    }

    fn handle_chat_key(&mut self, event: KeyEvent, backend: &mut Backend) -> anyhow::Result<()> {
        if event.code == KeyCode::Esc && self.transcript_context_menu.is_some() {
            self.transcript_context_menu = None;
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
                _ => {}
            }
        }

        match event.code {
            KeyCode::Esc => {
                if self.state.is_streaming {
                    backend.send(FrontendCommand::Interrupt)?;
                }
            }
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
            KeyCode::Char('g') if self.input.is_empty() => {
                self.follow_tail = false;
                self.scroll_y = 0;
            }
            KeyCode::Char('G') if self.input.is_empty() => {
                self.follow_tail = true;
                self.scroll_y = self.max_scroll;
            }
            KeyCode::End if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.follow_tail = true;
                self.scroll_y = self.max_scroll;
            }
            KeyCode::Home if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.follow_tail = false;
                self.scroll_y = 0;
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
                    "/sessions" => self.open_sessions(backend)?,
                    "/capabilities" => self.open_capabilities(backend)?,
                    "/settings" => self.open_settings(backend)?,
                    "/status" => self.open_status(backend)?,
                    "/login" => self.open_auth(backend)?,
                    "/provider" | "/providers" => self.open_providers(backend)?,
                    _ => backend.send(FrontendCommand::Submit { text })?,
                }
                self.input.clear();
                self.cursor = 0;
                self.command_index = 0;
                self.follow_tail = true;
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                self.cursor = cmp::min(self.cursor + 1, self.input.chars().count());
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
        match event.code {
            KeyCode::Esc | KeyCode::F(4) => self.close_popup(),
            KeyCode::Up | KeyCode::Left => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down | KeyCode::Right => {
                self.popup_index = cmp::min(
                    self.popup_index + 1,
                    REASONING_LEVELS.len().saturating_sub(1),
                );
            }
            KeyCode::Enter => {
                if let Some(level) = REASONING_LEVELS.get(self.popup_index) {
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
            KeyCode::Up => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down => {
                let count = self.filtered_models().len();
                self.popup_index = cmp::min(self.popup_index + 1, count.saturating_sub(1));
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
                self.popup_index = cmp::min(
                    self.popup_index + 1,
                    self.state.saved_sessions.len().saturating_sub(1),
                );
            }
            KeyCode::Char('n') => {
                backend.send(FrontendCommand::NewSession)?;
                self.close_popup();
            }
            KeyCode::Enter => {
                if let Some(session) = self.state.saved_sessions.get(self.popup_index) {
                    let id = session.id.clone();
                    backend.send(FrontendCommand::LoadSession { session_id: id })?;
                    self.close_popup();
                    self.follow_tail = true;
                }
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
            KeyCode::Up => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down => {
                let count = self.filtered_capabilities().len();
                self.popup_index = cmp::min(self.popup_index + 1, count.saturating_sub(1));
            }
            KeyCode::Enter => {
                let items = self.filtered_capabilities();
                if let Some(item) = items.get(self.popup_index) {
                    self.capability_detail_id = Some(item.id.clone());
                    self.mode = Mode::CapabilityDetail;
                }
            }
            KeyCode::Char(' ') => {
                let items = self.filtered_capabilities();
                if let Some(item) = items.get(self.popup_index) {
                    backend.send(FrontendCommand::ToggleCapability {
                        id: item.id.clone(),
                    })?;
                }
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
            KeyCode::Char(' ') => {
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

    fn handle_providers_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => self.close_popup(),
            KeyCode::Up => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down => {
                self.popup_index = cmp::min(
                    self.popup_index + 1,
                    self.state.provider_configurations.len().saturating_sub(1),
                );
            }
            KeyCode::Char('r') => backend.send(FrontendCommand::RequestProviders)?,
            KeyCode::Char('n') if !self.state.providers_working => self.open_provider_editor(None),
            KeyCode::Enter if !self.state.providers_working => {
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
                if let Some(provider) = self.state.provider_configurations.get(self.popup_index) {
                    backend.send(FrontendCommand::RemoveProvider {
                        id: provider.id.clone(),
                    })?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_provider_edit_key(
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

    fn handle_settings_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        let row_count = self.settings_row_count();
        match event.code {
            KeyCode::Esc => self.close_popup(),
            KeyCode::Up => self.popup_index = self.popup_index.saturating_sub(1),
            KeyCode::Down => {
                self.popup_index = cmp::min(self.popup_index + 1, row_count.saturating_sub(1))
            }
            KeyCode::Char('r') => backend.send(FrontendCommand::RequestSettings)?,
            KeyCode::Enter | KeyCode::Char(' ') if !self.state.settings_working => {
                if self.openai_provider_active() && self.popup_index == 0 {
                    if self.openai_flex_available() {
                        backend.send(FrontendCommand::SetOpenAiFlex {
                            enabled: !self.state.openai_flex,
                        })?;
                    }
                } else if self.popup_index == self.foundation_settings_row_index() {
                    backend.send(FrontendCommand::SetFoundationMemory {
                        enabled: !self.state.foundation_memory_enabled,
                    })?;
                } else {
                    self.open_sandbox_presets();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_sandbox_presets_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => {
                self.mode = Mode::Settings;
                self.popup_index = self.sandbox_settings_row_index();
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

    fn handle_sandbox_policy_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        let section = self.settings_section;
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
                match self.popup_index {
                    0 => self.toggle_execution_mode(backend)?,
                    1 => self.toggle_auto_approve(backend)?,
                    2 => self.toggle_scratch(backend)?,
                    3 => backend.send(FrontendCommand::UpdateSandbox {
                        action: SandboxAction::Reset,
                    })?,
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

    fn handle_settings_edit_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        match event.code {
            KeyCode::Esc => {
                self.clear_editor();
                self.mode = Mode::SandboxPolicy;
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

    fn open_models(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Models;
        self.popup_filter.clear();
        self.popup_index = 0;
        backend.send(FrontendCommand::RequestModels)
    }

    fn open_sessions(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Sessions;
        self.popup_filter.clear();
        self.popup_index = 0;
        backend.send(FrontendCommand::RequestSessions)
    }

    fn open_reasoning(&mut self) {
        self.mode = Mode::Reasoning;
        self.popup_filter.clear();
        self.popup_index = REASONING_LEVELS
            .iter()
            .position(|level| *level == self.state.active_reasoning_level)
            .unwrap_or(0);
    }

    fn open_capabilities(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Capabilities;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.capability_detail_id = None;
        backend.send(FrontendCommand::RequestCapabilities)
    }

    fn open_auth(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Auth;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.active_auth_provider = None;
        backend.send(FrontendCommand::RequestAuth)
    }

    fn open_providers(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Providers;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.clear_editor();
        backend.send(FrontendCommand::RequestProviders)
    }

    fn open_provider_editor(&mut self, provider: Option<ProviderConfigurationItem>) {
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

    fn provider_field_editable(&self) -> bool {
        self.editor_index < 2 && !(self.editor_index == 0 && self.editing_provider_id.is_some())
    }

    fn open_settings(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Settings;
        self.popup_filter.clear();
        self.popup_index = 0;
        self.settings_section = None;
        self.settings_edit_kind = None;
        backend.send(FrontendCommand::RequestSettings)
    }

    fn open_status(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        self.mode = Mode::Status;
        self.popup_filter.clear();
        self.popup_index = 0;
        backend.send(FrontendCommand::RequestAuth)
    }

    fn open_sandbox_presets(&mut self) {
        self.mode = Mode::SandboxPresets;
        self.popup_index = 0;
    }

    fn open_sandbox_policy(&mut self) {
        self.settings_section = Some(SettingsSection::Core);
        self.settings_edit_kind = None;
        self.popup_index = 0;
        self.mode = Mode::SandboxPolicy;
    }

    fn open_settings_editor(&mut self, kind: SettingsEditKind, fields: Vec<String>) {
        self.settings_edit_kind = Some(kind);
        self.editor_fields = fields;
        self.editor_index = 0;
        self.mode = Mode::SettingsEdit;
    }

    fn selected_auth_provider(&self) -> Option<String> {
        self.state
            .auth_providers
            .get(self.popup_index)
            .map(|item| item.provider.clone())
    }

    fn toggle_scratch(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        if let Some(settings) = self.state.sandbox_settings.as_ref() {
            backend.send(FrontendCommand::UpdateSandbox {
                action: SandboxAction::SetScratchWritable {
                    enabled: !settings.scratch_writable,
                },
            })?;
        }
        Ok(())
    }

    fn apply_sandbox_preset(&mut self, preset: &str, backend: &mut Backend) -> anyhow::Result<()> {
        backend.send(FrontendCommand::UpdateSandbox {
            action: SandboxAction::ApplyPreset {
                preset: preset.into(),
            },
        })
    }

    fn toggle_execution_mode(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
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

    fn toggle_auto_approve(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        if let Some(settings) = self.state.sandbox_settings.as_ref() {
            backend.send(FrontendCommand::UpdateSandbox {
                action: SandboxAction::SetAutoApprove {
                    enabled: !settings.auto_approve,
                },
            })?;
        }
        Ok(())
    }

    fn cycle_settings_section(&mut self, delta: isize) {
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

    pub(crate) fn openai_provider_active(&self) -> bool {
        self.state
            .active_model
            .split_once('/')
            .is_some_and(|(provider, _)| provider == "openai")
    }

    pub(crate) fn openai_flex_available(&self) -> bool {
        self.openai_provider_active()
            && self.state.auth_providers.iter().any(|provider| {
                provider.provider == "openai"
                    && provider.authenticated
                    && matches!(provider.method.as_str(), "api-key" | "environment")
            })
    }

    pub(crate) fn settings_row_count(&self) -> usize {
        2 + usize::from(self.openai_provider_active())
    }

    fn foundation_settings_row_index(&self) -> usize {
        usize::from(self.openai_provider_active())
    }

    fn sandbox_settings_row_index(&self) -> usize {
        self.foundation_settings_row_index() + 1
    }

    fn selected_limit(&self) -> Option<(&'static str, u64)> {
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

    fn delete_settings_item(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
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

    fn submit_settings_editor(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        let Some(kind) = self.settings_edit_kind.clone() else {
            return Ok(());
        };
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
        };
        backend.send(FrontendCommand::UpdateSandbox { action })?;
        self.clear_editor();
        self.mode = Mode::SandboxPolicy;
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
        self.settings_section = None;
        self.clear_editor();
    }

    fn clamp_popup_selection(&mut self) {
        let count = match self.mode {
            Mode::Models => self.filtered_models().len(),
            Mode::Reasoning => REASONING_LEVELS.len(),
            Mode::Sessions => self.state.saved_sessions.len(),
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

    fn insert_char(&mut self, character: char) {
        let byte = byte_index(&self.input, self.cursor);
        self.input.insert(byte, character);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = byte_index(&self.input, self.cursor - 1);
        let end = byte_index(&self.input, self.cursor);
        self.input.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.input.chars().count() {
            return;
        }
        let start = byte_index(&self.input, self.cursor);
        let end = byte_index(&self.input, self.cursor + 1);
        self.input.replace_range(start..end, "");
    }
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
    ("/settings", "Runtime and sandbox settings"),
    ("/sessions", "Browse saved chats"),
    ("/capabilities", "Toggle skills, capabilities, and MCP"),
    ("/image", "Queue an image for the next turn"),
    ("/compact", "Compact model context now"),
    ("/context", "Show or override model context length"),
    ("/status", "Show detailed runtime and usage status"),
    ("/attach", "Attach an optional capability"),
    ("/detach", "Detach an optional capability"),
    ("/allow", "Allow pending shell command once"),
    ("/deny", "Deny pending shell command"),
    ("/help", "Show commands"),
    ("/clear", "Clear current response"),
];

fn byte_index(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debate_form_selects_each_role_independently_and_restores_saved_models() {
        let mut app = App::default();
        app.state.active_model = "p/default".into();
        app.state.available_models = vec![
            "p/default".into(),
            "p/pro".into(),
            "p/con".into(),
            "p/jury".into(),
        ];
        app.open_debate();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.edit_debate_form(key(KeyCode::Char('T')));
        app.edit_debate_form(key(KeyCode::Tab));
        app.edit_debate_form(key(KeyCode::Down));
        app.edit_debate_form(key(KeyCode::Tab));
        app.edit_debate_form(key(KeyCode::Down));
        app.edit_debate_form(key(KeyCode::Down));
        app.edit_debate_form(key(KeyCode::Tab));
        app.edit_debate_form(key(KeyCode::Up));
        assert_eq!(app.popup_filter, "T");
        assert_eq!(app.debate_models.pro, "p/pro");
        assert_eq!(app.debate_models.con, "p/con");
        assert_eq!(app.debate_models.jury, "p/jury");
        let saved = app.debate_models.clone();
        let mut debate = crate::debate::DebateState::default();
        debate.models = saved.clone();
        app.state.debate = Some(debate);
        app.close_popup();
        app.open_debate();
        assert_eq!(app.debate_models, saved);
        app.debate_field = 3;
        app.edit_debate_form(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.edit_debate_form(key(KeyCode::Char('x')));
        assert_eq!(app.debate_models.jury, "x");
        assert_eq!(app.debate_models.pro, "p/pro");
    }

    #[test]
    fn streaming_clock_follows_state_transitions() {
        let mut app = App::default();
        app.merge_state(BridgeState {
            is_streaming: true,
            ..BridgeState::default()
        });
        let started = app.stream_started_at.expect("stream should start a clock");

        app.merge_state(BridgeState {
            is_streaming: true,
            ..BridgeState::default()
        });
        assert_eq!(app.stream_started_at, Some(started));

        app.merge_state(BridgeState::default());
        assert!(app.stream_started_at.is_none());
        assert!(app.last_stream_duration.is_some());
        assert_eq!(app.latest_turn_duration(), app.last_stream_duration);
    }

    #[test]
    fn new_command_is_suggested() {
        let app = App {
            input: "/n".into(),
            ..App::default()
        };

        assert!(
            app.command_suggestions()
                .iter()
                .any(|(name, _)| *name == "/new")
        );
    }

    #[test]
    fn mouse_wheel_scrolls_the_transcript_and_restores_tail_following() {
        let mut app = App {
            max_scroll: 30,
            follow_tail: true,
            scroll_y: 30,
            transcript_area: (0, 0, 80, 20),
            ..App::default()
        };

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 4,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_y, 27);
        assert!(!app.follow_tail);

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 4,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_y, 30);
        assert!(app.follow_tail);
    }

    #[test]
    fn transcript_selection_is_scoped_clamped_and_direction_independent() {
        let mut app = App {
            transcript_area: (10, 4, 8, 3),
            transcript_cells: vec![
                "abcdefgh".chars().map(|ch| ch.to_string()).collect(),
                "ijklmnop".chars().map(|ch| ch.to_string()).collect(),
                "qrstuvwx".chars().map(|ch| ch.to_string()).collect(),
            ],
            ..App::default()
        };

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 12,
            row: 4,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 15,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            app.selected_transcript_text().as_deref(),
            Some("cdefgh\nijklmn")
        );

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 15,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 12,
            row: 4,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            app.selected_transcript_text().as_deref(),
            Some("cdefgh\nijklmn")
        );

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 40,
            row: 40,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selection_end, Some((17, 6)));
    }

    #[test]
    fn sidebar_clicks_do_not_start_selection_and_context_menu_copies_selection() {
        let mut app = App {
            transcript_area: (20, 5, 6, 2),
            transcript_cells: vec![
                "hello!".chars().map(|ch| ch.to_string()).collect(),
                "world!".chars().map(|ch| ch.to_string()).collect(),
            ],
            ..App::default()
        };

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.selection_start.is_none());

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 20,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 24,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 22,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.transcript_context_menu.is_some());

        app.transcript_context_menu_area = (22, 5, 22, 4);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 23,
            row: 6,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.take_clipboard_request().as_deref(), Some("hello"));
        assert!(app.transcript_context_menu.is_none());
    }
}
