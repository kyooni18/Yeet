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

    fn navigation_key(&mut self, event: KeyEvent) -> KeyCode {
        if self.stage != Stage::Actions
            || !(event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT)
        {
            return event.code;
        }
        match event.code {
            KeyCode::Char('j') => KeyCode::Down,
            KeyCode::Char('k') => KeyCode::Up,
            KeyCode::Char('h') => KeyCode::Left,
            KeyCode::Char('l') => KeyCode::Right,
            KeyCode::Char('q') => KeyCode::Esc,
            KeyCode::Char(c @ ('r' | 'm' | 'd')) => {
                self.selected = match c {
                    'r' => 0,
                    'm' => 1,
                    _ => 2,
                };
                KeyCode::Enter
            }
            code => code,
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
    /// Search is an explicit insert mode; normal-mode letters never edit the query.
    pub(super) fn session_navigation_key(&mut self, event: KeyEvent) -> Option<KeyEvent> {
        if self.session_searching {
            match event.code {
                KeyCode::Esc | KeyCode::Enter => self.session_searching = false,
                KeyCode::Backspace => {
                    self.popup_filter.pop();
                    self.popup_index = 0;
                }
                KeyCode::Char('u') if event.modifiers == KeyModifiers::CONTROL => {
                    self.popup_filter.clear();
                    self.popup_index = 0;
                }
                KeyCode::Char(c)
                    if event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT =>
                {
                    self.popup_filter.push(c);
                    self.popup_index = 0;
                }
                _ => {}
            }
            return None;
        }
        let plain = event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT;
        let code = match event.code {
            KeyCode::Char('/') if plain => {
                self.session_searching = true;
                return None;
            }
            KeyCode::Char('j') if plain => KeyCode::Down,
            KeyCode::Char('k') if plain => KeyCode::Up,
            KeyCode::Char('g') if plain => KeyCode::Home,
            KeyCode::Char('G') if plain => KeyCode::End,
            KeyCode::Char('h') if plain => KeyCode::Left,
            KeyCode::Char('l') if plain => KeyCode::Right,
            KeyCode::Char('q') if plain => KeyCode::Esc,
            KeyCode::Char('e') if plain => {
                self.open_session_manager();
                return None;
            }
            KeyCode::Char('n') if plain => {
                return Some(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
            }
            KeyCode::Char('u') if event.modifiers == KeyModifiers::CONTROL => KeyCode::PageUp,
            KeyCode::Char('d') if event.modifiers == KeyModifiers::CONTROL => KeyCode::PageDown,
            KeyCode::Char('e' | 'n') if event.modifiers == KeyModifiers::CONTROL => {
                return Some(event);
            }
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::F(_) => return None,
            code => code,
        };
        Some(KeyEvent::new(code, KeyModifiers::NONE))
    }

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
        let code = manager.navigation_key(event);
        match code {
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
    fn session_normal_keys_and_search_text_are_separate() {
        let mut app = App::default();
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(
            app.session_navigation_key(key('j')).unwrap().code,
            KeyCode::Down
        );
        assert_eq!(
            app.session_navigation_key(key('k')).unwrap().code,
            KeyCode::Up
        );
        for (letter, arrow) in [
            ('h', KeyCode::Left),
            ('j', KeyCode::Down),
            ('k', KeyCode::Up),
            ('l', KeyCode::Right),
        ] {
            assert_eq!(app.session_navigation_key(key(letter)).unwrap().code, arrow);
            assert_eq!(
                app.session_navigation_key(KeyEvent::new(arrow, KeyModifiers::NONE))
                    .unwrap()
                    .code,
                arrow
            );
        }
        assert_eq!(
            app.session_navigation_key(key('G')).unwrap().code,
            KeyCode::End
        );
        assert!(
            app.session_navigation_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE))
                .is_none()
        );
        assert!(app.session_navigation_key(key('/')).is_none());
        assert!(app.session_searching);
        for c in "hjklqen/".chars() {
            assert!(app.session_navigation_key(key(c)).is_none());
        }
        assert_eq!(app.popup_filter, "hjklqen/");
        assert!(
            app.session_navigation_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .is_none()
        );
        assert!(!app.session_searching);
        assert_eq!(app.popup_filter, "hjklqen/");
        assert_eq!(
            app.session_navigation_key(key('q')).unwrap().code,
            KeyCode::Esc
        );
    }

    #[test]
    fn management_vim_keys_do_not_steal_editor_text() {
        let mut manager = SessionManager {
            id: "session".into(),
            title: "Title".into(),
            stage: Stage::Actions,
            selected: 0,
            text: String::new(),
            error: None,
        };
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        for (letter, arrow) in [
            ('h', KeyCode::Left),
            ('j', KeyCode::Down),
            ('k', KeyCode::Up),
            ('l', KeyCode::Right),
        ] {
            assert_eq!(manager.navigation_key(key(letter)), arrow);
            assert_eq!(
                manager.navigation_key(KeyEvent::new(arrow, KeyModifiers::NONE)),
                arrow
            );
            assert_eq!(manager.stage, Stage::Actions);
            assert_eq!(manager.selected, 0);
        }
        assert_eq!(manager.navigation_key(key('d')), KeyCode::Enter);
        assert_eq!(manager.selected, 2);
        for stage in [Stage::Rename, Stage::Move, Stage::Delete] {
            manager.stage = stage;
            for c in "jkhlqrmd".chars() {
                assert_eq!(manager.navigation_key(key(c)), KeyCode::Char(c));
            }
        }
    }
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
