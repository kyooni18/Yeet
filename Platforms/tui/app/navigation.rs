//! Navigation shared by the workbench launcher, tabs, keyboard and mouse.
use super::{App, Mode};
use crate::model::FrontendCommand;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub use crate::shared_ui::workbench::WorkbenchTab;

impl App {
    pub(crate) fn activate_toolbar_control(
        &mut self,
        backend: &mut crate::backend::Backend,
        id: &str,
    ) -> anyhow::Result<bool> {
        self.send_toolbar_action(
            backend,
            crate::shared_ui::toolbar::ToolbarAction::Activate(id.into()),
        )
    }

    pub(crate) fn send_toolbar_action(
        &mut self,
        backend: &mut crate::backend::Backend,
        action: crate::shared_ui::toolbar::ToolbarAction,
    ) -> anyhow::Result<bool> {
        self.sync_composer();
        let prepared = self.application.prepare_toolbar(action, &self.state);
        let accepted = prepared.effect.command.is_some() || prepared.effect.destination.is_some();
        if let Some(command) = prepared.effect.command.clone() {
            backend.send(command)?;
        }
        if let Some(destination) = prepared.effect.destination {
            use crate::shared_ui::composer::ComposerDestination;
            match destination {
                ComposerDestination::Models => self.open_models(backend)?,
                ComposerDestination::Sessions => self.open_sessions(backend)?,
                ComposerDestination::Files => self.open_files(),
                ComposerDestination::Reasoning => self.open_reasoning(),
                ComposerDestination::Capabilities => self.open_capabilities(backend)?,
                ComposerDestination::Settings => self.open_settings(backend)?,
                _ => return Ok(false),
            }
        }
        self.application.commit_toolbar(prepared, &self.state);
        Ok(accepted)
    }

    pub(crate) fn toolbar_control(
        &self,
        id: &str,
    ) -> Option<crate::shared_ui::toolbar::ToolbarControl> {
        let view = self.application.toolbar_view_for(&self.state);
        view.groups
            .into_iter()
            .flat_map(|group| group.controls)
            .chain(view.sandbox_controls)
            .find(|control| control.id == id)
    }

    pub(crate) fn workbench_resources(&self) -> crate::shared_ui::navigation::WorkbenchResources {
        crate::shared_ui::navigation::WorkbenchResources {
            agents: self.application.agent_state().open
                || (!self.agents_dismissed && !self.agent_members().is_empty()),
            files: self
                .files
                .as_ref()
                .map(|files| files.tabs.views().iter().map(|view| view.id).collect()),
            active_file: self.files.as_ref().and_then(|files| files.active_tab()),
            diffs: self.diff_tabs.views().iter().map(|view| view.id).collect(),
            active_diff: self.diff_tabs.active_id(),
            sessions: self
                .session_tabs
                .views()
                .iter()
                .map(|view| view.id)
                .collect(),
            active_session: self.session_tabs.active_id(),
            order: self.tab_order.clone(),
            files_browser_closed: self.files_browser_closed,
            sessions_managed: true,
        }
    }

    /// The active conversation tab follows the runtime's current session,
    /// except while a tab switch is still waiting for its session to load.
    pub(crate) fn sync_session_tab(&mut self, previous: Option<String>) {
        let current = self.state.current_session_id.clone();
        if let Some(from) = &self.session_switch_from {
            if *from == current {
                return;
            }
            self.session_switch_from = None;
        }
        if self.session_tabs.is_empty() {
            // Closing the last tab leaves the implicit session tab until the
            // runtime moves to another session.
            if current.is_none() || previous == current {
                return;
            }
            self.session_tabs.open("", current);
            return;
        }
        if let Some(tab) = self.session_tabs.active_mut() {
            tab.state = current;
        }
    }

    fn ensure_session_tab(&mut self) {
        if self.session_tabs.is_empty() {
            let current = self.state.current_session_id.clone();
            self.session_tabs.open("", current);
        }
    }

    /// Activate a conversation tab, loading its session into the runtime.
    fn switch_session_tab(&mut self, id: crate::tui::kit::SurfaceId) -> bool {
        if !self.session_tabs.activate(id) {
            return false;
        }
        let target = self.session_tabs.active().and_then(|tab| tab.state.clone());
        let current = self.state.current_session_id.clone();
        if target != current {
            self.workbench_command = Some(match target {
                Some(session_id) => FrontendCommand::LoadSession { session_id },
                None => FrontendCommand::NewSession,
            });
            self.session_switch_from = Some(current);
            self.follow_tail = true;
        }
        true
    }

    /// Show `session_id`: focus a tab already showing it, otherwise load it
    /// into the active conversation tab.
    pub(crate) fn focus_session(&mut self, session_id: &str) -> bool {
        let existing = self
            .session_tabs
            .views()
            .iter()
            .find(|tab| tab.state.as_deref() == Some(session_id))
            .map(|tab| tab.id);
        match existing {
            Some(id) => {
                self.activate_workbench_tab(WorkbenchTab::Conversation(id));
                true
            }
            None => false,
        }
    }

    pub(crate) fn draggable_tab(tab: WorkbenchTab) -> bool {
        !matches!(
            tab,
            WorkbenchTab::Home
                | WorkbenchTab::Launcher
                | WorkbenchTab::NewConversation
                | WorkbenchTab::NewDiff
                | WorkbenchTab::CloseFile(_)
                | WorkbenchTab::CloseDiff(_)
                | WorkbenchTab::CloseConversation(_)
                | WorkbenchTab::CloseFiles
                | WorkbenchTab::CloseAgents
        )
    }

    fn move_tab(&mut self, tab: WorkbenchTab, delta: isize) {
        self.tab_order = self.workbench_resources().moved(tab, delta);
        self.tab_targets.clear();
    }

    pub(crate) fn drop_tab(&mut self, tab: WorkbenchTab, target: WorkbenchTab) {
        if tab != target {
            self.tab_order = self.workbench_resources().dropped(tab, target);
            self.tab_targets.clear();
        }
    }

    fn close_active_tab(&mut self) {
        match self.active_workbench_tab() {
            WorkbenchTab::File(id) => self.activate_workbench_tab(WorkbenchTab::CloseFile(id)),
            WorkbenchTab::Diff(id) => self.activate_workbench_tab(WorkbenchTab::CloseDiff(id)),
            WorkbenchTab::Conversation(id) => {
                self.activate_workbench_tab(WorkbenchTab::CloseConversation(id))
            }
            WorkbenchTab::Agents => self.activate_workbench_tab(WorkbenchTab::CloseAgents),
            WorkbenchTab::Files => self.activate_workbench_tab(WorkbenchTab::CloseFiles),
            _ => self.activate_workbench_tab(WorkbenchTab::Home),
        }
    }
    pub(crate) fn report_navigation(&mut self, tab: WorkbenchTab) {
        let resources = self.workbench_resources();
        self.application
            .accepted_navigation(tab, &resources, &self.state);
    }
    pub fn home_visible(&self) -> bool {
        self.application.navigation().home_visible()
    }
    pub fn active_workbench_tab(&self) -> WorkbenchTab {
        self.application
            .navigation()
            .active_tab(&self.workbench_resources())
    }
    pub fn workbench_tabs(&self) -> Vec<WorkbenchTab> {
        self.workbench_resources().tabs()
    }

    pub fn activate_workbench_tab(&mut self, tab: WorkbenchTab) {
        let owner = match tab {
            WorkbenchTab::CloseFile(id) => WorkbenchTab::File(id),
            WorkbenchTab::CloseDiff(id) => WorkbenchTab::Diff(id),
            WorkbenchTab::CloseConversation(id) => WorkbenchTab::Conversation(id),
            WorkbenchTab::CloseFiles => WorkbenchTab::Files,
            WorkbenchTab::CloseAgents => WorkbenchTab::Agents,
            _ => return self.apply_navigation(tab),
        };
        // Every tab closes like an IDE tab: the displayed one hands focus to
        // its right neighbor (or left at the end of the strip).
        let before = self.workbench_tabs();
        let displayed = self.active_workbench_tab() == owner;
        let index = before.iter().position(|t| *t == owner);
        self.apply_navigation(tab);
        self.tab_order.retain(|t| *t != owner);
        let after = self.workbench_tabs();
        if after.contains(&owner) {
            return;
        }
        if displayed {
            let next = index
                .and_then(|index| after.get(index.min(after.len().saturating_sub(1))))
                .copied()
                .unwrap_or(WorkbenchTab::Home);
            self.apply_navigation(next);
        } else if let WorkbenchTab::Conversation(_) = owner
            && let Some(next) = self.session_tabs.active_id()
        {
            // Keep the runtime on the session of the remaining active tab.
            self.switch_session_tab(next);
        }
    }

    pub(crate) fn apply_shared_navigation(
        &mut self,
        action: crate::shared_ui::actions::NavigationAction,
    ) {
        use crate::shared_ui::actions::NavigationAction as Shared;
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
            WorkbenchTab::Conversation(id) => {
                if self.switch_session_tab(id) {
                    self.report_navigation(tab);
                    self.sidebar_focus = false;
                    self.clear_transcript_selection();
                }
            }
            WorkbenchTab::NewConversation => {
                self.ensure_session_tab();
                let id = self.session_tabs.open("", None);
                self.switch_session_tab(id);
                self.report_navigation(WorkbenchTab::Conversation(id));
                self.sidebar_focus = false;
                self.input_focused = true;
                self.clear_transcript_selection();
            }
            WorkbenchTab::CloseConversation(id) => {
                if self.session_tabs.close(id).is_some() {
                    self.report_navigation(tab);
                }
            }
            WorkbenchTab::CloseFiles => {
                if self
                    .files
                    .as_ref()
                    .is_some_and(|files| files.tabs.is_empty())
                {
                    self.files = None;
                } else if let Some(files) = &mut self.files {
                    self.files_browser_closed = true;
                    if let Some(id) = files.tabs.active_id() {
                        files.activate_tab(id);
                    }
                }
                self.report_navigation(tab);
            }
            WorkbenchTab::CloseAgents => {
                self.agents_dismissed = true;
                self.apply_agent_action(crate::shared_ui::agents::AgentAction::Close);
            }
            WorkbenchTab::Session => {
                self.ensure_session_tab();
                let id = self.session_tabs.active_id().expect("session tab");
                return self.apply_navigation(WorkbenchTab::Conversation(id));
            }
            WorkbenchTab::Home => {
                self.report_navigation(tab);
                self.sidebar_focus = false;
                self.clear_transcript_selection();
            }
            WorkbenchTab::Files => {
                self.files_browser_closed = false;
                if self.files.is_none() {
                    self.open_files();
                }
                if let Some(files) = &mut self.files {
                    files.activate_browser();
                }
                self.report_navigation(tab);
            }
            WorkbenchTab::File(index) => {
                if let Some(files) = &mut self.files {
                    if files.activate_tab(index) {
                        self.report_navigation(tab);
                    }
                }
            }
            WorkbenchTab::CloseFile(index) => {
                if let Some(files) = &mut self.files {
                    files.close_tab(index);
                    if self.files_browser_closed && files.tabs.is_empty() {
                        self.files = None;
                        self.files_browser_closed = false;
                    }
                }
            }
            WorkbenchTab::NewDiff => self.open_diff(None),
            WorkbenchTab::Agents => {
                self.agents_dismissed = false;
                self.open_agents()
            }
            WorkbenchTab::Diff(id) => {
                if self.diff_tabs.activate(id) {
                    self.report_navigation(tab);
                }
            }
            WorkbenchTab::CloseDiff(id) => {
                if self.diff_tabs.close(id).is_some() {
                    self.application.remove_diff_view(id);
                    self.report_navigation(tab);
                }
            }
            WorkbenchTab::Launcher => self.open_views(),
        }
        // Old hit targets belong to the previous frame.
        self.diff_frame.begin(ratatui::layout::Rect::default());
        self.tab_targets.clear();
        self.view_targets.clear();
    }

    fn cycle_workbench_tab(&mut self, backwards: bool) {
        let target = self
            .application
            .navigation()
            .cycle_tab(&self.workbench_resources(), backwards);
        self.activate_workbench_tab(target);
    }

    pub(crate) fn handle_workbench_key(&mut self, event: &KeyEvent) -> bool {
        if !matches!(
            self.mode,
            Mode::Chat | Mode::Files | Mode::Diff | Mode::Agents
        ) {
            return false;
        }
        // Terminal-safe bindings: Alt+, / Alt+. (macOS Option glyphs ≤ ≥)
        // cycle tabs, Alt+< / Alt+> (˘ ¯) move them, Alt+1..9 jump, Ctrl+T
        // opens a session tab. None of these are reserved by Windows, Linux
        // desktops or macOS; Ctrl+Tab still works where the terminal sends it.
        let control = event.modifiers.contains(KeyModifiers::CONTROL);
        let alt = event.modifiers.contains(KeyModifiers::ALT);
        let active = self.active_workbench_tab();
        match event.code {
            KeyCode::Char('.' | '≥') if alt || event.code == KeyCode::Char('≥') => {
                self.cycle_workbench_tab(false)
            }
            KeyCode::Char(',' | '≤') if alt || event.code == KeyCode::Char('≤') => {
                self.cycle_workbench_tab(true)
            }
            KeyCode::Char('>' | '˘') if alt || event.code == KeyCode::Char('˘') => {
                self.move_tab(active, 1)
            }
            KeyCode::Char('<' | '¯') if alt || event.code == KeyCode::Char('¯') => {
                self.move_tab(active, -1)
            }
            KeyCode::Char(c @ '1'..='9') if alt || control => {
                let tabs = self.workbench_tabs();
                let tab = if c == '9' {
                    tabs.last().copied()
                } else {
                    tabs.get((c as u8 - b'1') as usize).copied()
                };
                if let Some(tab) = tab {
                    self.activate_workbench_tab(tab);
                }
            }
            KeyCode::Char('t') if control => {
                self.activate_workbench_tab(WorkbenchTab::NewConversation)
            }
            KeyCode::Char('o') if control => self.open_views(),
            KeyCode::Tab | KeyCode::BackTab if control => {
                let back =
                    event.code == KeyCode::BackTab || event.modifiers.contains(KeyModifiers::SHIFT);
                self.cycle_workbench_tab(back);
            }
            // Ctrl+W keeps deleting words while a draft is being typed.
            KeyCode::Char('w')
                if control
                    && !(self.mode == Mode::Chat
                        && self.input_focused
                        && !self.input.is_empty()) =>
            {
                self.close_active_tab()
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
    fn session_tabs_switch_sessions_and_reorder_with_terminal_safe_keys() {
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        let mut app = App::default();
        let mut next = app.state.clone();
        next.current_session_id = Some("a".into());
        app.merge_state(next);
        assert_eq!(app.session_tabs.len(), 1);
        assert!(app.handle_workbench_key(&key(KeyCode::Char('t'), KeyModifiers::CONTROL)));
        assert!(matches!(
            app.take_workbench_command(),
            Some(FrontendCommand::NewSession)
        ));
        let second = app.session_tabs.active_id().unwrap();
        // The runtime still reports the old session; the new tab must not adopt it.
        app.merge_state(app.state.clone());
        let mut next = app.state.clone();
        next.current_session_id = Some("b".into());
        app.merge_state(next);
        assert_eq!(
            app.session_tabs.get(second).unwrap().state.as_deref(),
            Some("b")
        );
        let first = app.session_tabs.views()[0].id;
        // Alt+, goes back to the first session tab and loads its session.
        assert!(app.handle_workbench_key(&key(KeyCode::Char(','), KeyModifiers::ALT)));
        assert_eq!(
            app.active_workbench_tab(),
            WorkbenchTab::Conversation(first)
        );
        assert!(matches!(app.take_workbench_command(),
            Some(FrontendCommand::LoadSession { session_id }) if session_id == "a"));
        // macOS Option+Shift+. glyph moves the tab right.
        assert!(app.handle_workbench_key(&key(KeyCode::Char('˘'), KeyModifiers::NONE)));
        assert_eq!(
            app.workbench_tabs()[..3],
            [
                WorkbenchTab::Home,
                WorkbenchTab::Conversation(second),
                WorkbenchTab::Conversation(first)
            ]
        );
        app.drop_tab(
            WorkbenchTab::Conversation(first),
            WorkbenchTab::Conversation(second),
        );
        assert_eq!(app.workbench_tabs()[1], WorkbenchTab::Conversation(first));
        // Ctrl+W edits a draft instead of closing; closing loads the neighbor.
        app.input_focused = true;
        app.input = "draft".into();
        assert!(!app.handle_workbench_key(&key(KeyCode::Char('w'), KeyModifiers::CONTROL)));
        app.input.clear();
        let mut next = app.state.clone();
        next.current_session_id = Some("a".into());
        app.merge_state(next);
        assert!(app.handle_workbench_key(&key(KeyCode::Char('w'), KeyModifiers::CONTROL)));
        assert_eq!(
            app.active_workbench_tab(),
            WorkbenchTab::Conversation(second)
        );
        assert!(matches!(app.take_workbench_command(),
            Some(FrontendCommand::LoadSession { session_id }) if session_id == "b"));
    }

    #[test]
    fn every_workbench_tab_closes_like_an_ide_tab() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "a").unwrap();
        let mut app = App::default();
        app.files = Some(super::super::files::FilesState::open(
            dir.path().to_path_buf(),
        ));
        app.files.as_mut().unwrap().open_path(file, false);
        let file_id = app.files.as_ref().unwrap().active_tab().unwrap();
        app.activate_workbench_tab(WorkbenchTab::Files);
        app.activate_workbench_tab(WorkbenchTab::CloseFiles);
        // The browser tab is gone; focus moves to its neighbor, the file tab.
        assert!(!app.workbench_tabs().contains(&WorkbenchTab::Files));
        assert_eq!(app.active_workbench_tab(), WorkbenchTab::File(file_id));
        app.activate_workbench_tab(WorkbenchTab::CloseFile(file_id));
        assert!(app.files.is_none());
        let session = app.session_tabs.active_id().unwrap();
        app.activate_workbench_tab(WorkbenchTab::Agents);
        assert!(app.workbench_tabs().contains(&WorkbenchTab::Agents));
        app.activate_workbench_tab(WorkbenchTab::CloseAgents);
        assert!(!app.workbench_tabs().contains(&WorkbenchTab::Agents));
        app.activate_workbench_tab(WorkbenchTab::CloseConversation(session));
        assert_eq!(app.workbench_tabs(), vec![WorkbenchTab::Home]);
        assert_eq!(app.active_workbench_tab(), WorkbenchTab::Home);
    }

    #[test]
    fn native_choice_selection_tracks_shared_runtime_options() {
        let mut app = App::default();
        app.state.active_model = "gpt-5.4".into();
        app.state.active_reasoning_level = "high".into();
        app.open_reasoning();
        let reasoning = app.toolbar_control("reasoning").unwrap();
        assert_eq!(reasoning.options[app.popup_index].value, "high");
        app.state.goal_mode = true;
        app.open_goal();
        let goal = app.toolbar_control("goal").unwrap();
        assert_eq!(goal.options[app.popup_index].value, "true");
        app.state.goal_mode = false;
        app.open_goal();
        let goal = app.toolbar_control("goal").unwrap();
        assert_eq!(goal.options[app.popup_index].value, "false");
    }

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
        app.diff_frame.hits.register(
            Rect::new(0, 0, 10, 1),
            DiffAction::SetFull {
                view: second,
                full: true,
            },
        );
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
        app.apply_shared_navigation(crate::shared_ui::actions::NavigationAction::Diff(0));
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
