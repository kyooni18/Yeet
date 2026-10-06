//! Agent view (`Mode::Agents`): the active Agent Group's rail selection,
//! steering composer, and the add, stop, and remove actions.
use super::{App, Mode, WorkbenchTab};
use crate::model::{AgentMemberItem, FrontendCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

pub use crate::shared_ui::agents::AgentAction;

/// Terminal geometry, scrolling and native modal return handling. Application
/// state has one owner in UI; an in-flight delivery candidate is only a host
/// transaction and becomes live state after Harness delivery succeeds.
#[derive(Default)]
pub struct AgentsState {
    pub scroll: usize,
    pub(crate) targets: Vec<(Rect, AgentAction)>,
    pub agent_group_origin: Option<Mode>,
    pending_delivery: Option<crate::shared_ui::agents_session::PreparedAgentAction>,
}
impl App {
    pub(crate) fn agent_members(&self) -> &[AgentMemberItem] {
        &self.state.agent_group.members
    }

    /// The selected member is resolved by the same UI state as graphical views.
    pub(crate) fn selected_agent(&self) -> Option<&AgentMemberItem> {
        self.application
            .agent_state()
            .selected_member(&self.state.agent_group)
    }

    pub(crate) fn open_agents(&mut self) {
        self.apply_agent_action(AgentAction::Open);
        self.report_navigation(WorkbenchTab::Agents);
    }

    pub(crate) fn close_agents(&mut self) {
        self.apply_agent_action(AgentAction::Close);
        self.activate_workbench_tab(WorkbenchTab::Home);
    }

    fn select_agent_row(&mut self, delta: isize) {
        self.application
            .compatibility_agent_state_mut()
            .move_selection(&self.state.agent_group, delta);
        self.application.refresh(&self.state);
    }

    fn stop_agent_command(&mut self) -> Option<FrontendCommand> {
        self.apply_agent_action(AgentAction::Stop)
    }

    fn remove_agent_command(&mut self) -> Option<FrontendCommand> {
        self.apply_agent_action(AgentAction::Remove)
    }

    fn start_creating_group(&mut self) {
        self.apply_agent_action(AgentAction::CreateGroup);
    }

    fn submit_agent_draft(&mut self) -> Option<FrontendCommand> {
        self.apply_agent_action(AgentAction::SubmitDraft(self.input.clone()))
    }

    /// Native input and pointer targets deliver shared controller actions.
    pub(crate) fn apply_agent_action(&mut self, action: AgentAction) -> Option<FrontendCommand> {
        self.application
            .compatibility_agent_state_mut()
            .input_focused = self.input_focused;
        let prepared = self.application.prepare_agent(action, &self.state);
        if let Some(command) = prepared.effect.command.clone() {
            self.agents.pending_delivery = Some(prepared);
            return Some(command);
        }
        let (_, effect) = self.application.commit_agent(prepared, &self.state);
        self.apply_agent_effect(effect)
    }

    fn apply_agent_effect(
        &mut self,
        effect: crate::shared_ui::agents::AgentEffect,
    ) -> Option<FrontendCommand> {
        self.input_focused = self.application.agent_state().input_focused;
        if let Some(text) = effect.submitted_text {
            self.record_input_history(&text);
            self.input.clear();
            self.cursor = 0;
        }
        if effect.open_group_settings {
            let request = self.open_agent_group();
            return effect.command.or(Some(request));
        }
        effect.command
    }

    /// The host commits the prepared UI transaction only after Harness delivery.
    pub(crate) fn complete_agent_delivery(&mut self) {
        if let Some(prepared) = self.agents.pending_delivery.take() {
            let (_, effect) = self.application.commit_agent(prepared, &self.state);
            self.apply_agent_effect(effect);
        }
    }

    pub(crate) fn send_ui_command(
        &mut self,
        backend: &mut crate::backend::Backend,
        command: FrontendCommand,
    ) -> anyhow::Result<()> {
        self.finish_agent_delivery(backend.send(command))
    }

    fn finish_agent_delivery(&mut self, delivered: anyhow::Result<()>) -> anyhow::Result<()> {
        if delivered.is_ok() {
            self.complete_agent_delivery();
        } else {
            self.agents.pending_delivery = None;
        }
        delivered
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
                KeyCode::Esc if self.application.agent_state().creating_group => {
                    self.apply_agent_action(AgentAction::CancelDraft);
                }
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
            KeyCode::Char('i' | 's') | KeyCode::Enter | KeyCode::Tab => {
                self.apply_agent_action(AgentAction::Steer);
            }
            KeyCode::Char('x') => return self.stop_agent_command(),
            KeyCode::Char('n' | 'a' | '+') => self.start_creating_group(),
            KeyCode::Char('r') => {
                return self.apply_agent_action(AgentAction::RunGroup);
            }
            KeyCode::Char('d') | KeyCode::Delete => return self.remove_agent_command(),
            KeyCode::Char('w') => return self.apply_agent_action(AgentAction::AgentGroup),
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
    fn group_objectives_use_group_lifecycle_and_member_controls_remain_scoped() {
        let mut app = App::default();
        app.state.agent_group = AgentGroupItem {
            group_id: "group-1".into(),
            status: "running".into(),
            objective: Some("Original objective".into()),
            members: vec![member("planner", "running"), member("docs", "stopped")],
            ..Default::default()
        };
        app.open_agents();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        app.handle_agents_key(key(KeyCode::Char('j')));
        assert_eq!(
            app.application.agent_state().selected.as_deref(),
            Some("planner")
        );
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
        assert_eq!(app.input, "floor 3 km", "draft remains until delivery");
        app.complete_agent_delivery();
        assert!(app.input.is_empty());

        app.handle_agents_key(key(KeyCode::Esc));
        app.handle_agents_key(key(KeyCode::Char('j')));
        assert_eq!(
            app.application.agent_state().selected.as_deref(),
            Some("docs")
        );
        assert!(app.handle_agents_key(key(KeyCode::Char('x'))).is_none());

        assert!(matches!(
            app.handle_agents_key(key(KeyCode::Char('d'))),
            Some(FrontendCommand::RemoveAgent { agent_id: Some(ref id) }) if id == "docs"
        ));

        app.complete_agent_delivery();
        app.handle_agents_key(key(KeyCode::Char('g')));
        assert!(matches!(
            app.handle_agents_key(key(KeyCode::Char('x'))),
            Some(FrontendCommand::CancelAgentGroup { .. })
        ));

        app.state.agent_group.status = "created".into();
        app.state.agent_group.objective = Some("Original objective".into());
        assert!(matches!(
            app.apply_agent_action(AgentAction::RunGroup),
            Some(FrontendCommand::StartAgentGroup { .. })
        ));
        app.state.agent_group.status = "running".into();
        assert!(matches!(
            app.apply_agent_action(AgentAction::StopGroup),
            Some(FrontendCommand::StopAgentGroup { .. })
        ));
        app.state.agent_group.status = "paused".into();
        app.apply_agent_action(AgentAction::CreateGroup);
        assert!(!app.application.agent_state().creating_group);

        // Creating an objective uses the Group Agent lifecycle command; it
        // cannot accidentally replace the group with a standalone member run.
        app.state.agent_group.status = "completed".into();
        app.handle_agents_key(key(KeyCode::Char('a')));
        for character in "Compare the candidate models".chars() {
            app.handle_agents_key(key(KeyCode::Char(character)));
        }
        assert!(matches!(
            app.handle_agents_key(key(KeyCode::Enter)),
            Some(FrontendCommand::CreateAgentGroup { ref objective })
                if objective == "Compare the candidate models"
        ));
        assert!(app.application.agent_state().creating_group);
        assert!(
            app.finish_agent_delivery(Err(anyhow::anyhow!("unavailable")))
                .is_err()
        );
        assert!(app.application.agent_state().creating_group);
        assert_eq!(app.input, "Compare the candidate models");
        assert!(matches!(
            app.handle_agents_key(key(KeyCode::Enter)),
            Some(FrontendCommand::CreateAgentGroup { .. })
        ));
        app.finish_agent_delivery(Ok(())).unwrap();
        assert!(!app.application.agent_state().creating_group);
    }
}
