//! Keyboard-driven file browser view (`Mode::Files`).
use super::{
    App, Mode,
    keymap::{Action, Context},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const PAGE: isize = 10;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Default)]
pub struct FilesState {
    pub dir: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub info: bool,
    pub changed_only: bool,
    pub diff: bool,
    pub hints: bool,
    pub count: Option<usize>,
    pub find: Option<String>,
    pub tabs: Vec<PathBuf>,
    pub active_tab: Option<usize>,
    pub changed: HashMap<String, String>,
    pub diff_lines: Vec<String>,
}

impl FilesState {
    pub fn open(dir: PathBuf) -> Self {
        let mut state = Self {
            dir,
            ..Self::default()
        };
        state.reload();
        state
    }

    pub fn reload(&mut self) {
        self.changed = git_changes(&self.dir);
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
        self.dir = dir;
        self.find = None;
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

    fn open_selected(&mut self, as_tab: bool) {
        let Some(entry) = self.selected().cloned() else {
            return;
        };
        let path = self.dir.join(&entry.name);
        if entry.is_dir && !as_tab {
            self.set_dir(path);
            return;
        }
        let index = match self.tabs.iter().position(|tab| *tab == path) {
            Some(index) => index,
            None => {
                self.tabs.push(path);
                self.tabs.len() - 1
            }
        };
        self.active_tab = Some(index);
    }

    /// Raw text entry while the find prompt is open; everything else goes
    /// through the keymap.
    pub fn handle_find_key(&mut self, event: KeyEvent) -> bool {
        let Some(query) = self.find.as_mut() else {
            return false;
        };
        match event.code {
            KeyCode::Esc => self.find = None,
            KeyCode::Enter => {
                if query.is_empty() {
                    self.find = None;
                } else {
                    self.open_selected(false);
                    return true;
                }
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
            Action::Close | Action::FocusInput => return FilesOutcome::Close,
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
                self.find = Some(String::new());
                self.cursor = 0;
            }
            Action::ParentFolder => self.go_parent(),
            Action::OpenAsTab => self.open_selected(true),
            Action::OpenEntry => self.open_selected(false),
            Action::OpenViews => {}
            Action::MoveDown => self.move_cursor(1),
            Action::MoveUp => self.move_cursor(-1),
            Action::MoveTop => self.move_cursor(isize::MIN / 2),
            Action::MoveBottom => self.move_cursor(isize::MAX / 2),
            Action::NextTab if !self.tabs.is_empty() => {
                let next = self.active_tab.map_or(0, |i| (i + 1) % self.tabs.len());
                self.active_tab = Some(next);
            }
            Action::NextTab => {}
            Action::PrevTab if !self.tabs.is_empty() => {
                let len = self.tabs.len();
                let prev = self.active_tab.map_or(len - 1, |i| (i + len - 1) % len);
                self.active_tab = Some(prev);
            }
            Action::PrevTab => {}
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
            Some((relative.to_owned(), line[..2].trim().to_owned()))
        })
        .collect()
}

fn git_diff(dir: &Path, name: &str) -> Vec<String> {
    git(dir, &["diff", "HEAD", "--no-color", "--", name])
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

impl App {
    pub(crate) fn open_files(&mut self) {
        let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let files = match self.files.take() {
            Some(mut files) => {
                files.reload();
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
            self.mode = Mode::Chat;
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
        if action == Action::OpenViews {
            self.open_views();
            return;
        }
        if files.apply_counted(action, count.unwrap_or(1)) == FilesOutcome::Close {
            self.mode = Mode::Chat;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::keymap::Keymap;

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
        assert_eq!(files.tabs, vec![root.join("alpha.rs")]);

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
