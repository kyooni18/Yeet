//! Keyboard navigation for the session sidebar (chat view, empty composer).
use super::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SidebarKey {
    Ignored,
    Handled,
    NewSession,
}

impl App {
    pub(super) fn handle_sidebar_key(&mut self, event: &KeyEvent) -> SidebarKey {
        if !(event.modifiers - KeyModifiers::SHIFT).is_empty() {
            return SidebarKey::Ignored;
        }
        if !self.sidebar_focus {
            let can_focus = self.input.is_empty()
                && !self.sidebar_nav_ids.is_empty()
                && event.code == KeyCode::Left;
            if can_focus {
                self.sidebar_focus = true;
                self.sidebar_cursor = self.current_sidebar_index();
                return SidebarKey::Handled;
            }
            return SidebarKey::Ignored;
        }
        if !self.input.is_empty() || self.sidebar_nav_ids.is_empty() {
            self.sidebar_focus = false;
            return SidebarKey::Ignored;
        }
        let last = self.sidebar_nav_ids.len() - 1;
        match event.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.sidebar_cursor = (self.sidebar_cursor + 1).min(last);
            }
            KeyCode::Home | KeyCode::Char('g') => self.sidebar_cursor = 0,
            KeyCode::End | KeyCode::Char('G') => self.sidebar_cursor = last,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(Some(id)) = self.sidebar_nav_ids.get(self.sidebar_cursor)
                    && self.state.current_session_id.as_deref() != Some(id.as_str())
                {
                    self.sidebar_load_request = Some(id.clone());
                    self.follow_tail = true;
                }
                self.sidebar_focus = false;
            }
            KeyCode::Char('n' | '+') => {
                self.sidebar_focus = false;
                return SidebarKey::NewSession;
            }
            KeyCode::Esc | KeyCode::Char('i') => self.sidebar_focus = false,
            _ => return SidebarKey::Ignored,
        }
        SidebarKey::Handled
    }

    fn current_sidebar_index(&self) -> usize {
        self.sidebar_nav_ids
            .iter()
            .position(|id| id.as_deref() == self.state.current_session_id.as_deref())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn empty_composer_focuses_sidebar_and_enter_requests_the_session() {
        let mut app = App::default();
        app.sidebar_nav_ids = vec![Some("a".into()), Some("b".into())];
        app.state.current_session_id = Some("a".into());

        app.input = "/mo".into();
        assert_eq!(app.handle_sidebar_key(&key(KeyCode::Left)), SidebarKey::Ignored);
        assert!(!app.sidebar_focus);

        app.input.clear();
        assert_eq!(app.handle_sidebar_key(&key(KeyCode::Left)), SidebarKey::Handled);
        assert!(app.sidebar_focus);
        app.handle_sidebar_key(&key(KeyCode::Char('j')));
        app.handle_sidebar_key(&key(KeyCode::Enter));
        assert!(!app.sidebar_focus);
        assert_eq!(app.take_sidebar_load_request().as_deref(), Some("b"));
    }
}
