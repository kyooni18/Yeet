//! Navigation shared by the workbench launcher, tabs, keyboard and mouse.
use super::{App, Mode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub use crate::actions::NavigationAction as WorkbenchTab;

impl App {
    pub fn home_visible(&self) -> bool {
        self.home_override.unwrap_or_else(|| {
            self.conversation.is_empty()
                && !self.state.is_streaming
                && self.state.current_session_id.is_none()
                && self.input.is_empty()
        })
    }

    pub fn active_workbench_tab(&self) -> WorkbenchTab {
        if self.mode == Mode::Diff || (self.mode == Mode::Views && self.views_origin == Mode::Diff)
        {
            return WorkbenchTab::Diff(self.active_diff);
        }
        if self.mode == Mode::Files
            || (self.mode == Mode::Views && self.views_origin == Mode::Files)
        {
            self.files
                .as_ref()
                .and_then(|files| files.active_tab)
                .map_or(WorkbenchTab::Files, WorkbenchTab::File)
        } else if self.home_visible() {
            WorkbenchTab::Home
        } else {
            WorkbenchTab::Session
        }
    }

    pub fn workbench_tabs(&self) -> Vec<WorkbenchTab> {
        let mut tabs = vec![WorkbenchTab::Home, WorkbenchTab::Session];
        if let Some(files) = &self.files {
            tabs.push(WorkbenchTab::Files);
            tabs.extend((0..files.tabs.len()).map(WorkbenchTab::File));
        }
        tabs.extend((0..self.diff_tabs.len()).map(WorkbenchTab::Diff));
        tabs
    }

    pub fn activate_workbench_tab(&mut self, tab: WorkbenchTab) {
        self.apply_navigation(tab);
    }

    pub(crate) fn apply_navigation(&mut self, tab: WorkbenchTab) {
        match tab {
            WorkbenchTab::Home | WorkbenchTab::Session => {
                self.home_override = Some(tab == WorkbenchTab::Home);
                self.mode = Mode::Chat;
                self.sidebar_focus = false;
                self.clear_transcript_selection();
            }
            WorkbenchTab::Files => {
                if self.files.is_none() {
                    self.open_files();
                }
                if let Some(files) = &mut self.files {
                    files.active_tab = None;
                }
                self.mode = Mode::Files;
            }
            WorkbenchTab::File(index) => {
                if let Some(files) = &mut self.files {
                    if files.activate_tab(index) {
                        self.mode = Mode::Files;
                    }
                }
            }
            WorkbenchTab::CloseFile(index) => {
                if let Some(files) = &mut self.files {
                    files.close_tab(index);
                }
            }
            WorkbenchTab::NewDiff => self.open_diff(None),
            WorkbenchTab::Diff(i) => {
                if i < self.diff_tabs.len() {
                    self.active_diff = i;
                    self.mode = Mode::Diff;
                }
            }
            WorkbenchTab::CloseDiff(i) => {
                if i < self.diff_tabs.len() {
                    self.diff_tabs.remove(i);
                    if self.active_diff > i {
                        self.active_diff -= 1;
                    }
                    self.active_diff = self.active_diff.min(self.diff_tabs.len().saturating_sub(1));
                    if self.diff_tabs.is_empty() && self.mode == Mode::Diff {
                        self.mode = Mode::Chat;
                        self.home_override = Some(true);
                    }
                }
            }
            WorkbenchTab::Launcher => self.open_views(),
        }
        // Old hit targets belong to the previous frame.
        self.tab_targets.clear();
        self.view_targets.clear();
    }

    pub(crate) fn handle_workbench_key(&mut self, event: &KeyEvent) -> bool {
        if !matches!(self.mode, Mode::Chat | Mode::Files | Mode::Diff) {
            return false;
        }
        if !event.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match event.code {
            KeyCode::Char('o') => self.open_views(),
            KeyCode::Tab | KeyCode::BackTab => {
                let tabs = self.workbench_tabs();
                let current = tabs
                    .iter()
                    .position(|tab| *tab == self.active_workbench_tab())
                    .unwrap_or(0);
                let back =
                    event.code == KeyCode::BackTab || event.modifiers.contains(KeyModifiers::SHIFT);
                let next = if back {
                    (current + tabs.len() - 1) % tabs.len()
                } else {
                    (current + 1) % tabs.len()
                };
                self.activate_workbench_tab(tabs[next]);
            }
            KeyCode::Char('w') => match self.active_workbench_tab() {
                WorkbenchTab::File(index) => {
                    self.activate_workbench_tab(WorkbenchTab::CloseFile(index))
                }
                WorkbenchTab::Diff(i) => self.activate_workbench_tab(WorkbenchTab::CloseDiff(i)),
                _ => self.activate_workbench_tab(WorkbenchTab::Home),
            },
            KeyCode::Char(c @ '1'..='9') => {
                if let Some(tab) = self
                    .workbench_tabs()
                    .get((c as u8 - b'1') as usize)
                    .copied()
                {
                    self.activate_workbench_tab(tab);
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn launcher_and_tabs_preserve_draft_and_activate_the_actual_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("alpha.rs");
        let second = dir.path().join("beta.rs");
        std::fs::write(&first, "alpha").unwrap();
        std::fs::write(&second, "beta").unwrap();
        let mut app = App::default();
        app.files = Some(super::super::files::FilesState::open(
            dir.path().to_path_buf(),
        ));
        app.files.as_mut().unwrap().tabs = vec![first.clone(), second.clone()];
        app.input = "Preserve my draft".into();
        app.cursor = 5;
        app.state.current_session_id = Some("keep-session".into());
        app.activate_workbench_tab(WorkbenchTab::Home);
        assert!(app.home_visible());
        let mut terminal = Terminal::new(TestBackend::new(110, 28)).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &mut app))
            .unwrap();
        let plus = app
            .tab_targets
            .iter()
            .find(|(_, tab)| *tab == WorkbenchTab::Launcher)
            .unwrap()
            .0;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: plus.x,
            row: plus.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.mode, Mode::Views);
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &mut app))
            .unwrap();
        let session = app
            .view_targets
            .iter()
            .find(|(_, tab)| *tab == WorkbenchTab::Session)
            .unwrap()
            .0;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: session.x,
            row: session.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!app.home_visible());
        assert_eq!(app.input, "Preserve my draft");
        assert_eq!(app.cursor, 5);
        assert_eq!(
            app.state.current_session_id.as_deref(),
            Some("keep-session")
        );
        app.activate_workbench_tab(WorkbenchTab::File(1));
        assert_eq!(app.files.as_ref().unwrap().selected_path(), Some(second));
        assert!(app.handle_workbench_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL)));
        assert!(app.home_visible());
        app.activate_workbench_tab(WorkbenchTab::File(1));
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &mut app))
            .unwrap();
        assert!(
            app.tab_targets
                .iter()
                .any(|(_, tab)| *tab == WorkbenchTab::File(1)),
            "active tab must survive overflow"
        );
        assert!(
            app.handle_workbench_key(&KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL))
        );
        assert_eq!(app.files.as_ref().unwrap().tabs, vec![first.clone()]);
        assert_eq!(app.files.as_ref().unwrap().selected_path(), Some(first));
    }
}
