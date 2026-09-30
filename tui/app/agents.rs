//! Agent view (`Mode::Agents`): the active Agent Group's rail selection,
//! steering composer, and stop action.
use super::{App, Mode, WorkbenchTab};
use crate::model::{AgentMemberItem, FrontendCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAction {
    /// A rail row: `None` is the whole group.
    Select(Option<String>),
    Steer,
    Stop,
}

#[derive(Debug, Default)]
pub struct AgentsState {
    /// The Agents tab stays listed once opened, even after the group empties.
    pub open: bool,
    /// Selected member id; `None` shows the whole group.
    pub selected: Option<String>,
    pub(crate) targets: Vec<(Rect, AgentAction)>,
}

impl App {
    pub(crate) fn agent_members(&self) -> &[AgentMemberItem] {
        &self.state.agent_group.members
    }

    /// The selected member, if it is still part of the group.
    pub(crate) fn selected_agent(&self) -> Option<&AgentMemberItem> {
        let id = self.agents.selected.as_deref()?;
        self.agent_members().iter().find(|member| member.id == id)
    }

    pub(crate) fn open_agents(&mut self) {
        self.agents.open = true;
        self.mode = Mode::Agents;
        self.input_focused = false;
    }

    pub(crate) fn close_agents(&mut self) {
        self.agents.open = false;
        self.agents.selected = None;
        self.activate_workbench_tab(WorkbenchTab::Home);
    }

    fn select_agent_row(&mut self, delta: isize) {
        let rows: Vec<Option<String>> = std::iter::once(None)
            .chain(
                self.agent_members()
                    .iter()
                    .map(|member| Some(member.id.clone())),
            )
            .collect();
        let current = rows
            .iter()
            .position(|row| *row == self.agents.selected)
            .unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, rows.len() as isize - 1) as usize;
        self.agents.selected = rows[next].clone();
    }

    /// Stops the selected member, or the whole group on the group row.
    fn stop_agent_command(&self) -> Option<FrontendCommand> {
        match self.selected_agent() {
            Some(member) if member.status != "stopped" => Some(FrontendCommand::StopAgent {
                agent_id: Some(member.id.clone()),
            }),
            Some(_) => None,
            None => self
                .agent_members()
                .iter()
                .any(|member| member.status != "stopped")
                .then_some(FrontendCommand::StopAgent { agent_id: None }),
        }
    }

    /// Sends the draft to the selected member, or to the primary agent from
    /// the group row. The draft stays put when it cannot be delivered.
    fn submit_agent_draft(&mut self) -> Option<FrontendCommand> {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return None;
        }
        let command = match self.selected_agent() {
            Some(member) if member.status == "stopped" => return None,
            Some(member) => FrontendCommand::MessageAgent {
                agent_id: member.id.clone(),
                message: text.clone(),
            },
            None if self.state.is_streaming => return None,
            None => FrontendCommand::Submit {
                text: text.clone(),
                images: Vec::new(),
                attachment_ids: Vec::new(),
            },
        };
        self.record_input_history(&text);
        self.input.clear();
        self.cursor = 0;
        Some(command)
    }

    pub(crate) fn apply_agent_action(&mut self, action: AgentAction) -> Option<FrontendCommand> {
        match action {
            AgentAction::Select(id) => {
                self.agents.selected = id;
                self.input_focused = false;
                None
            }
            AgentAction::Steer => {
                self.input_focused = true;
                None
            }
            AgentAction::Stop => self.stop_agent_command(),
        }
    }

    pub(crate) fn handle_agents_key(&mut self, event: KeyEvent) -> Option<FrontendCommand> {
        if event.modifiers == KeyModifiers::CONTROL && event.code == KeyCode::Char('c') {
            self.quit = true;
            return None;
        }
        if self.input_focused {
            if self.handle_chat_editing_key(&event) {
                return None;
            }
            match event.code {
                KeyCode::Esc => self.input_focused = false,
                KeyCode::Enter if event.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.insert_char('\n')
                }
                KeyCode::Enter => return self.submit_agent_draft(),
                KeyCode::Backspace => self.backspace(),
                KeyCode::Delete => self.delete(),
                KeyCode::Left => {
                    self.cursor =
                        crate::text_layout::previous_grapheme_cursor(&self.input, self.cursor)
                }
                KeyCode::Right => {
                    self.cursor = crate::text_layout::next_grapheme_cursor(&self.input, self.cursor)
                }
                KeyCode::Home => self.cursor = 0,
                KeyCode::End => self.cursor = self.input.chars().count(),
                KeyCode::Char(character) if !event.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.insert_char(character)
                }
                _ => {}
            }
            return None;
        }
        if !event.modifiers.is_empty() {
            return None;
        }
        match event.code {
            KeyCode::Down | KeyCode::Char('j') => self.select_agent_row(1),
            KeyCode::Up | KeyCode::Char('k') => self.select_agent_row(-1),
            KeyCode::Home | KeyCode::Char('g') => self.select_agent_row(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.select_agent_row(isize::MAX / 2),
            KeyCode::Char('i' | 's') | KeyCode::Enter | KeyCode::Tab => self.input_focused = true,
            KeyCode::Char('x') => return self.stop_agent_command(),
            KeyCode::Char('q') => self.close_agents(),
            _ => {}
        }
        None
    }

    pub(crate) fn agent_action_at(&self, column: u16, row: u16) -> Option<AgentAction> {
        self.agents
            .targets
            .iter()
            .find(|(area, _)| area.contains((column, row).into()))
            .map(|(_, action)| action.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentGroupItem;

    fn member(id: &str, status: &str) -> AgentMemberItem {
        AgentMemberItem {
            id: id.into(),
            description: id.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    #[test]
    fn steering_targets_the_selected_member_and_stop_skips_stopped_members() {
        let mut app = App::default();
        app.state.agent_group = AgentGroupItem {
            members: vec![member("planner", "running"), member("docs", "stopped")],
            ..Default::default()
        };
        app.open_agents();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        app.handle_agents_key(key(KeyCode::Char('j')));
        assert_eq!(app.agents.selected.as_deref(), Some("planner"));
        app.handle_agents_key(key(KeyCode::Char('i')));
        for character in "floor 3 km".chars() {
            app.handle_agents_key(key(KeyCode::Char(character)));
        }
        let sent = app.handle_agents_key(key(KeyCode::Enter));
        assert!(matches!(
            sent,
            Some(FrontendCommand::MessageAgent { ref agent_id, ref message })
                if agent_id == "planner" && message == "floor 3 km"
        ));
        assert!(app.input.is_empty());

        app.handle_agents_key(key(KeyCode::Esc));
        app.handle_agents_key(key(KeyCode::Char('j')));
        assert_eq!(app.agents.selected.as_deref(), Some("docs"));
        assert!(app.handle_agents_key(key(KeyCode::Char('x'))).is_none());

        app.handle_agents_key(key(KeyCode::Char('g')));
        assert!(matches!(
            app.handle_agents_key(key(KeyCode::Char('x'))),
            Some(FrontendCommand::StopAgent { agent_id: None })
        ));
    }
}
