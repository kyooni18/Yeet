//! Keyboard-driven file browser view (`Mode::Files`).
use super::{
    App, Mode,
    keymap::{Action, Context},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use crate::tui::kit::{SurfaceId, Tabs};

const PAGE: isize = 10;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Default)]
pub struct FileViewState {
    /// Resource identity is independent of the current rail selection.
    pub resource: Option<PathBuf>,
    pub dir: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub info: bool,
    pub changed_only: bool,
    pub diff: bool,
    pub hints: bool,
    pub count: Option<usize>,
    pub find: Option<String>,
    pub find_locked: bool,
    pub recent_dirs: Vec<PathBuf>,
    pub changed: HashMap<String, String>,
    pub diff_lines: Vec<String>,
    pub selected_lines: Option<usize>,
    pub diff_stats: Option<(usize, usize)>,
    pub git_branch: String,
    pub git_commit: String,
}

/// The browser and every opened file own independent navigation/filter state.
/// Deref exposes only the currently visible view to existing rendering helpers.
#[derive(Debug, Clone, Default)]
pub struct FilesState {
    pub tabs: Tabs<FileViewState>,
    browser: FileViewState,
    showing_browser: bool,
}
impl Deref for FilesState {
    type Target = FileViewState;
    fn deref(&self) -> &Self::Target {
        if !self.showing_browser
            && let Some(view) = self.tabs.active()
        {
            &view.state
        } else {
            &self.browser
        }
    }
}
impl DerefMut for FilesState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        if !self.showing_browser
            && let Some(view) = self.tabs.active_mut()
        {
            &mut view.state
        } else {
            &mut self.browser
        }
    }
}
impl FileViewState {
    fn open(dir: PathBuf) -> Self {
        let mut state = Self {
            recent_dirs: vec![dir.clone()],
            dir,
            ..Self::default()
        };
        state.reload();
        state
    }
}

impl FilesState {
    pub fn open(dir: PathBuf) -> Self {
        Self {
            browser: FileViewState::open(dir),
            tabs: Tabs::default(),
            showing_browser: true,
        }
    }

    pub fn active_tab(&self) -> Option<SurfaceId> {
        (!self.showing_browser)
            .then(|| self.tabs.active_id())
            .flatten()
    }

    pub(crate) fn activate_browser(&mut self) {
        self.showing_browser = true;
    }

    pub(crate) fn open_browser(&mut self, dir: PathBuf) {
        self.browser = FileViewState::open(dir);
        self.showing_browser = true;
    }

    pub(crate) fn active_resource(&self) -> Option<&PathBuf> {
        self.active_tab()
            .and_then(|id| self.tabs.get(id))
            .and_then(|view| view.state.resource.as_ref())
    }
}

impl FileViewState {
    pub fn reload(&mut self) {
        self.changed = git_changes(&self.dir);
        self.git_branch = git(&self.dir, &["branch", "--show-current"])
            .unwrap_or_default()
            .trim()
            .to_owned();
        self.git_commit = git(&self.dir, &["rev-parse", "--short", "HEAD"])
            .unwrap_or_default()
            .trim()
            .to_owned();
        let mut entries: Vec<FileEntry> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                Some(FileEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    is_dir: meta.is_dir(),
                    size: meta.len(),
                    modified: meta.modified().ok(),
                })
            })
            .collect();
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.entries = entries;
        self.cursor = 0;
        self.refresh_detail();
    }

    pub(crate) fn reload_preserving_selection(&mut self) {
        let selected = self.selected_path();
        let cursor = self.cursor;
        self.reload();
        self.cursor = selected
            .and_then(|path| {
                self.visible()
                    .iter()
                    .position(|entry| self.dir.join(&entry.name) == path)
            })
            .unwrap_or(cursor);
        self.refresh_detail();
    }

    pub fn visible(&self) -> Vec<&FileEntry> {
        let query = self.find.as_deref().map(str::to_lowercase);
        self.entries
            .iter()
            .filter(|entry| {
                (!self.changed_only || self.is_changed(entry))
                    && query
                        .as_deref()
                        .is_none_or(|q| entry.name.to_lowercase().contains(q))
            })
            .collect()
    }

    pub fn is_changed(&self, entry: &FileEntry) -> bool {
        if entry.is_dir {
            let prefix = format!("{}/", entry.name);
            self.changed.keys().any(|path| path.starts_with(&prefix))
        } else {
            self.changed.contains_key(&entry.name)
        }
    }

    pub fn selected(&self) -> Option<&FileEntry> {
        self.visible().get(self.cursor).copied()
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        self.selected().map(|entry| self.dir.join(&entry.name))
    }

    fn clamp(&mut self) {
        let count = self.visible().len();
        self.cursor = self.cursor.min(count.saturating_sub(1));
    }

    fn refresh_detail(&mut self) {
        self.clamp();
        self.selected_lines = self.selected().and_then(|entry| {
            (!entry.is_dir)
                .then(|| count_text_lines(&self.dir.join(&entry.name)))
                .flatten()
        });
        self.diff_stats = self.selected().and_then(|entry| {
            (!entry.is_dir)
                .then(|| git_numstat(&self.dir, &entry.name))
                .flatten()
        });
        self.diff_lines = match (self.diff, self.selected()) {
            (true, Some(entry)) if !entry.is_dir => git_diff(&self.dir, &entry.name),
            _ => Vec::new(),
        };
    }

    fn move_cursor(&mut self, delta: isize) {
        let count = self.visible().len();
        if count == 0 {
            return;
        }
        self.cursor = self
            .cursor
            .saturating_add_signed(delta)
            .min(count.saturating_sub(1));
        self.refresh_detail();
    }

    fn set_dir(&mut self, dir: PathBuf) {
        self.recent_dirs.retain(|recent| recent != &dir);
        self.recent_dirs.insert(0, dir.clone());
        self.recent_dirs.truncate(5);
        self.dir = dir;
        self.find = None;
        self.find_locked = false;
        self.reload();
    }

    fn go_parent(&mut self) {
        let name = self
            .dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            self.set_dir(parent);
            if let Some(name) = name
                && let Some(index) = self.visible().iter().position(|e| e.name == name)
            {
                self.cursor = index;
                self.refresh_detail();
            }
        }
    }
}

impl FilesState {
    fn open_selected(&mut self, as_tab: bool) {
        let Some(entry) = self.selected().cloned() else {
            return;
        };
        let path = self.dir.join(&entry.name);
        if entry.is_dir && !as_tab {
            self.set_dir(path);
            return;
        }
        self.open_path(path, self.diff);
    }

    pub(crate) fn activate_tab(&mut self, id: SurfaceId) -> bool {
        if !self.tabs.activate(id) {
            return false;
        }
        self.showing_browser = false;
        // Refresh external data while restoring the local selection and filters.
        self.reload_preserving_selection();
        true
    }

    /// Open a resource without resetting the state of an existing view.
    pub fn open_path(&mut self, path: PathBuf, diff: bool) {
        if let Some(id) = self
            .tabs
            .views()
            .iter()
            .find(|view| view.state.resource.as_ref() == Some(&path))
            .map(|view| view.id)
        {
            self.activate_tab(id);
            return;
        }
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut state = FileViewState::open(dir);
        state.resource = Some(path.clone());
        state.diff = diff;
        if let Some(cursor) = state
            .visible()
            .iter()
            .position(|entry| state.dir.join(&entry.name) == path)
        {
            state.cursor = cursor;
        }
        state.refresh_detail();
        self.tabs.open(
            path.file_name().unwrap_or_default().to_string_lossy(),
            state,
        );
        self.showing_browser = false;
    }

    pub(crate) fn close_tab(&mut self, id: SurfaceId) {
        if self.tabs.close(id).is_some() && self.tabs.is_empty() {
            self.showing_browser = true;
        }
    }

    fn cycle_tab(&mut self, back: bool) {
        let len = self.tabs.len();
        if len == 0 {
            return;
        }
        let current = self
            .active_tab()
            .and_then(|id| self.tabs.views().iter().position(|view| view.id == id));
        let next = match current {
            Some(i) if back => (i + len - 1) % len,
            Some(i) => (i + 1) % len,
            None if back => len - 1,
            None => 0,
        };
        self.activate_tab(self.tabs.views()[next].id);
    }

    /// Raw text entry while the find prompt is open; everything else goes
    /// through the keymap.
    pub fn handle_find_key(&mut self, event: KeyEvent) -> bool {
        if self.find_locked {
            return false;
        }
        let Some(query) = self.find.as_mut() else {
            return false;
        };
        match event.code {
            KeyCode::Esc => self.find = None,
            KeyCode::Enter => {
                if query.is_empty() {
                    self.find = None;
                } else {
                    self.find_locked = true;
                }
                return true;
            }
            KeyCode::Up | KeyCode::Down => {
                self.move_cursor(if event.code == KeyCode::Up { -1 } else { 1 });
                return true;
            }
            KeyCode::Backspace => {
                query.pop();
            }
            KeyCode::Char(c) if !event.modifiers.contains(KeyModifiers::CONTROL) => {
                query.push(c);
            }
            _ => {}
        }
        self.cursor = 0;
        self.refresh_detail();
        true
    }

    pub fn apply(&mut self, action: Action) -> FilesOutcome {
        match action {
            Action::Close if self.hints => self.hints = false,
            Action::Close if self.find.is_some() => {
                self.find = None;
                self.find_locked = false;
                self.cursor = 0;
                self.refresh_detail();
            }
            Action::Close => return FilesOutcome::Close,
            Action::FocusInput => {
                self.find.get_or_insert_with(String::new);
                self.find_locked = false;
            }
            Action::ToggleHints => self.hints = !self.hints,
            Action::ToggleInfo => self.info = !self.info,
            Action::ToggleChangedOnly => {
                self.changed_only = !self.changed_only;
                self.cursor = 0;
                self.refresh_detail();
            }
            Action::ToggleDiff => {
                self.diff = !self.diff;
                self.refresh_detail();
            }
            Action::Find => {
                self.find.get_or_insert_with(String::new);
                self.find_locked = false;
            }
            Action::ParentFolder => self.go_parent(),
            Action::OpenAsTab => self.open_selected(true),
            Action::OpenEntry => self.open_selected(false),
            Action::OpenViews => {}
            Action::MoveDown => self.move_cursor(1),
            Action::MoveUp => self.move_cursor(-1),
            Action::MoveTop => self.move_cursor(isize::MIN / 2),
            Action::MoveBottom => self.move_cursor(isize::MAX / 2),
            Action::NextTab => self.cycle_tab(false),
            Action::PrevTab => self.cycle_tab(true),
            Action::PageDown => self.move_cursor(PAGE),
            Action::PageUp => self.move_cursor(-PAGE),
        }
        FilesOutcome::Stay
    }

    /// Vim-style count prefix: `10j`, `3l`. Non-movement actions run once.
    pub fn apply_counted(&mut self, action: Action, count: usize) -> FilesOutcome {
        let repeat = match action {
            Action::MoveDown | Action::MoveUp | Action::PageDown | Action::PageUp => {
                return self.apply_scaled(action, count);
            }
            Action::ParentFolder | Action::OpenEntry | Action::NextTab | Action::PrevTab => count,
            _ => 1,
        };
        for step in 0..repeat.max(1) {
            if step > 0 && action == Action::OpenEntry && !self.selected().is_some_and(|e| e.is_dir)
            {
                break;
            }
            if self.apply(action) == FilesOutcome::Close {
                return FilesOutcome::Close;
            }
        }
        FilesOutcome::Stay
    }

    fn apply_scaled(&mut self, action: Action, count: usize) -> FilesOutcome {
        let step = match action {
            Action::MoveDown => 1,
            Action::MoveUp => -1,
            Action::PageDown => PAGE,
            _ => -PAGE,
        };
        self.move_cursor(step.saturating_mul(count.max(1) as isize));
        FilesOutcome::Stay
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FilesOutcome {
    Stay,
    Close,
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Paths relative to `dir` mapped to their two-character porcelain status.
fn git_changes(dir: &Path) -> HashMap<String, String> {
    let Some(prefix) = git(dir, &["rev-parse", "--show-prefix"]) else {
        return HashMap::new();
    };
    let prefix = prefix.trim();
    let Some(status) = git(dir, &["status", "--porcelain", "--untracked-files=all"]) else {
        return HashMap::new();
    };
    status
        .lines()
        .filter(|line| line.len() > 3)
        .filter_map(|line| {
            let path = line[3..].rsplit(" -> ").next()?.trim_matches('"');
            let relative = path.strip_prefix(prefix)?;
            Some((relative.to_owned(), line[..2].to_owned()))
        })
        .collect()
}

fn git_diff(dir: &Path, name: &str) -> Vec<String> {
    git(dir, &["diff", "HEAD", "--no-color", "--", name])
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn git_numstat(dir: &Path, name: &str) -> Option<(usize, usize)> {
    let output = git(dir, &["diff", "HEAD", "--numstat", "--", name])?;
    let mut parts = output.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn count_text_lines(path: &Path) -> Option<usize> {
    if std::fs::metadata(path).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    std::str::from_utf8(&bytes).ok()?;
    Some(
        bytes.iter().filter(|byte| **byte == b'\n').count()
            + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n")),
    )
}

impl App {
    pub(crate) fn open_files(&mut self) {
        let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let files = match self.files.take() {
            Some(mut files) => {
                files.reload_preserving_selection();
                files
            }
            None => FilesState::open(dir),
        };
        self.files = Some(files);
        self.mode = Mode::Files;
    }

    pub(crate) fn handle_files_key(&mut self, event: KeyEvent) {
        if self
            .files
            .as_mut()
            .is_some_and(|files| files.handle_find_key(event))
        {
            return;
        }
        let Some(files) = self.files.as_mut() else {
            self.activate_workbench_tab(super::WorkbenchTab::Home);
            return;
        };
        if event.modifiers.is_empty()
            && let KeyCode::Char(c @ '0'..='9') = event.code
            && (c != '0' || files.count.is_some())
        {
            let digit = c as usize - '0' as usize;
            files.count = Some(
                files
                    .count
                    .unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(digit)
                    .min(9999),
            );
            return;
        }
        let count = files.count.take();
        let Some(action) = self.keymap.lookup(Context::Files, &event) else {
            return;
        };
        if action == Action::ToggleDiff {
            let path = files
                .active_resource()
                .cloned()
                .or_else(|| files.selected_path());
            self.open_diff(path.filter(|p| !p.is_dir()));
            return;
        }
        if action == Action::OpenViews {
            self.open_views();
            return;
        }
        if files.apply_counted(action, count.unwrap_or(1)) == FilesOutcome::Close {
            self.activate_workbench_tab(super::WorkbenchTab::Home);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::keymap::Keymap;

    #[test]
    fn file_instances_restore_filters_selection_and_browser_state() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("alpha.rs");
        let second = dir.path().join("beta.rs");
        std::fs::write(&first, "a").unwrap();
        std::fs::write(&second, "b").unwrap();
        let mut files = FilesState::open(dir.path().to_path_buf());
        files.apply(Action::MoveDown);
        files.find = Some("bet".into());
        files.find_locked = true;
        files.refresh_detail();
        files.open_path(first.clone(), false);
        let first_id = files.active_tab().unwrap();
        files.info = true;
        files.hints = true;
        files.diff = true;
        files.find = Some("alp".into());
        files.find_locked = true;
        files.count = Some(3);
        files.open_path(second.clone(), false);
        let second_id = files.active_tab().unwrap();
        assert!(!files.info);
        assert_eq!(files.find, None);
        files.activate_tab(first_id);
        assert!(files.info && files.hints && files.diff && files.find_locked);
        assert_eq!(files.find.as_deref(), Some("alp"));
        assert_eq!(files.count, Some(3));
        assert_eq!(files.selected_path(), Some(first.clone()));
        files.activate_browser();
        assert_eq!(files.find.as_deref(), Some("bet"));
        assert_eq!(files.selected_path(), Some(second.clone()));
        files.open_path(first, false);
        assert_eq!(files.active_tab(), Some(first_id));
        assert!(files.diff, "reopening must not reset display options");
        files.close_tab(first_id);
        assert_eq!(files.active_tab(), Some(second_id));
        files.close_tab(first_id); // stale identity must not close the neighbor
        assert_eq!(files.tabs.len(), 1);
        files.open_browser(dir.path().to_path_buf());
        files.activate_tab(second_id);
        assert_eq!(files.selected_path(), Some(second));
        files.close_tab(second_id);
        assert_eq!(files.active_tab(), None);
    }

    #[test]
    fn reopening_files_keeps_the_selected_entry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.rs"), "a\n").unwrap();
        std::fs::write(dir.path().join("beta.rs"), "b\n").unwrap();
        let mut app = App::default();
        app.files = Some(FilesState::open(dir.path().to_path_buf()));
        app.files.as_mut().unwrap().apply(Action::MoveDown);
        app.open_files();
        assert_eq!(
            app.files.as_ref().unwrap().selected().unwrap().name,
            "beta.rs"
        );
    }

    fn press(files: &mut FilesState, keymap: &Keymap, code: KeyCode) -> FilesOutcome {
        let event = KeyEvent::new(code, KeyModifiers::NONE);
        if files.handle_find_key(event) {
            return FilesOutcome::Stay;
        }
        keymap
            .lookup(Context::Files, &event)
            .map_or(FilesOutcome::Stay, |action| files.apply(action))
    }

    #[test]
    fn keys_navigate_toggle_and_find() {
        let root = std::env::temp_dir().join(format!("yeet-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("alpha.rs"), "a").unwrap();
        std::fs::write(root.join("beta.rs"), "b").unwrap();
        let keymap = Keymap::default();
        let mut files = FilesState::open(root.clone());
        let key = |files: &mut FilesState, code| press(files, &keymap, code);
        assert_eq!(files.visible()[0].name, "sub");

        key(&mut files, KeyCode::Char('j'));
        key(&mut files, KeyCode::Char(' '));
        assert_eq!(
            files.tabs.views()[0].state.resource,
            Some(root.join("alpha.rs"))
        );

        key(&mut files, KeyCode::Char('p'));
        assert!(files.info);
        key(&mut files, KeyCode::Char('?'));
        assert!(files.hints);
        key(&mut files, KeyCode::Esc);
        assert!(!files.hints);

        key(&mut files, KeyCode::Char('/'));
        for c in "bet".chars() {
            key(&mut files, KeyCode::Char(c));
        }
        assert_eq!(files.visible().len(), 1);
        key(&mut files, KeyCode::Enter);
        assert!(files.find_locked);
        key(&mut files, KeyCode::Char('j'));
        assert_eq!(files.find.as_deref(), Some("bet"));
        key(&mut files, KeyCode::Esc);
        assert_eq!(files.visible().len(), 3);

        key(&mut files, KeyCode::Char('g'));
        key(&mut files, KeyCode::Enter);
        assert_eq!(files.dir, root.join("sub"));
        key(&mut files, KeyCode::Char('h'));
        assert_eq!(files.dir, root);
        assert_eq!(files.selected().unwrap().name, "sub");
        assert_eq!(key(&mut files, KeyCode::Char('q')), FilesOutcome::Close);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn count_prefix_and_page_keys_move_by_the_expected_amount() {
        let root = std::env::temp_dir().join(format!("yeet-files-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        for n in 0..30 {
            std::fs::write(root.join(format!("f{n:02}")), "x").unwrap();
        }
        let mut files = FilesState::open(root.clone());
        files.apply_counted(Action::MoveDown, 12);
        assert_eq!(files.cursor, 12);
        files.apply_counted(Action::PageDown, 1);
        assert_eq!(files.cursor, 22);
        files.apply_counted(Action::MoveUp, 100);
        assert_eq!(files.cursor, 0);
        files.apply_counted(Action::OpenEntry, 3);
        assert_eq!(files.dir, root.join("a/b"));
        files.apply_counted(Action::ParentFolder, 2);
        assert_eq!(files.dir, root);
        let _ = std::fs::remove_dir_all(&root);
    }
}
