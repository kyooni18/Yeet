//! Navigation shared by the workbench launcher, tabs, keyboard and mouse.
use super::{App, Mode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::kit::SurfaceId;

/// Terminal actions use instance identities, never mutable vector positions.
/// The shared/wire navigation API remains index-based at the adapter boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkbenchTab {
    Home,
    Session,
    Files,
    File(SurfaceId),
    CloseFile(SurfaceId),
    Diff(SurfaceId),
    CloseDiff(SurfaceId),
    NewDiff,
    Launcher,
}

impl App {
    pub fn home_visible(&self) -> bool {
        // Visibility is workbench navigation state, not conversation state.
        self.home_override.unwrap_or(true)
    }

    pub fn active_workbench_tab(&self) -> WorkbenchTab {
        if self.mode == Mode::Diff || (self.mode == Mode::Views && self.views_origin == Mode::Diff)
        {
            return self
                .diff_tabs
                .active_id()
                .map_or(WorkbenchTab::Home, WorkbenchTab::Diff);
        }
        if self.mode == Mode::Files
            || (self.mode == Mode::Views && self.views_origin == Mode::Files)
        {
            self.files
                .as_ref()
                .and_then(|files| files.active_tab())
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
            tabs.extend(
                files
                    .tabs
                    .views()
                    .iter()
                    .map(|view| WorkbenchTab::File(view.id)),
            );
        }
        tabs.extend(
            self.diff_tabs
                .views()
                .iter()
                .map(|view| WorkbenchTab::Diff(view.id)),
        );
        tabs
    }

    pub fn activate_workbench_tab(&mut self, tab: WorkbenchTab) {
        self.apply_navigation(tab);
    }

    pub(crate) fn apply_shared_navigation(
        &mut self,
        action: crate::agents::actions::NavigationAction,
    ) {
        use crate::agents::actions::NavigationAction as Shared;
        let target = match action {
            Shared::Home => Some(WorkbenchTab::Home),
            Shared::Session => Some(WorkbenchTab::Session),
            Shared::Files => Some(WorkbenchTab::Files),
            Shared::NewDiff => Some(WorkbenchTab::NewDiff),
            Shared::Launcher => Some(WorkbenchTab::Launcher),
            Shared::File(index) | Shared::CloseFile(index) => self
                .files
                .as_ref()
                .and_then(|files| files.tabs.views().get(index))
                .map(|view| {
                    if matches!(action, Shared::CloseFile(_)) {
                        WorkbenchTab::CloseFile(view.id)
                    } else {
                        WorkbenchTab::File(view.id)
                    }
                }),
            Shared::Diff(index) | Shared::CloseDiff(index) => {
                self.diff_tabs.views().get(index).map(|view| {
                    if matches!(action, Shared::CloseDiff(_)) {
                        WorkbenchTab::CloseDiff(view.id)
                    } else {
                        WorkbenchTab::Diff(view.id)
                    }
                })
            }
        };
        if let Some(target) = target {
            self.apply_navigation(target);
        }
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
                    files.activate_browser();
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
            WorkbenchTab::Diff(id) => {
                if self.diff_tabs.activate(id) {
                    self.mode = Mode::Diff;
                }
            }
            WorkbenchTab::CloseDiff(id) => {
                if self.diff_tabs.close(id).is_some()
                    && self.diff_tabs.is_empty()
                    && self.mode == Mode::Diff
                {
                    self.mode = Mode::Chat;
                    self.home_override = Some(true);
                }
            }
            WorkbenchTab::Launcher => self.open_views(),
        }
        // Old hit targets belong to the previous frame.
        self.diff_frame.begin(ratatui::layout::Rect::default());
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
    fn stable_diff_actions_and_frame_geometry_do_not_follow_shifted_indices() {
        use super::super::diff::{DiffAction, DiffState};
        use ratatui::layout::Rect;
        let mut app = App::default();
        let first = app.diff_tabs.open(
            "same",
            DiffState {
                scroll: 7,
                ..Default::default()
            },
        );
        let second = app.diff_tabs.open(
            "same",
            DiffState {
                scroll: 12,
                ..Default::default()
            },
        );
        app.activate_workbench_tab(WorkbenchTab::Diff(first));
        assert_eq!(app.diff_tabs.active().unwrap().state.scroll, 7);
        app.activate_workbench_tab(WorkbenchTab::Diff(second));
        assert_eq!(app.diff_tabs.active().unwrap().state.scroll, 12);
        app.diff_frame.begin(Rect::new(0, 0, 80, 24));
        app.diff_frame.view = Some(second);
        app.diff_frame
            .hits
            .register(Rect::new(0, 0, 10, 1), DiffAction::Display(true));
        app.activate_workbench_tab(WorkbenchTab::CloseDiff(first));
        app.activate_workbench_tab(WorkbenchTab::CloseDiff(first));
        assert_eq!(app.diff_tabs.len(), 1);
        assert_eq!(app.diff_tabs.active_id(), Some(second));
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(
            !app.diff_tabs.active().unwrap().state.full,
            "old frame must not receive clicks"
        );
        app.activate_workbench_tab(WorkbenchTab::Diff(first));
        assert_eq!(app.diff_tabs.active_id(), Some(second));
        // Wire compatibility translates positions to IDs only at dispatch time.
        app.apply_shared_navigation(crate::agents::actions::NavigationAction::Diff(0));
        assert_eq!(app.active_workbench_tab(), WorkbenchTab::Diff(second));
        let mut terminal = Terminal::new(TestBackend::new(4, 3)).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &mut app))
            .unwrap();
        assert_eq!(app.diff_frame.view, None);
        assert_eq!(app.diff_frame.hits.targets().count(), 0);
    }

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
        app.files.as_mut().unwrap().open_path(first.clone(), false);
        let first_id = app.files.as_ref().unwrap().active_tab().unwrap();
        app.files.as_mut().unwrap().open_path(second.clone(), false);
        let second_id = app.files.as_ref().unwrap().active_tab().unwrap();
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
        app.activate_workbench_tab(WorkbenchTab::File(second_id));
        assert_eq!(app.files.as_ref().unwrap().selected_path(), Some(second));
        assert!(app.handle_workbench_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL)));
        assert!(app.home_visible());
        app.activate_workbench_tab(WorkbenchTab::File(second_id));
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &mut app))
            .unwrap();
        assert!(
            app.tab_targets
                .iter()
                .any(|(_, tab)| *tab == WorkbenchTab::File(second_id)),
            "active tab must survive overflow"
        );
        assert!(
            app.handle_workbench_key(&KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL))
        );
        assert_eq!(app.files.as_ref().unwrap().tabs.views()[0].id, first_id);
        assert_eq!(app.files.as_ref().unwrap().selected_path(), Some(first));
    }
}

#[cfg(test)]
mod independence_tests {
    use super::*;

    #[test]
    fn session_updates_do_not_choose_the_workbench_view() {
        let mut app = App::default();
        app.input = "draft".into();
        let mut next = app.state.clone();
        next.current_session_id = Some("background-session".into());
        next.is_streaming = true;
        next.error_message = Some("Session failed".into());
        app.merge_state(next);
        assert_eq!(app.active_workbench_tab(), WorkbenchTab::Home);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 20)).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &mut app))
            .unwrap();
        assert_eq!(app.transcript_area, (0, 0, 0, 0));

        app.activate_workbench_tab(WorkbenchTab::Files);
        let directory = app.files.as_ref().unwrap().dir.clone();
        let mut next = app.state.clone();
        next.current_session_id = None;
        next.is_streaming = false;
        app.merge_state(next);
        assert_eq!(app.active_workbench_tab(), WorkbenchTab::Files);
        assert_eq!(app.files.as_ref().unwrap().dir, directory);
        app.handle_files_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(app.active_workbench_tab(), WorkbenchTab::Home);
        assert_eq!(app.input, "draft");
        for mode in [
            Mode::Settings,
            Mode::Providers,
            Mode::Capabilities,
            Mode::Sessions,
        ] {
            app.mode = mode;
            terminal
                .draw(|frame| crate::tui::ui::draw(frame, &mut app))
                .unwrap();
            assert_eq!(app.transcript_area, (0, 0, 0, 0));
            assert!(app.sidebar_session_targets.is_empty());
        }
    }
}
