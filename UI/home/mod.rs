//! Shared Home composition and interaction. Resources are supplied by Harness;
//! adapters retain viewport geometry, drawing and platform input translation.
mod content;
mod recent;
mod types;
use crate::harness::resources::{ChangeStats, GitSnapshot};
pub use content::{ResourceItem, ResourceKind, ResourceTarget, WorkspaceContent};
pub use recent::{RecentView, RecentViews};
pub use types::{HomeAction, HomeRow, HomeView, ResourceChanges, ResourceView};

#[derive(Debug, Clone)]
pub enum ActivityRow<'a> {
    Heading(&'static str),
    Item(&'a ResourceItem),
    Message(&'a str),
    Gap,
}
#[derive(Debug, Clone, Default)]
pub struct HomeState {
    pub content: WorkspaceContent,
    pub selected: Option<ResourceTarget>,
    pub scroll: usize,
}
#[derive(Debug)]
pub struct HomeProjection<'a> {
    pub overview_label: &'static str,
    pub summary: String,
    pub recent: Vec<&'a ResourceItem>,
    pub activity: Vec<ActivityRow<'a>>,
    pub inspector: Option<&'a ResourceItem>,
    pub open_label: &'static str,
    pub new_session_label: &'static str,
    pub recent_empty: &'static str,
    pub inspector_empty: &'static str,
    pub usage_label: &'static str,
}
impl HomeState {
    pub fn selected_item(&self) -> Option<&ResourceItem> {
        self.selected
            .as_ref()
            .and_then(|target| self.content.find(target))
    }
    pub fn replace_content(&mut self, content: WorkspaceContent) {
        self.content = content;
        if self.selected_item().is_none() {
            self.selected = self.content.items().next().map(|item| item.target.clone());
        }
    }
    pub fn select_next(&mut self, delta: isize) {
        let mut targets = Vec::new();
        for item in self.content.items() {
            if !targets.contains(&item.target) {
                targets.push(item.target.clone());
            }
        }
        let current = targets
            .iter()
            .position(|target| Some(target) == self.selected.as_ref())
            .unwrap_or(0);
        self.selected = targets
            .get(
                current
                    .saturating_add_signed(delta)
                    .min(targets.len().saturating_sub(1)),
            )
            .cloned();
    }
    pub fn reveal_selection(&mut self, include_sessions: bool, capacity: usize) {
        let projection = self.view(include_sessions);
        let len = projection.activity.len();
        let selected = projection.activity.iter().position(|row| matches!(row, ActivityRow::Item(item) if Some(&item.target) == self.selected.as_ref()));
        self.scroll = self.scroll.min(len.saturating_sub(capacity));
        if let Some(selected) = selected {
            if selected < self.scroll {
                self.scroll = selected;
            } else if selected >= self.scroll.saturating_add(capacity) {
                self.scroll = selected.saturating_sub(capacity.saturating_sub(1));
            }
        }
    }
    pub fn view(&self, include_sessions: bool) -> HomeProjection<'_> {
        let c = &self.content;
        let mut activity = Vec::new();
        let groups = [
            ("Sessions", &c.sessions, "No saved sessions"),
            ("Recent views", &c.recent_views, "No views opened yet"),
            ("Agent tasks", &c.tasks, "No agent tasks"),
            ("Changed files", &c.diffs, "Working tree clean"),
        ];
        for (index, (heading, items, empty)) in groups.into_iter().enumerate() {
            if index == 0 && !include_sessions {
                continue;
            }
            activity.push(ActivityRow::Heading(heading));
            activity.extend(items.iter().map(ActivityRow::Item));
            if index == 3 && c.git_message.is_some() {
                activity.push(ActivityRow::Message(c.git_message.as_deref().unwrap()));
            } else if items.is_empty() {
                activity.push(ActivityRow::Message(empty));
            }
            if index != 3 {
                activity.push(ActivityRow::Gap);
            }
        }
        let mut recent: Vec<&ResourceItem> = Vec::new();
        for item in c
            .recent_views
            .iter()
            .chain(&c.tasks)
            .chain(&c.diffs)
            .chain(&c.sessions)
        {
            if !recent.iter().any(|old| old.target == item.target) {
                recent.push(item);
            }
        }
        let workspace = c
            .workspace
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                let path = c.workspace.display().to_string();
                if path.is_empty() {
                    "Workspace".into()
                } else {
                    path
                }
            });
        let branch = if c.branch.is_empty() {
            "local"
        } else {
            &c.branch
        };
        let status = if c.git_message.is_some() {
            "Git status unavailable".into()
        } else if c.diffs.is_empty() {
            "working tree clean".into()
        } else {
            format!("{} changed", c.diffs.len())
        };
        let inspector = self.selected_item();
        HomeProjection {
            overview_label: "Overview",
            summary: format!("{workspace}  ·  {branch}  ·  {status}"),
            recent,
            activity,
            inspector,
            open_label: match inspector.map(|item| item.kind) {
                Some(ResourceKind::Session) => "Open session →",
                Some(ResourceKind::Task) => "Open agents →",
                Some(ResourceKind::Diff) => "Review diff →",
                Some(ResourceKind::File) => "Open file →",
                _ => "Open view →",
            },
            new_session_label: "+  New session",
            recent_empty: "No recent objects",
            inspector_empty: "Select a session, recent view, task or changed file.",
            usage_label: "Usage details →",
        }
    }

    /// Project the current Home composition into an owned transport-safe view.
    pub fn project(&self, include_sessions: bool) -> HomeView {
        let projection = self.view(include_sessions);
        HomeView {
            overview_label: projection.overview_label.into(),
            summary: projection.summary,
            recent: projection
                .recent
                .into_iter()
                .map(ResourceView::from)
                .collect(),
            activity: projection
                .activity
                .into_iter()
                .map(|row| match row {
                    ActivityRow::Heading(label) => HomeRow::Heading(label.into()),
                    ActivityRow::Item(item) => HomeRow::Item(ResourceView::from(item)),
                    ActivityRow::Message(message) => HomeRow::Message(message.into()),
                    ActivityRow::Gap => HomeRow::Gap,
                })
                .collect(),
            selected: self.selected.clone(),
            inspector: projection.inspector.map(ResourceView::from),
            open_label: projection.open_label.into(),
            new_session_label: projection.new_session_label.into(),
            recent_empty: projection.recent_empty.into(),
            inspector_empty: projection.inspector_empty.into(),
            usage_label: projection.usage_label.into(),
        }
    }

    /// Reject stale resource identities before a platform applies the action.
    /// Status is a stable command target for the existing usage affordance and
    /// can be opened, but it is not a selectable content item.
    pub fn validate_action(&self, action: HomeAction) -> Option<HomeAction> {
        match &action {
            HomeAction::NewSession => Some(action),
            HomeAction::Open(ResourceTarget::Status) => Some(action),
            HomeAction::Select(ResourceTarget::Status) => None,
            HomeAction::Select(target) | HomeAction::Open(target)
                if self.content.find(target).is_some() =>
            {
                Some(action)
            }
            HomeAction::Select(_) | HomeAction::Open(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn home_composition_deduplicates_recent_targets_and_keeps_selection_visible_on_resize() {
        let session = ResourceItem::new(
            ResourceTarget::Session("one".into()),
            ResourceKind::Session,
            "One",
        );
        let file = ResourceItem::new(
            ResourceTarget::File("/workspace/file.rs".into()),
            ResourceKind::File,
            "file.rs",
        );
        let mut state = HomeState::default();
        state.replace_content(WorkspaceContent {
            sessions: vec![session.clone()],
            recent_views: vec![file.clone(), session],
            ..Default::default()
        });
        assert_eq!(state.view(false).recent.len(), 2);
        assert!(matches!(
            state.view(false).activity[0],
            ActivityRow::Heading("Recent views")
        ));
        assert!(matches!(
            state.view(true).activity[0],
            ActivityRow::Heading("Sessions")
        ));
        state.selected = Some(file.target.clone());
        state.reveal_selection(true, 1);
        assert!(
            matches!(&state.view(true).activity[state.scroll], ActivityRow::Item(item) if item.target == file.target)
        );
        state.reveal_selection(false, 30);
        assert_eq!(state.scroll, 0);
        assert_eq!(state.view(false).open_label, "Open file →");
        state.replace_content(WorkspaceContent::default());
        assert!(state.selected.is_none());
        assert_eq!(
            state.view(false).summary,
            "Workspace  ·  local  ·  working tree clean"
        );
    }

    #[test]
    fn owned_home_projection_roundtrips_stable_targets_and_rejects_stale_actions() {
        let file = ResourceItem::new(
            ResourceTarget::File("/workspace/file.rs".into()),
            ResourceKind::File,
            "file.rs",
        );
        let target = file.target.clone();
        let mut state = HomeState::default();
        state.replace_content(WorkspaceContent {
            recent_views: vec![file.clone()],
            diffs: vec![file],
            ..Default::default()
        });
        state.selected = Some(target.clone());

        let view = state.project(false);
        let wire = serde_json::to_value(&view).unwrap();
        let decoded: HomeView = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded, view);
        assert_eq!(decoded.selected, Some(target.clone()));
        assert!(decoded.recent.iter().all(|item| item.target == target));
        assert!(matches!(decoded.activity[1], HomeRow::Item(_)));

        assert_eq!(
            state.validate_action(HomeAction::Open(target.clone())),
            Some(HomeAction::Open(target.clone()))
        );
        assert_eq!(
            state.validate_action(HomeAction::Select(target.clone())),
            Some(HomeAction::Select(target))
        );
        assert!(
            state
                .validate_action(HomeAction::Open(ResourceTarget::Status))
                .is_some()
        );
        assert!(
            state
                .validate_action(HomeAction::Select(ResourceTarget::Status))
                .is_none()
        );
        assert!(
            state
                .validate_action(HomeAction::Open(ResourceTarget::Session("stale".into())))
                .is_none()
        );
    }
}
