use super::{App, Mode, WorkbenchTab, files::FilesState};
use crate::{
    harness::resources::{self, GitRefresh, GitSnapshot},
    model::FrontendCommand,
    workbench::{ResourceTarget, WorkspaceContent},
};
use crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;

pub(crate) use crate::shared_ui::home::HomeAction;

#[derive(Debug, Default)]
pub struct HomeRefresh {
    pub git: GitSnapshot,
    refresh: GitRefresh,
}

impl HomeRefresh {
    pub fn set_git_snapshot(&mut self, snapshot: GitSnapshot) {
        self.git = snapshot.clone();
        self.refresh.set_snapshot(snapshot);
    }

    fn refresh_git(&mut self, workspace: &std::path::Path, force: bool) {
        self.refresh.refresh_git(workspace, force);
        self.git = self.refresh.snapshot.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn stable_resource_targets_open_files_and_deleted_directory_diffs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-q"]);
        std::fs::create_dir(root.join("nested")).unwrap();
        let path = root.join("nested/real.rs");
        std::fs::write(&path, "before\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "Fixture",
        ]);
        let mut app = App::default();
        app.input = "Keep this draft".into();
        app.cursor = 3;
        app.open_resource(ResourceTarget::File(path.clone()));
        app.open_resource(ResourceTarget::File(path.clone()));
        assert_eq!(app.mode, Mode::Files);
        assert_eq!(
            app.files.as_ref().unwrap().tabs.views()[0].state.resource,
            Some(path.clone())
        );
        assert_eq!(
            app.files.as_ref().unwrap().selected_path(),
            Some(path.clone())
        );
        app.observe_workbench_view();
        assert_eq!(
            app.recent_views.entries()[0].target,
            ResourceTarget::File(path.clone())
        );
        std::fs::remove_dir_all(root.join("nested")).unwrap();
        app.open_resource(ResourceTarget::Diff(path.clone()));
        assert_eq!(app.mode, Mode::Diff);
        let diff = &app.diff_tabs.active().unwrap().state;
        assert!(diff.error.is_none(), "{:?}", diff.error);
        assert_eq!(diff.selected_path(), Some(path.clone()));
        assert!(diff.lines.iter().any(|line| line == "-before"));
        app.observe_workbench_view();
        assert_eq!(
            app.recent_views.entries()[0].target,
            ResourceTarget::Diff(path)
        );
        assert_eq!(app.input, "Keep this draft");
        assert_eq!(app.cursor, 3);
    }

    #[test]
    fn new_session_waits_for_delivery_before_shared_home_commit() {
        let mut app = App::default();
        let before = app.application.projection().ui_revision;
        app.apply_home_action(HomeAction::NewSession);
        assert!(matches!(
            app.take_workbench_command(),
            Some(FrontendCommand::NewSession)
        ));
        assert_eq!(app.application.projection().ui_revision, before);
        assert!(
            app.finish_home_delivery(Err(anyhow::anyhow!("delivery failed")))
                .is_err()
        );
        assert_eq!(app.application.projection().ui_revision, before);

        app.apply_home_action(HomeAction::NewSession);
        assert!(matches!(
            app.take_workbench_command(),
            Some(FrontendCommand::NewSession)
        ));
        app.finish_home_delivery(Ok(())).unwrap();
        assert!(app.application.projection().ui_revision > before);
        assert!(!app.home_visible());
    }

    #[test]
    fn home_mouse_wheel_selection_updates_shared_projection() {
        let mut app = App::default();
        let first = ResourceTarget::Session("first".into());
        let second = ResourceTarget::Session("second".into());
        let mut content = WorkspaceContent::default();
        content.sessions.extend([
            crate::shared_ui::home::ResourceItem::new(
                first.clone(),
                crate::shared_ui::home::ResourceKind::Session,
                "First session",
            ),
            crate::shared_ui::home::ResourceItem::new(
                second.clone(),
                crate::shared_ui::home::ResourceKind::Session,
                "Second session",
            ),
        ]);
        app.application.update_home_content(content);
        app.activate_workbench_tab(WorkbenchTab::Home);
        app.home_targets.push((
            ratatui::layout::Rect::new(0, 0, 1, 1),
            HomeAction::NewSession,
        ));

        assert_eq!(app.application.projection().home.selected, Some(first));
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });

        let projection = app.application.projection();
        assert_eq!(projection.home.selected, Some(second.clone()));
        assert_eq!(
            projection.home.inspector.map(|item| item.target),
            Some(second)
        );
        app.reveal_home_selection(true, 1);
        assert_eq!(app.home_scroll, 2);
    }
}

impl App {
    pub fn workspace_path(&self) -> PathBuf {
        self.state
            .known_workspaces
            .iter()
            .find(|workspace| workspace.is_current)
            .map(|workspace| PathBuf::from(&workspace.path))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    pub(crate) fn observe_workbench_view(&mut self) {
        let resource = match self.mode {
            Mode::Chat if !self.home_visible() => {
                let target = self
                    .state
                    .current_session_id
                    .as_ref()
                    .map_or(ResourceTarget::CurrentSession, |id| {
                        ResourceTarget::Session(id.clone())
                    });
                let title = self
                    .state
                    .saved_sessions
                    .iter()
                    .find(|session| Some(&session.id) == self.state.current_session_id.as_ref())
                    .map(|session| session.display_title())
                    .unwrap_or_else(|| "Current session".into());
                Some((target, title))
            }
            Mode::Files => self.files.as_ref().map(|files| {
                if let Some(path) = files.active_resource().cloned() {
                    let title = path
                        .strip_prefix(self.workspace_path())
                        .unwrap_or(&path)
                        .display()
                        .to_string();
                    (
                        if files.diff {
                            ResourceTarget::Diff(path)
                        } else {
                            ResourceTarget::File(path)
                        },
                        title,
                    )
                } else {
                    (ResourceTarget::Files(files.dir.clone()), "Files".into())
                }
            }),
            Mode::Diff => self.diff_tabs.active().and_then(|view| {
                let diff = &view.state;
                diff.selected_path().map(|path| {
                    let title = path
                        .strip_prefix(&diff.root)
                        .unwrap_or(&path)
                        .display()
                        .to_string();
                    (ResourceTarget::Diff(path), title)
                })
            }),
            // Overlays retain the underlying view's visit time.
            Mode::Chat => None,
            _ => return,
        };
        if let Some((target, title)) = resource {
            if self.observed_resource.as_ref() != Some(&target) {
                self.recent_views.visit(target.clone(), title);
                self.observed_resource = Some(target);
            } else {
                self.recent_views.update_title(&target, title);
            }
        } else {
            self.observed_resource = None;
        }
    }

    pub(crate) fn refresh_home(&mut self, force: bool) {
        self.home.refresh_git(&self.workspace_path(), force);
        let content = WorkspaceContent::collect(&self.state, &self.recent_views, &self.home.git);
        self.application.update_home_content(content);
    }

    pub(crate) fn apply_home_action(&mut self, action: HomeAction) {
        let selecting = matches!(action, HomeAction::Select(_) | HomeAction::MoveSelection(_));
        let prepared = self.application.prepare_home(action, &self.state);
        if let Some(command) = prepared.effect.command.clone() {
            self.workbench_command = Some(command);
            self.pending_home = Some(prepared);
            return;
        }
        if let Some(open) = prepared.effect.open.clone() {
            self.open_resource(open);
        }
        self.application.commit_home(prepared, &self.state);
        if selecting {
            self.input_focused = false;
        }
    }

    pub(crate) fn finish_home_delivery(
        &mut self,
        delivered: anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        if delivered.is_ok() {
            if let Some(prepared) = self.pending_home.take() {
                self.application.commit_home(prepared, &self.state);
                self.activate_workbench_tab(WorkbenchTab::Session);
                self.input_focused = true;
            }
        } else {
            self.pending_home = None;
        }
        delivered
    }

    pub(crate) fn reveal_home_selection(&mut self, include_sessions: bool, capacity: usize) {
        let home = self.application.projection().home;
        let start = if include_sessions {
            0
        } else {
            home.activity
                .iter()
                .position(|row| matches!(row, crate::shared_ui::home::HomeRow::Gap))
                .map_or(0, |index| index + 1)
        };
        let rows = &home.activity[start..];
        self.home_scroll = self.home_scroll.min(rows.len().saturating_sub(capacity));
        let selected = rows.iter().position(|row| {
            matches!(row, crate::shared_ui::home::HomeRow::Item(item)
                if Some(&item.target) == home.selected.as_ref())
        });
        if let Some(selected) = selected {
            if selected < self.home_scroll {
                self.home_scroll = selected;
            } else if selected >= self.home_scroll.saturating_add(capacity) {
                self.home_scroll = selected.saturating_sub(capacity.saturating_sub(1));
            }
        }
    }

    /// Route stable shared resource targets through the existing workbench.
    pub fn open_resource(&mut self, target: ResourceTarget) {
        match target {
            ResourceTarget::Session(id) => {
                self.activate_workbench_tab(WorkbenchTab::Session);
                if self.state.current_session_id.as_ref() != Some(&id) {
                    self.workbench_command = Some(FrontendCommand::LoadSession { session_id: id });
                }
                self.follow_tail = true;
            }
            ResourceTarget::CurrentSession => self.activate_workbench_tab(WorkbenchTab::Session),
            ResourceTarget::Task(_) => self.activate_workbench_tab(WorkbenchTab::Agents),
            ResourceTarget::Files(path) => {
                if let Some(files) = &mut self.files {
                    files.open_browser(path);
                } else {
                    self.files = Some(FilesState::open(path));
                }
                self.mode = Mode::Files;
            }
            ResourceTarget::Diff(path) => self.open_diff(Some(path)),
            ResourceTarget::File(path) => {
                if !resources::path_is_file(&path) {
                    self.backend_message = Some(format!("File unavailable: {}", path.display()));
                    return;
                }
                if self.files.is_none() {
                    self.files = Some(FilesState::open(self.workspace_path()));
                }
                if let Some(files) = &mut self.files {
                    files.open_path(path, false);
                }
                self.mode = Mode::Files;
            }
            ResourceTarget::Status => {
                self.mode = Mode::Status;
                self.workbench_command = Some(FrontendCommand::RequestAuth);
            }
        }
        self.home_targets.clear();
        self.tab_targets.clear();
    }

    /// Shared queue for pointer actions and the existing session sidebar.
    pub fn take_workbench_command(&mut self) -> Option<FrontendCommand> {
        self.workbench_command.take().or_else(|| {
            self.take_sidebar_load_request()
                .map(|session_id| FrontendCommand::LoadSession { session_id })
        })
    }

    pub(crate) fn handle_home_key(&mut self, event: &KeyEvent) -> bool {
        if self.mode != Mode::Chat
            || !self.home_visible()
            || self.state.error_message.is_some()
            || self.state.pending_shell_permission.is_some()
            || self.state.pending_native_app_permission.is_some()
            || !event.modifiers.is_empty()
        {
            return false;
        }
        if self.input_focused {
            if event.code == KeyCode::Esc {
                self.input_focused = false;
                return true;
            }
            return false;
        }
        match event.code {
            KeyCode::Char('i') | KeyCode::Esc | KeyCode::Tab => self.input_focused = true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.apply_home_action(HomeAction::MoveSelection(1))
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.apply_home_action(HomeAction::MoveSelection(-1))
            }
            KeyCode::PageDown => self.apply_home_action(HomeAction::MoveSelection(8)),
            KeyCode::PageUp => self.apply_home_action(HomeAction::MoveSelection(-8)),
            KeyCode::Home => self.apply_home_action(HomeAction::MoveSelection(i32::MIN)),
            KeyCode::End => self.apply_home_action(HomeAction::MoveSelection(i32::MAX)),
            KeyCode::Enter | KeyCode::Right => {
                if let Some(target) = self.application.projection().home.selected {
                    self.apply_home_action(HomeAction::Open(target));
                }
            }
            KeyCode::Char('r') => self.refresh_home(true),
            KeyCode::Char('n' | '+') => self.apply_home_action(HomeAction::NewSession),
            _ => return false,
        }
        true
    }
}
