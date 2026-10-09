//! Agent Group settings panel for session availability and member coordination.
use super::{App, Mode};
use crate::model::{AgentMode, FrontendCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentGroupRow {
    Enabled,
    AutoDeploy,
    Writers,
    OpenAgents,
}

pub const AGENT_GROUP_ROWS: [AgentGroupRow; 4] = [
    AgentGroupRow::Enabled,
    AgentGroupRow::AutoDeploy,
    AgentGroupRow::Writers,
    AgentGroupRow::OpenAgents,
];

impl App {
    pub(crate) fn agent_group_enabled(&self) -> bool {
        self.state.agent_mode == AgentMode::Adaptive
    }

    /// Opens the panel; `Esc` returns to the mode it was opened from.
    pub(crate) fn open_agent_group(&mut self) -> FrontendCommand {
        self.agents.agent_group_origin = Some(self.mode);
        self.mode = Mode::AgentGroup;
        self.popup_index = 0;
        self.input_focused = false;
        FrontendCommand::RequestSettings
    }

    fn close_agent_group(&mut self) {
        match self.agents.agent_group_origin.take() {
            Some(Mode::Agents) => self.open_agents(),
            Some(Mode::Settings) => {
                self.mode = Mode::Settings;
                let prepared = self.application.prepare_settings(
                    crate::shared_ui::settings::SettingsAction::Select(Some("agents".into())),
                    &self.state,
                );
                self.application.commit_settings(prepared, &self.state);
                self.sync_settings();
            }
            _ => self.close_popup(),
        }
    }

    pub(crate) fn handle_agent_group_key(&mut self, event: KeyEvent) -> Vec<FrontendCommand> {
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            return Vec::new();
        }
        let last = AGENT_GROUP_ROWS.len() - 1;
        match event.code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_agent_group(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.popup_index = self.popup_index.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.popup_index = (self.popup_index + 1).min(last)
            }
            KeyCode::Left | KeyCode::Char('h' | '-') => return self.adjust_agent_group(),
            KeyCode::Right | KeyCode::Char('l' | '+' | '=') => {
                return self.adjust_agent_group();
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if AGENT_GROUP_ROWS[self.popup_index.min(last)] == AgentGroupRow::OpenAgents {
                    self.agents.agent_group_origin = None;
                    self.open_agents();
                    return Vec::new();
                }
                return self.adjust_agent_group();
            }
            _ => {}
        }
        Vec::new()
    }

    /// Changes the selected row. Settings are applied locally right away so
    /// repeated presses build on each other before the backend echoes them.
    fn adjust_agent_group(&mut self) -> Vec<FrontendCommand> {
        let row = AGENT_GROUP_ROWS[self.popup_index.min(AGENT_GROUP_ROWS.len() - 1)];
        let mut settings = self.state.runtime_settings.agent_group.clone();
        let mut commands = Vec::new();
        match row {
            AgentGroupRow::Enabled => {
                if self.state.is_streaming {
                    self.state.settings_notice = Some(
                        "Group Agent tools can be switched after the current response.".into(),
                    );
                } else {
                    let mode = if self.agent_group_enabled() {
                        AgentMode::Single
                    } else {
                        AgentMode::Adaptive
                    };
                    commands.push(FrontendCommand::SetAgentMode { mode });
                }
                return commands;
            }
            AgentGroupRow::AutoDeploy => {
                settings.auto_deploy = !settings.auto_deploy;
                // Automatic delegation requires Group Agent tools in this session.
                if settings.auto_deploy && !self.agent_group_enabled() && !self.state.is_streaming {
                    commands.push(FrontendCommand::SetAgentMode {
                        mode: AgentMode::Adaptive,
                    });
                }
            }
            AgentGroupRow::Writers => {
                settings.write_policy = if settings.write_policy == "primary_only" {
                    "single_writer".into()
                } else {
                    "primary_only".into()
                };
            }
            AgentGroupRow::OpenAgents => return commands,
        }
        let settings = settings.normalized();
        if settings == self.state.runtime_settings.agent_group {
            return commands;
        }
        self.state.runtime_settings.agent_group = settings.clone();
        commands.insert(0, FrontendCommand::SetAgentGroupSettings { settings });
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn agent_group_panel_edits_delegation_and_writer_settings() {
        let mut app = App::default();
        app.open_agents();
        assert!(matches!(
            app.handle_agents_key(key(KeyCode::Char('w'))),
            Some(FrontendCommand::RequestSettings)
        ));
        assert_eq!(app.mode, Mode::AgentGroup);

        // Automatic delegation also enables Group Agent tools for this session.
        app.popup_index = 1;
        let sent = app.handle_agent_group_key(key(KeyCode::Enter));
        assert!(matches!(
            sent.as_slice(),
            [
                FrontendCommand::SetAgentGroupSettings { settings },
                FrontendCommand::SetAgentMode { mode: AgentMode::Adaptive },
            ] if settings.auto_deploy
        ));

        app.popup_index = 2;
        app.handle_agent_group_key(key(KeyCode::Right));
        assert_eq!(
            app.state.runtime_settings.agent_group.write_policy,
            "primary_only"
        );

        app.handle_agent_group_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Agents);
    }
}
