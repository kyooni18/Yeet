//! Shared Home composition and interaction. Resources are supplied by Harness;
//! adapters retain viewport geometry, drawing and platform input translation.
mod content;
mod recent;
mod types;
use crate::harness::resources::{ChangeStats, GitSnapshot};
pub use content::{ResourceItem, ResourceKind, ResourceTarget, WorkspaceContent};
pub use recent::{RecentView, RecentViews};
pub use types::{
    HomeAction, HomeProviderUsage, HomeProviderUsageWindow, HomeRow, HomeView, ResourceChanges,
    ResourceView,
};

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
            workspace: self.content.workspace.display().to_string(),
            overview_label: projection.overview_label.into(),
            summary: projection.summary,
            providers: self
                .content
                .providers
                .iter()
                .map(|provider| HomeProviderUsage {
                    provider: provider.provider.clone(),
                    available: provider.available,
                    windows: provider
                        .windows
                        .iter()
                        .map(|window| HomeProviderUsageWindow {
                            label: window.label.clone(),
                            used_percent: window.used_percent,
                        })
                        .collect(),
                    message: provider.message.clone(),
                })
                .collect(),
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
            usage_empty_label: "Provider usage not reported".into(),
            usage_label: projection.usage_label.into(),
        }
    }

    /// Reject stale resource identities before a platform applies the action.
    /// Status is a stable command target for the existing usage affordance and
    /// can be opened, but it is not a selectable content item.
    pub fn validate_action(&self, action: HomeAction) -> Option<HomeAction> {
        match &action {
            HomeAction::NewSession => Some(action),
            HomeAction::MoveSelection(_) => Some(action),
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
    fn home_composition_deduplicates_recent_targets_and_preserves_selection() {
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
        state.select_next(isize::MAX);
        assert_eq!(state.selected, Some(file.target.clone()));
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
            workspace: "/workspace".into(),
            recent_views: vec![file.clone()],
            diffs: vec![file],
            providers: vec![crate::model::ProviderUsageStatus {
                provider: "test-provider".into(),
                available: true,
                source: "test".into(),
                fetched_at: String::new(),
                plan: None,
                windows: vec![crate::model::ProviderUsageWindow {
                    id: "primary".into(),
                    label: "5h".into(),
                    used_percent: 73,
                    remaining_percent: 27,
                    resets_at: None,
                }],
                message: None,
            }],
            ..Default::default()
        });
        state.selected = Some(target.clone());

        let view = state.project(false);
        let wire = serde_json::to_value(&view).unwrap();
        let decoded: HomeView = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded, view);
        assert_eq!(decoded.workspace, "/workspace");
        assert_eq!(decoded.providers[0].provider, "test-provider");
        assert_eq!(decoded.providers[0].windows[0].used_percent, 73);
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
        assert_eq!(
            state.validate_action(HomeAction::MoveSelection(1)),
            Some(HomeAction::MoveSelection(1))
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
