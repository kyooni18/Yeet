//! Native session management input. Persistence stays in Harness commands.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Actions,
    Rename,
    Move,
    Delete,
}

pub(crate) struct SessionManager {
    pub id: String,
    pub title: String,
    pub stage: Stage,
    pub selected: usize,
    pub text: String,
    pub error: Option<String>,
}

impl SessionManager {
    pub fn paste(&mut self, text: &str) {
        if self.stage != Stage::Actions {
            self.text.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    fn command(&mut self) -> Option<FrontendCommand> {
        let text = self.text.trim().to_owned();
        let session_id = self.id.clone();
        match self.stage {
            Stage::Rename if !text.is_empty() => Some(FrontendCommand::RenameSession {
                session_id,
                title: text,
            }),
            Stage::Move if !text.is_empty() => Some(FrontendCommand::MoveSession {
                session_id,
                workspace_path: text,
            }),
            Stage::Delete if text == "delete" => {
                Some(FrontendCommand::DeleteSession { session_id })
            }
            _ => {
                self.error = Some(
                    if self.stage == Stage::Delete {
                        "Type delete to confirm, or Esc to cancel."
                    } else {
                        "Enter a non-empty value."
                    }
                    .into(),
                );
                None
            }
        }
    }
}

impl App {
    pub(crate) fn open_session_manager(&mut self) {
        let Some(item) = self
            .filtered_session_picker_items()
            .get(self.popup_index)
            .cloned()
        else {
            return;
        };
        self.session_manager = Some(SessionManager {
            id: item.session.id.clone(),
            title: item.session.display_title(),
            stage: Stage::Actions,
            selected: 0,
            text: String::new(),
            error: None,
        });
    }

    pub(super) fn handle_session_management_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        let Some(manager) = &mut self.session_manager else {
            return Ok(());
        };
        manager.error = None;
        match event.code {
            KeyCode::Esc => {
                if manager.stage == Stage::Actions {
                    self.session_manager = None;
                } else {
                    manager.stage = Stage::Actions;
                    manager.text.clear();
                }
            }
            KeyCode::Up if manager.stage == Stage::Actions => {
                manager.selected = manager.selected.saturating_sub(1)
            }
            KeyCode::Down if manager.stage == Stage::Actions => {
                manager.selected = (manager.selected + 1).min(2)
            }
            KeyCode::Enter if manager.stage == Stage::Actions => {
                let protected = self.state.current_session_id.as_deref() == Some(&manager.id);
                if manager.selected != 0 && protected {
                    manager.error = Some(
                        "Switch to a new session before moving or deleting this session.".into(),
                    );
                } else {
                    manager.stage = [Stage::Rename, Stage::Move, Stage::Delete][manager.selected];
                    manager.text = if manager.stage == Stage::Rename {
                        manager.title.clone()
                    } else {
                        String::new()
                    };
                }
            }
            KeyCode::Enter => {
                if let Some(command) = manager.command() {
                    backend.send(command)?;
                    self.session_manager = None;
                    self.popup_index = 0;
                }
            }
            KeyCode::Tab if manager.stage == Stage::Move => {
                let paths = &self.state.known_workspaces;
                if !paths.is_empty() {
                    let next = paths
                        .iter()
                        .position(|w| w.path == manager.text)
                        .map_or(0, |i| (i + 1) % paths.len());
                    manager.text = paths[next].path.clone();
                }
            }
            KeyCode::Backspace => {
                manager.text.pop();
            }
            KeyCode::Char('u') if event.modifiers == KeyModifiers::CONTROL => manager.text.clear(),
            KeyCode::Char(c)
                if event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT =>
            {
                manager.paste(&c.to_string())
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_and_commands_are_bound_to_the_original_id() {
        let mut manager = SessionManager {
            id: "original".into(),
            title: "Title".into(),
            stage: Stage::Delete,
            selected: 2,
            text: String::new(),
            error: None,
        };
        assert!(manager.command().is_none());
        manager.paste("delete\r\n");
        assert!(
            matches!(manager.command(), Some(FrontendCommand::DeleteSession { session_id }) if session_id == "original")
        );
        manager.stage = Stage::Rename;
        manager.text = "  ".into();
        assert!(manager.command().is_none());
        manager.text = " New name ".into();
        assert!(
            matches!(manager.command(), Some(FrontendCommand::RenameSession { title, .. }) if title == "New name")
        );
    }
}
