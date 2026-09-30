//! Git-backed resource review state.
use super::{App, Mode};
use crossterm::event::{KeyCode, KeyEvent};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
#[derive(Debug, Clone, Default)]
pub struct DiffState {
    pub root: PathBuf,
    pub paths: Vec<PathBuf>,
    pub selected: usize,
    pub statuses: Vec<String>,
    pub original_paths: Vec<Option<PathBuf>>,
    pub file_targets: Vec<(ratatui::layout::Rect, usize)>,
    pub full_target: Option<ratatui::layout::Rect>,
    pub body_target: ratatui::layout::Rect,
    pub lines: Vec<String>,
    pub full: bool,
    pub scroll: usize,
    pub error: Option<String>,
    pub branch: String,
    pub base: String,
}
fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let o = Command::new("git")
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
impl DiffState {
    pub fn open(dir: &Path, selected: Option<&Path>) -> Self {
        let mut s = Self {
            root: dir.into(),
            ..Self::default()
        };
        match git(dir, &["rev-parse", "--show-toplevel"]) {
            Ok(o) => s.root = PathBuf::from(String::from_utf8_lossy(&o).trim()),
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
        let canonical = p
            .canonicalize()
            .or_else(|_| {
                p.parent()
                    .unwrap_or(Path::new("."))
                    .canonicalize()
                    .map(|parent| parent.join(p.file_name().unwrap_or_default()))
            })
            .unwrap_or_else(|_| p.to_path_buf());
        let p = canonical.as_path();
        if let Some(i) = self.paths.iter().position(|f| self.root.join(f) == p) {
            self.selected = i;
            self.scroll = 0;
            self.patch();
        } else if p.starts_with(&self.root) && p.is_file() {
            // An explicitly requested clean file must not silently select a different change.
            self.paths
                .push(p.strip_prefix(&self.root).unwrap().to_path_buf());
            self.statuses.push("  ".into());
            self.original_paths.push(None);
            self.selected = self.paths.len() - 1;
            self.scroll = 0;
            self.patch();
        }
    }
    pub fn reload(&mut self) {
        let old = self.selected_path();
        self.error = None;
        self.branch = git(&self.root, &["branch", "--show-current"])
            .map(|o| String::from_utf8_lossy(&o).trim().into())
            .unwrap_or_default();
        self.base = git(&self.root, &["rev-parse", "--short", "HEAD"])
            .map(|o| String::from_utf8_lossy(&o).trim().into())
            .unwrap_or_default();
        match git(
            &self.root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        ) {
            Ok(o) => {
                self.paths.clear();
                self.statuses.clear();
                self.original_paths.clear();
                let mut entries = o.split(|b| *b == 0).filter(|e| !e.is_empty());
                while let Some(e) = entries.next() {
                    if e.len() < 4 {
                        continue;
                    }
                    let original = if e[..2].contains(&b'R') || e[..2].contains(&b'C') {
                        entries.next().map(bytes_path)
                    } else {
                        None
                    };
                    self.original_paths.push(original);
                    self.paths.push(bytes_path(&e[3..]));
                    self.statuses
                        .push(String::from_utf8_lossy(&e[..2]).into_owned());
                }
            }
            Err(e) => {
                self.paths.clear();
                self.error = Some(e);
            }
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
        let untracked = self.statuses.get(self.selected).is_some_and(|s| s == "??");
        let mut c = Command::new("git");
        c.arg("--literal-pathspecs").current_dir(&self.root).args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            if self.full {
                "--unified=1000000"
            } else {
                "--unified=3"
            },
        ]);
        if untracked {
            c.args(["--no-index", "--", "/dev/null"]).arg(p);
        } else {
            let base = if self.base.is_empty() {
                git(&self.root, &["hash-object", "-t", "tree", "--stdin"])
                    .map(|o| String::from_utf8_lossy(&o).trim().to_owned())
                    .unwrap_or_default()
            } else {
                "HEAD".into()
            };
            c.arg(base).arg("--").arg(p);
            if let Some(Some(original)) = self.original_paths.get(self.selected) {
                c.arg(original);
            }
        }
        match c.output() {
            Ok(o) if o.status.success() || (untracked && o.status.code() == Some(1)) => {
                self.lines = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect()
            }
            Ok(o) => self.error = Some(String::from_utf8_lossy(&o.stderr).trim().into()),
            Err(e) => self.error = Some(e.to_string()),
        }
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(1));
    }
    pub fn move_file(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.paths.len().saturating_sub(1));
        self.scroll = 0;
        self.patch();
    }
}
#[cfg(unix)]
fn bytes_path(b: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(b))
}
#[cfg(not(unix))]
fn bytes_path(b: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(b).as_ref())
}
impl App {
    pub(crate) fn open_diff(&mut self, path: Option<PathBuf>) {
        let dir = path
            .as_ref()
            .and_then(|p| p.ancestors().skip(1).find(|dir| dir.is_dir()))
            .map(Path::to_path_buf)
            .or_else(|| self.files.as_ref().map(|f| f.dir.clone()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let state = DiffState::open(&dir, path.as_deref());
        self.active_diff = if let Some(i) = self.diff_tabs.iter().position(|s| s.root == state.root)
        {
            self.diff_tabs[i].reload();
            if let Some(p) = path {
                self.diff_tabs[i].select_path(&p);
            }
            i
        } else {
            self.diff_tabs.push(state);
            self.diff_tabs.len() - 1
        };
        self.mode = Mode::Diff;
    }
    pub(crate) fn handle_diff_key(&mut self, e: KeyEvent) {
        if e.code == KeyCode::Esc {
            self.mode = Mode::Chat;
            self.home_override = Some(true);
            return;
        }
        if matches!(e.code, KeyCode::Enter | KeyCode::Char('o')) {
            if let Some(p) = self
                .diff_tabs
                .get(self.active_diff)
                .and_then(DiffState::selected_path)
                .filter(|p| p.is_file())
            {
                if self.files.is_none() {
                    self.open_files();
                }
                if let Some(f) = &mut self.files {
                    let i = if let Some(i) = f.tabs.iter().position(|tab| *tab == p) {
                        i
                    } else {
                        f.tabs.push(p);
                        f.tabs.len() - 1
                    };
                    f.activate_tab(i);
                }
                self.mode = Mode::Files;
            }
            return;
        }
        let Some(s) = self.diff_tabs.get_mut(self.active_diff) else {
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
                s.full = !s.full;
                s.scroll = 0;
                s.patch();
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
        let Some(s) = self.diff_tabs.get_mut(self.active_diff) else {
            return;
        };
        let point = (e.column, e.row).into();
        match e.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(i) = s
                    .file_targets
                    .iter()
                    .find(|(r, _)| r.contains(point))
                    .map(|(_, i)| *i)
                {
                    s.selected = i;
                    s.scroll = 0;
                    s.patch();
                } else if s.full_target.is_some_and(|r| r.contains(point)) {
                    s.full = !s.full;
                    s.scroll = 0;
                    s.patch();
                }
            }
            MouseEventKind::ScrollDown if s.body_target.contains(point) => {
                s.scroll = (s.scroll + 3).min(s.lines.len().saturating_sub(1))
            }
            MouseEventKind::ScrollUp if s.body_target.contains(point) => {
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
        app.diff_tabs[0].select_path(&root.join("alpha.rs"));
        assert!(app.diff_tabs[0].lines.iter().any(|l| l == "+unstaged"));
        assert!(app.diff_tabs[0].lines.iter().any(|l| l == "-old"));
        let mut terminal = Terminal::new(TestBackend::new(144, 44)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let control = app.diff_tabs[0].full_target.unwrap();
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: control.x,
            row: control.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.diff_tabs[0].full);
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
        assert_eq!(app.files.as_ref().unwrap().tabs[0], root.join("alpha.rs"));
        app.handle_files_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Diff);
        assert_eq!(app.diff_tabs.len(), 1);
        assert_eq!(app.input, "keep draft");
        app.diff_tabs[0].select_path(&root.join("space name.txt"));
        assert!(app.diff_tabs[0].lines.iter().any(|l| l == "+untracked"));
        app.diff_tabs[0].select_path(&root.join("gone.txt"));
        assert!(app.diff_tabs[0].lines.iter().any(|l| l == "-removed"));
        std::fs::write(root.join("binary.bin"), [0, 1, 2, 3]).unwrap();
        app.diff_tabs[0].reload();
        app.diff_tabs[0].select_path(&root.join("binary.bin"));
        assert!(
            app.diff_tabs[0]
                .lines
                .iter()
                .any(|l| l.contains("Binary files"))
        );
        run(&["mv", "alpha.rs", "renamed.rs"]);
        app.diff_tabs[0].reload();
        app.diff_tabs[0].select_path(&root.join("renamed.rs"));
        assert!(app.diff_tabs[0].lines.iter().any(|l| l == "+unstaged"));
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
        app.diff_tabs[0].reload();
        app.diff_tabs[0].select_path(&root.join("clean.txt"));
        assert_eq!(
            app.diff_tabs[0].selected_path(),
            Some(root.join("clean.txt"))
        );
        assert!(app.diff_tabs[0].lines.is_empty());
        for (width, height) in [(80, 24), (40, 18), (8, 5), (4, 3)] {
            let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
            t.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        }
        app.activate_workbench_tab(super::super::WorkbenchTab::CloseDiff(0));
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
