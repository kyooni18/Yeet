//! Terminal-only input adapter. Task business logic remains in Harness.
use super::*;
use crate::harness::tasks::{Schedule, Task, TaskAction, TaskDraft};
use std::path::PathBuf;

pub(crate) enum Stage {
    Browse,
    Edit,
    Folder,
    Delete(String),
}
pub(crate) struct TaskPanel {
    pub stage: Stage,
    pub selected: usize,
    pub selected_id: Option<String>,
    pub editing_id: Option<String>,
    pub fields: [String; 5],
    pub field: usize,
    pub text: String,
    pub error: Option<String>,
    pub folder: PathBuf,
    pub directories: Vec<String>,
    pub directory_index: usize,
    pub pending: bool,
    pub last_refresh: Instant,
}
impl TaskPanel {
    pub fn new(folder: PathBuf, model: String) -> Self {
        Self {
            stage: Stage::Browse,
            selected: 0,
            selected_id: None,
            editing_id: None,
            fields: [
                String::new(),
                folder.display().to_string(),
                model,
                "every 3600".into(),
                String::new(),
            ],
            field: 0,
            text: String::new(),
            error: None,
            folder,
            directories: vec![],
            directory_index: 0,
            pending: false,
            last_refresh: Instant::now(),
        }
    }
    pub fn paste(&mut self, text: &str) {
        if self.pending {
            return;
        }
        let target = match self.stage {
            Stage::Edit => &mut self.fields[self.field],
            Stage::Delete(_) => &mut self.text,
            _ => return,
        };
        target.extend(
            text.chars()
                .filter(|c| !c.is_control() || (self.field == 4 && *c == '\n')),
        );
    }
    pub fn sync(&mut self, tasks: &[Task], notice: &Option<String>) {
        if self.pending
            && let Some(notice) = notice
        {
            self.pending = false;
            if notice == "Task updated" {
                self.stage = Stage::Browse;
                self.error = None;
            } else {
                self.error = Some(notice.clone());
            }
        }
        if let Some(id) = &self.selected_id {
            if let Some(index) = tasks.iter().position(|t| &t.id == id) {
                self.selected = index;
            }
        }
        self.selected = self.selected.min(tasks.len().saturating_sub(1));
        self.selected_id = tasks.get(self.selected).map(|t| t.id.clone());
    }
    fn browse_folder(&mut self, folder: PathBuf) -> anyhow::Result<()> {
        let folder = folder.canonicalize()?;
        std::fs::read_dir(&folder)?;
        self.directories = crate::harness::resources::directory_entries(&folder)
            .into_iter()
            .filter(|e| e.is_dir)
            .map(|e| e.name)
            .collect();
        self.directory_index = 0;
        self.folder = folder;
        self.stage = Stage::Folder;
        Ok(())
    }
    fn draft(&self) -> anyhow::Result<TaskDraft> {
        let schedule = self.fields[3].trim();
        let schedule = if let Some(at) = schedule.strip_prefix("at ") {
            Schedule::At {
                at: chrono::DateTime::parse_from_rfc3339(at.trim())
                    .map_err(|_| anyhow::anyhow!("Use at YYYY-MM-DDTHH:MM:SS+HH:MM (or Z)"))?
                    .with_timezone(&chrono::Utc),
            }
        } else if let Some(seconds) = schedule.strip_prefix("every ") {
            Schedule::Interval {
                seconds: seconds
                    .trim()
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Use every SECONDS"))?,
            }
        } else if schedule == "quota" {
            Schedule::QuotaReset
        } else {
            anyhow::bail!("Schedule: at RFC3339, every SECONDS, or quota");
        };
        Ok(TaskDraft {
            name: self.fields[0].clone(),
            folder: self.fields[1].clone().into(),
            model: self.fields[2].clone(),
            schedule,
            prompt: self.fields[4].clone(),
        })
    }
    fn key(
        &mut self,
        event: KeyEvent,
        tasks: &[Task],
    ) -> anyhow::Result<(bool, Option<TaskAction>)> {
        if self.pending {
            return Ok((false, None));
        }
        if event.code == KeyCode::Esc {
            if matches!(self.stage, Stage::Browse) {
                return Ok((true, None));
            }
            self.stage = if matches!(self.stage, Stage::Folder) {
                Stage::Edit
            } else {
                Stage::Browse
            };
            self.error = None;
            return Ok((false, None));
        }
        let control = event.modifiers.contains(KeyModifiers::CONTROL);
        let mut action = None;
        match &self.stage {
            Stage::Browse => match event.code {
                KeyCode::Char('q') => return Ok((true, None)),
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1).min(tasks.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
                KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
                KeyCode::End | KeyCode::Char('G') => self.selected = tasks.len().saturating_sub(1),
                KeyCode::Char('n') => {
                    self.editing_id = None;
                    self.fields[0].clear();
                    self.fields[4].clear();
                    self.fields[3] = "every 3600".into();
                    self.field = 0;
                    self.stage = Stage::Edit;
                    self.error = None;
                }
                KeyCode::Char('e') | KeyCode::Enter => {
                    if let Some(task) = tasks.get(self.selected) {
                        self.editing_id = Some(task.id.clone());
                        self.fields = [
                            task.draft.name.clone(),
                            task.draft.folder.display().to_string(),
                            task.draft.model.clone(),
                            match task.draft.schedule {
                                Schedule::At { at } => format!("at {}", at.to_rfc3339()),
                                Schedule::Interval { seconds } => format!("every {seconds}"),
                                Schedule::QuotaReset => "quota".into(),
                            },
                            task.draft.prompt.clone(),
                        ];
                        self.field = 0;
                        self.stage = Stage::Edit;
                        self.error = None;
                    }
                }
                KeyCode::Char('p') => {
                    if let Some(task) = tasks.get(self.selected) {
                        action = Some(TaskAction::SetEnabled {
                            id: task.id.clone(),
                            enabled: !task.enabled,
                        });
                    }
                }
                KeyCode::Char('d') => {
                    if let Some(task) = tasks.get(self.selected) {
                        self.stage = Stage::Delete(task.id.clone());
                        self.text.clear();
                        self.error = None;
                    }
                }
                _ => {}
            },
            Stage::Edit => match event.code {
                KeyCode::Char('s') if control => {
                    action = Some(TaskAction::Save {
                        id: self.editing_id.clone(),
                        draft: self.draft()?,
                    });
                }
                KeyCode::Char('b') if control && self.field == 1 => {
                    self.browse_folder(self.fields[1].clone().into())?
                }
                KeyCode::Tab => self.field = (self.field + 1) % 5,
                KeyCode::BackTab => self.field = (self.field + 4) % 5,
                KeyCode::Char('u') if control => self.fields[self.field].clear(),
                KeyCode::Enter if self.field == 4 => self.fields[4].push('\n'),
                KeyCode::Enter => self.field = (self.field + 1) % 5,
                KeyCode::Backspace => {
                    self.fields[self.field].pop();
                }
                KeyCode::Char(c) if !control && !c.is_control() => self.fields[self.field].push(c),
                _ => {}
            },
            Stage::Folder => match event.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.directory_index =
                        (self.directory_index + 1).min(self.directories.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.directory_index = self.directory_index.saturating_sub(1)
                }
                KeyCode::Backspace => {
                    if let Some(parent) = self.folder.parent() {
                        self.browse_folder(parent.into())?;
                    }
                }
                KeyCode::Enter => {
                    if let Some(name) = self.directories.get(self.directory_index) {
                        self.browse_folder(self.folder.join(name))?;
                    }
                }
                KeyCode::Char('s') if control => {
                    self.fields[1] = self.folder.display().to_string();
                    self.stage = Stage::Edit;
                }
                _ => {}
            },
            Stage::Delete(id) => match event.code {
                KeyCode::Enter if self.text == "delete" => {
                    action = Some(TaskAction::Delete { id: id.clone() })
                }
                KeyCode::Enter => self.error = Some("Type delete to confirm".into()),
                KeyCode::Char('u') if control => self.text.clear(),
                KeyCode::Backspace => {
                    self.text.pop();
                }
                KeyCode::Char(c) if !control && !c.is_control() => self.text.push(c),
                _ => {}
            },
        }
        self.selected_id = tasks.get(self.selected).map(|t| t.id.clone());
        Ok((false, action))
    }
}
impl App {
    pub(crate) fn open_tasks(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        backend.send(FrontendCommand::RequestTasks)?;
        self.task_manager = Some(TaskPanel::new(
            std::env::current_dir()?,
            self.state.active_model.clone(),
        ));
        self.clear_transcript_selection();
        Ok(())
    }
    pub(crate) fn refresh_tasks(&mut self, backend: &mut Backend) -> anyhow::Result<()> {
        if let Some(panel) = &mut self.task_manager {
            panel.sync(&self.state.scheduled_tasks, &self.state.tasks_notice);
            if !panel.pending && panel.last_refresh.elapsed() >= Duration::from_secs(2) {
                backend.send(FrontendCommand::RequestTasks)?;
                panel.last_refresh = Instant::now();
            }
        }
        Ok(())
    }
    pub(super) fn handle_tasks_key(
        &mut self,
        event: KeyEvent,
        backend: &mut Backend,
    ) -> anyhow::Result<()> {
        let Some(panel) = &mut self.task_manager else {
            return Ok(());
        };
        panel.sync(&self.state.scheduled_tasks, &self.state.tasks_notice);
        match panel.key(event, &self.state.scheduled_tasks) {
            Ok((true, _)) => self.task_manager = None,
            Ok((false, Some(action))) => {
                self.state.tasks_notice = None;
                backend.send(FrontendCommand::ManageTask { action })?;
                panel.pending = true;
            }
            Err(error) => panel.error = Some(error.to_string()),
            _ => {}
        }
        if event.code == KeyCode::Char('r')
            && self
                .task_manager
                .as_ref()
                .is_some_and(|p| matches!(p.stage, Stage::Browse))
        {
            backend.send(FrontendCommand::RequestTasks)?;
        }
        Ok(())
    }
}
