use super::{ChangeStats, GitSnapshot, RecentViews};
use crate::model::{BridgeState, ProviderUsageStatus};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::PathBuf};

/// Stable identities survive tab closing/reordering and live content updates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ResourceTarget {
    CurrentSession,
    Session(String),
    Files(PathBuf),
    File(PathBuf),
    Diff(PathBuf),
    Task(String),
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Session,
    View,
    File,
    Diff,
    Task,
}

#[derive(Debug, Clone)]
pub struct ResourceItem {
    pub target: ResourceTarget,
    pub kind: ResourceKind,
    pub title: String,
    pub context: String,
    pub age: String,
    pub detail: String,
    pub changes: Option<ChangeStats>,
}

impl ResourceItem {
    pub fn new(target: ResourceTarget, kind: ResourceKind, title: impl Into<String>) -> Self {
        Self {
            target,
            kind,
            title: title
                .into()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            context: String::new(),
            age: String::new(),
            detail: String::new(),
            changes: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceContent {
    pub workspace: PathBuf,
    pub sessions: Vec<ResourceItem>,
    pub recent_views: Vec<ResourceItem>,
    pub tasks: Vec<ResourceItem>,
    pub diffs: Vec<ResourceItem>,
    pub providers: Vec<ProviderUsageStatus>,
    pub branch: String,
    pub git_message: Option<String>,
}

impl WorkspaceContent {
    /// Pure adapter: collection and IO are supplied separately from rendering.
    pub fn collect(state: &BridgeState, recent: &RecentViews, git: &GitSnapshot) -> Self {
        let mut content = Self {
            workspace: git.workspace.clone(),
            branch: git.branch.clone(),
            git_message: git.message.clone(),
            ..Self::default()
        };
        let sessions = state
            .known_workspaces
            .iter()
            .find(|workspace| workspace.is_current)
            .and_then(|workspace| {
                state
                    .workspace_session_groups
                    .iter()
                    .find(|group| group.workspace_id == workspace.id)
            })
            .map_or(state.saved_sessions.as_slice(), |group| {
                group.sessions.as_slice()
            });
        let mut sorted: Vec<_> = sessions.iter().collect();
        sorted.sort_by(|a, b| {
            let date = |s: &crate::model::SessionSummary| {
                chrono::DateTime::parse_from_rfc3339(&s.updated_at).ok()
            };
            date(b).cmp(&date(a))
        });
        let mut ids = HashSet::new();
        for session in sorted {
            if !ids.insert(&session.id) {
                continue;
            }
            let mut item = ResourceItem::new(
                ResourceTarget::Session(session.id.clone()),
                ResourceKind::Session,
                session.display_title(),
            );
            item.context = state
                .session_activity
                .get(&session.id)
                .map_or("", |a| a.label())
                .into();
            if state.current_session_id.as_ref() == Some(&session.id) && item.context.is_empty() {
                if state.pending_shell_permission.is_some()
                    || state.pending_native_app_permission.is_some()
                {
                    item.context = "permission".into();
                } else if state.is_streaming {
                    item.context = "running".into();
                }
            }
            item.age = session.updated_label();
            item.detail = format!(
                "{} messages\nModel: {}\nUpdated: {}\nSession: {}",
                session.message_count, session.model, session.updated_at, session.id
            );
            content.sessions.push(item);
        }
        if let Some(id) = &state.current_session_id
            && !content
                .sessions
                .iter()
                .any(|item| item.target == ResourceTarget::Session(id.clone()))
        {
            let mut item = ResourceItem::new(
                ResourceTarget::Session(id.clone()),
                ResourceKind::Session,
                "Current session",
            );
            item.context = if state.is_streaming {
                "running"
            } else {
                "current"
            }
            .into();
            item.detail = format!("Model: {}\nSession: {id}", state.active_model);
            content.sessions.insert(0, item);
        }
        for view in recent.entries() {
            // Removed sessions cannot be reopened. Files are checked on activation.
            if let ResourceTarget::Session(id) = &view.target
                && !content
                    .sessions
                    .iter()
                    .any(|item| item.target == ResourceTarget::Session(id.clone()))
                && state.current_session_id.as_ref() != Some(id)
            {
                continue;
            }
            let kind = match view.target {
                ResourceTarget::File(_) => ResourceKind::File,
                ResourceTarget::Diff(_) => ResourceKind::Diff,
                ResourceTarget::Session(_) | ResourceTarget::CurrentSession => {
                    ResourceKind::Session
                }
                _ => ResourceKind::View,
            };
            let mut item = ResourceItem::new(view.target.clone(), kind, &view.title);
            item.age = elapsed_label(view.visited_at);
            item.context = "visited".into();
            item.detail = match &view.target {
                ResourceTarget::File(path)
                | ResourceTarget::Diff(path)
                | ResourceTarget::Files(path) => path.display().to_string(),
                ResourceTarget::Session(_) => content
                    .sessions
                    .iter()
                    .find(|session| session.target == view.target)
                    .map(|session| session.detail.clone())
                    .unwrap_or_default(),
                _ => view.title.clone(),
            };
            content.recent_views.push(item);
        }
        for task in &state.agent_tasks {
            let mut item = ResourceItem::new(
                ResourceTarget::Task(task.id.clone()),
                ResourceKind::Task,
                &task.objective,
            );
            item.context = format!("{} · {}", task.role, task.status);
            item.detail = format!(
                "{}\n\n{}",
                task.objective,
                task.summary.as_deref().unwrap_or("No summary yet.")
            );
            content.tasks.push(item);
        }
        for change in &git.changes {
            let mut item = ResourceItem::new(
                ResourceTarget::Diff(git.root.join(&change.path)),
                ResourceKind::Diff,
                change.path.display().to_string(),
            );
            item.context = change.status.clone();
            item.changes = change.stats;
            item.detail = format!(
                "{}\n\nIndex / working tree: {}\n{}",
                git.root.join(&change.path).display(),
                change.status,
                match change.stats {
                    Some(stats) => format!("{} added · {} removed", stats.added, stats.removed),
                    None => "Binary file or line counts unavailable".into(),
                }
            );
            if let Some(old) = &change.previous_path {
                item.detail
                    .push_str(&format!("\nRenamed from: {}", old.display()));
            }
            content.diffs.push(item);
        }
        content.providers = state
            .auth_providers
            .iter()
            .filter_map(|provider| provider.usage.clone())
            .collect();
        content
    }

    pub fn items(&self) -> impl Iterator<Item = &ResourceItem> {
        self.sessions
            .iter()
            .chain(self.recent_views.iter())
            .chain(self.tasks.iter())
            .chain(self.diffs.iter())
    }

    pub fn find(&self, target: &ResourceTarget) -> Option<&ResourceItem> {
        self.items().find(|item| &item.target == target)
    }
}

fn elapsed_label(date: chrono::DateTime<chrono::Utc>) -> String {
    let seconds = (chrono::Utc::now() - date).num_seconds().max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3600)
    }
}
