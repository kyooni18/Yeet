//! Git-backed resource review state.
use super::App;
#[cfg(test)]
use super::Mode;
use crate::harness::resources;
pub use crate::shared_ui::diff::DiffAction;
use crossterm::event::{KeyCode, KeyEvent};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct DiffState {
    pub root: PathBuf,
    pub paths: Vec<PathBuf>,
    pub selected: usize,
    pub statuses: Vec<String>,
    pub totals: crate::workbench::ChangeStats,
    pub original_paths: Vec<Option<PathBuf>>,
    pub lines: Vec<String>,
    pub full: bool,
    pub scroll: usize,
    pub error: Option<String>,
    pub branch: String,
    pub base: String,
}
/// Only the most recently painted diff can receive pointer input.
#[derive(Debug, Default)]
pub struct DiffFrame {
    pub view: Option<crate::tui::kit::SurfaceId>,
    pub hits: crate::tui::kit::HitMap<DiffAction>,
    pub body: ratatui::layout::Rect,
}
impl DiffFrame {
    pub fn begin(&mut self, bounds: ratatui::layout::Rect) {
        self.view = None;
        self.hits.begin(bounds);
        self.body = ratatui::layout::Rect::default();
    }
}
impl DiffState {
    pub fn open(dir: &Path, selected: Option<&Path>) -> Self {
        let mut s = Self {
            root: dir.into(),
            ..Self::default()
        };
        match resources::repository_root(dir) {
            Ok(root) => s.root = root,
            Err(_) => {
                s.error = Some("Not a Git repository".into());
                return s;
            }
        }
        s.reload();
        if let Some(p) = selected {
            s.select_path(p);
        }
        s
    }
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.paths.get(self.selected).map(|p| self.root.join(p))
    }
    pub fn select_path(&mut self, p: &Path) {
        let canonical = resources::canonical_resource_path(p);
        let p = canonical.as_path();
        if let Some(i) = self.paths.iter().position(|f| self.root.join(f) == p) {
            self.apply_action(DiffAction::SelectFile(self.root.join(&self.paths[i])));
        } else if p.starts_with(&self.root) && resources::path_is_file(p) {
            // An explicitly requested clean file must not silently select a different change.
            self.paths
                .push(p.strip_prefix(&self.root).unwrap().to_path_buf());
            self.statuses.push("  ".into());
            self.original_paths.push(None);
            self.apply_action(DiffAction::SelectFile(self.root.join(
                self.paths.last().expect("the selected path was just inserted"),
            )));
        }
    }
    pub fn reload(&mut self) {
        let old = self.selected_path();
        self.error = None;
        (self.branch, self.base) = resources::git_identity(&self.root);
        let snapshot = crate::workbench::GitSnapshot::load(&self.root);
        self.paths.clear();
        self.statuses.clear();
        self.original_paths.clear();
        self.totals = Default::default();
        self.error = snapshot.message;
        for change in snapshot.changes {
            if let Some(stats) = change.stats {
                self.totals.added += stats.added;
                self.totals.removed += stats.removed;
            }
            self.paths.push(change.path);
            self.statuses.push(change.status);
            self.original_paths.push(change.previous_path);
        }
        self.selected = self.selected.min(self.paths.len().saturating_sub(1));
        if let Some(p) = old {
            self.select_path(&p);
        }
        self.patch();
    }
    pub fn patch(&mut self) {
        self.lines.clear();
        let Some(p) = self.paths.get(self.selected) else {
            return;
        };
        self.error = None;
        match resources::review_patch(
            &self.root,
            p,
            self.original_paths.get(self.selected).and_then(|path| path.as_deref()),
            self.statuses.get(self.selected).is_some_and(|status| status == "??"),
            !self.base.is_empty(),
            self.full,
        ) {
            Ok(lines) => self.lines = lines,
            Err(error) => self.error = Some(error),
        }
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(1));
    }
    pub fn apply_action(&mut self, action: DiffAction) -> bool {
        match action {
            DiffAction::SelectFile(target) => {
                let Some(index) = self
                    .paths
                    .iter()
                    .position(|path| self.root.join(path) == target)
                else {
                    return false;
                };
                self.selected = index;
                self.scroll = 0;
                self.patch();
                true
            }
            DiffAction::SetFull(full) => {
                self.full = full;
                self.scroll = 0;
                self.patch();
                true
            }
        }
    }
    pub fn move_file(&mut self, delta: isize) {
        let selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.paths.len().saturating_sub(1));
        if let Some(path) = self.paths.get(selected) {
            self.apply_action(DiffAction::SelectFile(self.root.join(path)));
        } else {
            self.scroll = 0;
            self.patch();
        }
    }
}
impl App {
    pub(crate) fn open_diff(&mut self, path: Option<PathBuf>) {
        let dir = path
            .as_ref()
            .and_then(|p| {
                p.ancestors()
                    .skip(1)
                    .find(|dir| resources::path_is_dir(dir))
            })
            .map(Path::to_path_buf)
            .or_else(|| self.files.as_ref().map(|f| f.dir.clone()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let state = DiffState::open(&dir, path.as_deref());
        if let Some(id) = self
            .diff_tabs
            .views()
            .iter()
            .find(|view| view.state.root == state.root)
            .map(|view| view.id)
        {
            self.diff_tabs.activate(id);
            let existing = &mut self.diff_tabs.active_mut().unwrap().state;
            existing.reload();
            if let Some(p) = path {
                existing.select_path(&p);
            }
        } else {
            self.diff_tabs.open("Diff", state);
        }
        self.diff_frame.begin(ratatui::layout::Rect::default());
        self.report_navigation(super::WorkbenchTab::NewDiff);
    }
    pub(crate) fn handle_diff_key(&mut self, e: KeyEvent) {
        if e.code == KeyCode::Esc {
            self.activate_workbench_tab(super::WorkbenchTab::Home);
            return;
        }
        if matches!(e.code, KeyCode::Enter | KeyCode::Char('o')) {
            if let Some(p) = self
                .diff_tabs
                .active()
                .and_then(|view| view.state.selected_path())
                .filter(|p| resources::path_is_file(p))
            {
                if self.files.is_none() {
                    self.open_files();
                }
                if let Some(f) = &mut self.files {
                    f.open_path(p, false);
                }
                self.report_navigation(super::WorkbenchTab::Files);
            }
            return;
        }
        let Some(s) = self.diff_tabs.active_mut().map(|view| &mut view.state) else {
            return;
        };
        match e.code {
            KeyCode::Down | KeyCode::Char('j') => {
                s.scroll = (s.scroll + 1).min(s.lines.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => s.scroll = s.scroll.saturating_sub(1),
            KeyCode::PageDown => s.scroll = (s.scroll + 15).min(s.lines.len().saturating_sub(1)),
            KeyCode::PageUp => s.scroll = s.scroll.saturating_sub(15),
            KeyCode::Home => s.scroll = 0,
            KeyCode::End => s.scroll = s.lines.len().saturating_sub(1),
            KeyCode::Right | KeyCode::Char(']') => s.move_file(1),
            KeyCode::Left | KeyCode::Char('[') => s.move_file(-1),
            KeyCode::Char('f') => {
                s.apply_action(DiffAction::SetFull(!s.full));
            }
            KeyCode::Char('r') => s.reload(),
            KeyCode::Char('n') => {
                if let Some((i, _)) = s
                    .lines
                    .iter()
                    .enumerate()
                    .find(|(i, l)| *i > s.scroll && l.starts_with("@@"))
                {
                    s.scroll = i;
                }
            }
            KeyCode::Char('p') => {
                if let Some((i, _)) = s
                    .lines
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(i, l)| *i < s.scroll && l.starts_with("@@"))
                {
                    s.scroll = i;
                }
            }
            _ => {}
        }
    }
}

impl App {
    pub(crate) fn handle_diff_mouse(&mut self, e: crossterm::event::MouseEvent) {
        use crossterm::event::{MouseButton, MouseEventKind};
        if self.diff_frame.view != self.diff_tabs.active_id() {
            return;
        }
        let Some(s) = self.diff_tabs.active_mut().map(|view| &mut view.state) else {
            return;
        };
        let point = (e.column, e.row).into();
        match e.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(action) = self.diff_frame.hits.at(point).cloned() {
                    s.apply_action(action);
                }
            }
            MouseEventKind::ScrollDown if self.diff_frame.body.contains(point) => {
                s.scroll = (s.scroll + 3).min(s.lines.len().saturating_sub(1))
            }
            MouseEventKind::ScrollUp if self.diff_frame.body.contains(point) => {
                s.scroll = s.scroll.saturating_sub(3)
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn shared_diff_actions_select_stable_paths_and_reject_stale_targets() {
        let root = PathBuf::from("/workspace");
        let mut state = DiffState {
            root: root.clone(),
            paths: vec![
                PathBuf::from("src/current.rs"),
                PathBuf::from("src/other.rs"),
            ],
            selected: 0,
            ..DiffState::default()
        };
        let other = root.join("src/other.rs");
        assert!(state.apply_action(DiffAction::SelectFile(other.clone())));
        assert_eq!(state.selected_path(), Some(other.clone()));

        assert!(!state.apply_action(DiffAction::SelectFile(
            root.join("src/removed.rs")
        )));
        assert_eq!(state.selected_path(), Some(other));
        state.scroll = 7;
        assert!(state.apply_action(DiffAction::SetFull(false)));
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn git_review_is_wired_to_launcher_files_tabs_and_mouse() {
        let dir = tempfile::tempdir().unwrap();
        let canonical_root = dir.path().canonicalize().unwrap();
        let root = canonical_root.as_path();
        let run = |args: &[&str]| {
            git(root, args).unwrap();
        };
        run(&["init", "-q"]);
        std::fs::write(root.join("alpha.rs"), "old\ncontext\n").unwrap();
        std::fs::write(root.join("gone.txt"), "removed\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "base",
        ]);
        std::fs::write(root.join("alpha.rs"), "new\ncontext\n").unwrap();
        run(&["add", "alpha.rs"]); // staged changes must be visible too
        std::fs::write(root.join("alpha.rs"), "new\ncontext\nunstaged\n").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::write(root.join("space name.txt"), "untracked\n").unwrap();
        let mut app = App::default();
        app.files = Some(super::super::files::FilesState::open(root.into()));
        app.input = "keep draft".into();
        app.open_views();
        app.views_index = 3;
        app.handle_views_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Diff);
        assert_eq!(app.diff_tabs.len(), 1);
        app.diff_tabs
            .active_mut()
            .unwrap()
            .state
            .select_path(&root.join("alpha.rs"));
        assert!(
            app.diff_tabs
                .active_mut()
                .unwrap()
                .state
                .lines
                .iter()
                .any(|l| l == "+unstaged")
        );
        assert!(
            app.diff_tabs
                .active_mut()
                .unwrap()
                .state
                .lines
                .iter()
                .any(|l| l == "-old")
        );
        let mut terminal = Terminal::new(TestBackend::new(144, 44)).unwrap();
        terminal
            .draw(|f| crate::tui::ui::draw(f, &mut app))
            .unwrap();
        let control = app
            .diff_frame
            .hits
            .targets()
            .find(|(_, a)| *a == DiffAction::SetFull(true))
            .unwrap()
            .0;
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: control.x,
            row: control.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.diff_tabs.active_mut().unwrap().state.full);
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(rendered.contains("changes only"));
        app.handle_diff_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Files);
        assert_eq!(
            app.files.as_ref().unwrap().tabs.views()[0]
                .state
                .resource
                .clone()
                .unwrap(),
            root.join("alpha.rs")
        );
        app.handle_files_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Diff);
        assert_eq!(app.diff_tabs.len(), 1);
        assert_eq!(app.input, "keep draft");
        app.diff_tabs
            .active_mut()
            .unwrap()
            .state
            .select_path(&root.join("space name.txt"));
        assert!(
            app.diff_tabs
                .active_mut()
                .unwrap()
                .state
                .lines
                .iter()
                .any(|l| l == "+untracked")
        );
        app.diff_tabs
            .active_mut()
            .unwrap()
            .state
            .select_path(&root.join("gone.txt"));
        assert!(
            app.diff_tabs
                .active_mut()
                .unwrap()
                .state
                .lines
                .iter()
                .any(|l| l == "-removed")
        );
        std::fs::write(root.join("binary.bin"), [0, 1, 2, 3]).unwrap();
        app.diff_tabs.active_mut().unwrap().state.reload();
        app.diff_tabs
            .active_mut()
            .unwrap()
            .state
            .select_path(&root.join("binary.bin"));
        assert!(
            app.diff_tabs
                .active_mut()
                .unwrap()
                .state
                .lines
                .iter()
                .any(|l| l.contains("Binary files"))
        );
        run(&["mv", "alpha.rs", "renamed.rs"]);
        app.diff_tabs.active_mut().unwrap().state.reload();
        app.diff_tabs
            .active_mut()
            .unwrap()
            .state
            .select_path(&root.join("renamed.rs"));
        assert!(
            app.diff_tabs
                .active_mut()
                .unwrap()
                .state
                .lines
                .iter()
                .any(|l| l == "+unstaged")
        );
        std::fs::write(root.join("clean.txt"), "unchanged\n").unwrap();
        run(&["add", "clean.txt"]);
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "staged",
        ]);
        app.diff_tabs.active_mut().unwrap().state.reload();
        app.diff_tabs
            .active_mut()
            .unwrap()
            .state
            .select_path(&root.join("clean.txt"));
        assert_eq!(
            app.diff_tabs.active_mut().unwrap().state.selected_path(),
            Some(root.join("clean.txt"))
        );
        assert!(app.diff_tabs.active_mut().unwrap().state.lines.is_empty());
        for (width, height) in [(80, 24), (40, 18), (8, 5), (4, 3)] {
            let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
            t.draw(|f| crate::tui::ui::draw(f, &mut app)).unwrap();
        }
        app.activate_workbench_tab(super::super::WorkbenchTab::CloseDiff(
            app.diff_tabs.active_id().unwrap(),
        ));
        assert!(app.diff_tabs.is_empty());
        assert_eq!(app.mode, Mode::Chat);
    }
    #[test]
    fn clean_unborn_and_non_repository_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = DiffState::open(dir.path(), None);
        assert!(state.error.is_some());
        git(dir.path(), &["init", "-q"]).unwrap();
        state = DiffState::open(dir.path(), None);
        assert!(state.error.is_none());
        assert!(state.paths.is_empty());
        std::fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        git(dir.path(), &["add", "."]).unwrap();
        state.reload();
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(state.lines.iter().any(|l| l == "+new"));
    }
}

#[cfg(test)]
fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let o = std::process::Command::new("git")
        .arg("--literal-pathspecs")
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(o.stdout)
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().into())
    }
}
