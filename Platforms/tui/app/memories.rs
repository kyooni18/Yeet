//! Local terminal memory-manager input. No protected state is sent through
//! FrontendCommand, conversations, agent history or Remote replay.

use super::*;
use crate::harness::memory::*;
use std::collections::BTreeMap;

pub(crate) enum Stage {
    Browse,
    Search,
    Inspect(MemoryRecord),
    Edit(Option<MemoryRecord>),
    Delete(Vec<(String, u64)>),
    Share(MemoryRecord),
    Download,
    Remove,
    Cloud,
    ConfirmCloud { profile: String, model: String },
}

pub(crate) struct MemoryPanel {
    pub manager: LocalMemoryManager,
    pub stage: Stage,
    pub selected: usize,
    pub marked: BTreeMap<String, u64>,
    pub text: String,
    pub error: Option<String>,
    pub scroll: u16,
    cursors: Vec<Option<String>>,
}

impl MemoryPanel {
    pub fn paste(&mut self, text: &str) {
        if matches!(self.stage, Stage::Browse | Stage::Inspect(_)) {
            return;
        }
        let multiline = matches!(self.stage, Stage::Edit(_));
        self.text.extend(
            text.chars()
                .filter(|c| !c.is_control() || (multiline && matches!(c, '\n' | '\t'))),
        );
    }
    pub fn selected_record(&self) -> Option<&MemoryRecord> {
        self.manager.page.records.get(self.selected)
    }
    fn reset_filter(&mut self) -> anyhow::Result<()> {
        self.manager.filter.after = None;
        self.cursors.clear();
        self.selected = 0;
        self.manager.refresh()
    }
    fn handle_key(&mut self, event: KeyEvent) -> anyhow::Result<bool> {
        use Stage::*;
        if event.code == KeyCode::Esc {
            if matches!(self.stage, Browse) {
                return Ok(true);
            }
            self.stage = Browse;
            self.text.clear();
            self.error = None;
            self.scroll = 0;
            return Ok(false);
        }
        let control = event.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(self.stage, Browse) {
            match event.code {
                KeyCode::Char('q') => return Ok(true),
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected =
                        (self.selected + 1).min(self.manager.page.records.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
                KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
                KeyCode::End | KeyCode::Char('G') => {
                    self.selected = self.manager.page.records.len().saturating_sub(1)
                }
                KeyCode::Char('/') => {
                    self.stage = Search;
                    self.text = self.manager.filter.query.clone().unwrap_or_default();
                }
                KeyCode::Char('f') => {
                    self.manager.filter.kind = match self.manager.filter.kind {
                        None => Some(MemoryKind::Explicit),
                        Some(MemoryKind::Explicit) => Some(MemoryKind::WholeSession),
                        _ => None,
                    };
                    self.reset_filter()?;
                }
                KeyCode::Char('v') => {
                    self.manager.filter.privacy = match self.manager.filter.privacy {
                        None => Some(Privacy::Shared),
                        Some(Privacy::Shared) => Some(Privacy::LocalOnly),
                        _ => None,
                    };
                    self.reset_filter()?;
                }
                KeyCode::Char(']') => {
                    if let Some(next) = self.manager.page.next_cursor.clone() {
                        self.cursors.push(self.manager.filter.after.clone());
                        self.manager.filter.after = Some(next);
                        self.selected = 0;
                        self.manager.refresh()?;
                    }
                }
                KeyCode::Char('[') => {
                    if let Some(previous) = self.cursors.pop() {
                        self.manager.filter.after = previous;
                        self.selected = 0;
                        self.manager.refresh()?;
                    }
                }
                KeyCode::Char('r') => self.manager.refresh()?,
                KeyCode::Char(' ') => {
                    if let Some(record) = self.selected_record().cloned()
                        && self.marked.remove(&record.id).is_none()
                    {
                        self.marked.insert(record.id, record.revision);
                    }
                }
                KeyCode::Char('a') => {
                    self.stage = Edit(None);
                    self.text.clear();
                    self.scroll = 0;
                }
                KeyCode::Char('e') => {
                    if let Some(record) = self.selected_record().cloned() {
                        self.text = record.text.clone();
                        self.stage = Edit(Some(record));
                        self.scroll = 0;
                    }
                }
                KeyCode::Enter => {
                    if let Some(record) = self.selected_record().cloned() {
                        self.stage = Inspect(record);
                        self.scroll = 0;
                    }
                }
                KeyCode::Char('d') => {
                    let targets = if self.marked.is_empty() {
                        self.selected_record()
                            .map(|r| vec![(r.id.clone(), r.revision)])
                            .unwrap_or_default()
                    } else {
                        self.marked
                            .iter()
                            .map(|(id, revision)| (id.clone(), *revision))
                            .collect()
                    };
                    if !targets.is_empty() {
                        self.stage = Delete(targets);
                        self.text.clear();
                    }
                }
                KeyCode::Char('p') => {
                    if let Some(record) = self.selected_record().cloned() {
                        if record.privacy == Privacy::Shared {
                            self.manager.privacy(&record, Privacy::LocalOnly, false)?;
                        } else {
                            self.stage = Share(record);
                            self.text.clear();
                        }
                    }
                }
                KeyCode::Char('1') => self.manager.toggle_enabled(MemoryKind::Explicit)?,
                KeyCode::Char('2') => self.manager.toggle_enabled(MemoryKind::WholeSession)?,
                KeyCode::Char('3') => self.manager.cycle_shared_profile(MemoryKind::Explicit)?,
                KeyCode::Char('4') => self
                    .manager
                    .cycle_shared_profile(MemoryKind::WholeSession)?,
                KeyCode::Char('i') => {
                    self.stage = Download;
                    self.text.clear();
                }
                KeyCode::Char('c') => {
                    self.stage = Cloud;
                    self.text.clear();
                }
                KeyCode::Char('u') => {
                    self.stage = Remove;
                    self.text.clear();
                }
                KeyCode::Char('x') => {
                    let profile = self
                        .manager
                        .settings
                        .explicit
                        .shared_profile_id
                        .clone()
                        .or_else(|| {
                            self.manager
                                .settings
                                .whole_session
                                .shared_profile_id
                                .clone()
                        })
                        .ok_or_else(|| {
                            anyhow::anyhow!("Choose a managed-local profile with 3/4 first")
                        })?;
                    self.manager
                        .start_task(MemoryTask::IndexLocal { profile })?;
                }
                KeyCode::Char('b') => self.manager.start_task(MemoryTask::Reconcile)?,
                _ => {}
            }
        } else if matches!(self.stage, Inspect(_)) {
            match event.code {
                KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(12),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(12),
                KeyCode::Char('q') => {
                    self.stage = Browse;
                    self.scroll = 0;
                }
                _ => {}
            }
        } else {
            match event.code {
                KeyCode::Char('u') if control => self.text.clear(),
                KeyCode::Backspace => {
                    self.text.pop();
                }
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(12),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(12),
                KeyCode::Char('s') if control => self.submit()?,
                KeyCode::Enter if matches!(self.stage, Edit(_)) => self.text.push('\n'),
                KeyCode::Enter => self.submit()?,
                KeyCode::Tab if matches!(self.stage, Edit(_)) => self.text.push('\t'),
                KeyCode::Char(c) if !control && !event.modifiers.contains(KeyModifiers::SUPER) => {
                    self.text.push(c)
                }
                _ => {}
            }
        }
        self.selected = self
            .selected
            .min(self.manager.page.records.len().saturating_sub(1));
        Ok(false)
    }
    fn submit(&mut self) -> anyhow::Result<()> {
        use Stage::*;
        match &self.stage {
            Search => {
                self.manager.filter.query =
                    (!self.text.trim().is_empty()).then(|| self.text.clone());
                self.reset_filter()?;
            }
            Edit(record) => self.manager.save(record.as_ref(), self.text.clone())?,
            Delete(targets) => {
                anyhow::ensure!(self.text.trim() == "delete", "Type delete to confirm");
                self.manager.delete(targets)?;
                self.marked.clear();
            }
            Share(record) => {
                anyhow::ensure!(
                    self.text.trim() == "share",
                    "Type share to confirm declassification"
                );
                self.manager.privacy(record, Privacy::Shared, true)?;
            }
            Download => {
                anyhow::ensure!(
                    self.text.trim() == "download",
                    "Type download to consent to ~91 MB from Hugging Face"
                );
                self.manager.start_task(MemoryTask::PrepareLocal {
                    model: "minilm-l6-v2".into(),
                    profile: "managed-minilm-v1".into(),
                })?;
            }
            Remove => {
                anyhow::ensure!(self.text.trim() == "remove", "Type remove to confirm");
                self.manager.start_task(MemoryTask::RemoveLocal {
                    model: "minilm-l6-v2".into(),
                })?;
            }
            Cloud => {
                let fields = self.text.split_whitespace().collect::<Vec<_>>();
                anyhow::ensure!(
                    fields.len() == 2 && fields[1].contains('/'),
                    "Enter PROFILE_ID PROVIDER/MODEL; credentials use existing provider auth"
                );
                self.stage = ConfirmCloud {
                    profile: fields[0].into(),
                    model: fields[1].into(),
                };
                self.text.clear();
                return Ok(());
            }
            ConfirmCloud { profile, model } => {
                anyhow::ensure!(
                    self.text.trim() == "allow-cloud",
                    "Type allow-cloud to send the non-sensitive fixture"
                );
                self.manager.start_task(MemoryTask::ProbeCloud {
                    profile: profile.clone(),
                    model: model.clone(),
                })?;
            }
            _ => return Ok(()),
        }
        self.stage = Browse;
        self.text.clear();
        self.error = None;
        self.scroll = 0;
        Ok(())
    }
}

impl App {
    pub(crate) fn open_memories(&mut self) {
        let workspace = self
            .state
            .known_workspaces
            .iter()
            .find(|w| w.is_current)
            .map(|w| Path::new(&w.path).to_path_buf())
            .or_else(|| std::env::current_dir().ok());
        let result = workspace
            .ok_or_else(|| anyhow::anyhow!("No current workspace"))
            .and_then(|workspace| {
                LocalMemoryManager::open(crate::config::ConfigStore::default().directory, workspace)
            });
        match result {
            Ok(manager) => {
                self.memory_manager = Some(MemoryPanel {
                    manager,
                    stage: Stage::Browse,
                    selected: 0,
                    marked: BTreeMap::new(),
                    text: String::new(),
                    error: None,
                    scroll: 0,
                    cursors: vec![],
                });
                self.clear_transcript_selection();
            }
            Err(error) => self.backend_message = Some(format!("Memory manager: {error}")),
        }
    }
    pub(super) fn handle_memories_key(&mut self, event: KeyEvent) {
        let Some(panel) = &mut self.memory_manager else {
            return;
        };
        match panel.handle_key(event) {
            Ok(true) => self.memory_manager = None,
            Err(error) => panel.error = Some(error.to_string()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_manager_routes_edits_confirms_and_renders_without_chat_or_remote_data() {
        use crate::shared_ui::{composer::*, settings::*};
        let harness = crate::harness::HarnessState::default();
        let mut composer = ComposerState::default();
        let editor = EditorSnapshot {
            text: "/memories".into(),
            ..Default::default()
        };
        let env = ComposerEnvironment {
            supported_destinations: vec![ComposerDestination::Memories],
            ..Default::default()
        };
        let effect = composer.apply(ComposerAction::Submit(editor), &env, &harness);
        assert_eq!(effect.ui.destination, Some(ComposerDestination::Memories));
        assert!(effect.command.is_none());
        let mut settings = SettingsState::default();
        let effect = settings.apply(
            SettingsAction::Activate("memories".into()),
            &SettingsEnvironment {
                supported_destinations: vec![SettingsDestination::Memories],
                ..Default::default()
            },
            &harness,
        );
        assert_eq!(effect.ui.destination, Some(SettingsDestination::Memories));
        assert!(effect.command.is_none());

        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let manager = LocalMemoryManager::open(temporary.path().join("state"), workspace).unwrap();
        let mut panel = MemoryPanel {
            manager,
            stage: Stage::Browse,
            selected: 0,
            marked: BTreeMap::new(),
            text: String::new(),
            error: None,
            scroll: 0,
            cursors: vec![],
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        panel.handle_key(key(KeyCode::Char('a'))).unwrap();
        for c in "jk literal".chars() {
            panel.handle_key(key(KeyCode::Char(c))).unwrap();
        }
        panel.paste("\nUnicode 한글");
        assert_eq!(panel.text, "jk literal\nUnicode 한글");
        panel
            .handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(panel.manager.page.records.len(), 1);
        assert_eq!(
            panel.manager.page.records[0].text,
            "jk literal\nUnicode 한글"
        );
        panel.handle_key(key(KeyCode::Char('p'))).unwrap();
        assert_eq!(panel.manager.page.records[0].privacy, Privacy::LocalOnly);
        panel.handle_key(key(KeyCode::Char('p'))).unwrap();
        assert!(matches!(panel.stage, Stage::Share(_)));
        assert!(panel.handle_key(key(KeyCode::Enter)).is_err());
        panel.handle_key(key(KeyCode::Esc)).unwrap();
        let mut app = App::default();
        app.memory_manager = Some(panel);
        for (width, height) in [(120, 40), (44, 60)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| crate::tui::ui::draw(frame, &mut app))
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("Memories"));
            assert!(text.contains("LocalOnly"));
        }
        assert!(app.conversation.is_empty());
        let wire = serde_json::to_string(&app.state).unwrap();
        assert!(!wire.contains("jk literal"));
        let panel = app.memory_manager.as_mut().unwrap();
        panel.handle_key(key(KeyCode::Char('d'))).unwrap();
        assert!(panel.handle_key(key(KeyCode::Enter)).is_err());
        panel.paste("delete");
        panel.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(panel.manager.page.records.is_empty());
    }
}
