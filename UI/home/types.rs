//! Owned, platform-neutral Home projection and stable actions.
use super::{ResourceItem, ResourceKind, ResourceTarget};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceView {
    pub target: ResourceTarget,
    pub kind: ResourceKind,
    pub title: String,
    pub context: String,
    pub age: String,
    pub detail: String,
    pub changes: Option<ResourceChanges>,
}

impl From<&ResourceItem> for ResourceView {
    fn from(item: &ResourceItem) -> Self {
        Self {
            target: item.target.clone(),
            kind: item.kind,
            title: item.title.clone(),
            context: item.context.clone(),
            age: item.age.clone(),
            detail: item.detail.clone(),
            changes: item.changes.map(|changes| ResourceChanges {
                added: changes.added,
                removed: changes.removed,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceChanges {
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeProviderUsageWindow {
    pub label: String,
    pub used_percent: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeProviderUsage {
    pub provider: String,
    pub available: bool,
    pub windows: Vec<HomeProviderUsageWindow>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum HomeRow {
    Heading(String),
    Item(ResourceView),
    Message(String),
    Gap,
}

/// Owned projection suitable for Remote/Tauri transport and native renderers.
/// It preserves Home's existing content and labels without specifying layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeView {
    pub workspace: String,
    pub overview_label: String,
    pub summary: String,
    pub providers: Vec<HomeProviderUsage>,
    pub recent: Vec<ResourceView>,
    pub activity: Vec<HomeRow>,
    pub selected: Option<ResourceTarget>,
    pub inspector: Option<ResourceView>,
    pub open_label: String,
    pub new_session_label: String,
    pub recent_empty: String,
    pub inspector_empty: String,
    pub usage_empty_label: String,
    pub usage_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum HomeAction {
    Select(ResourceTarget),
    Open(ResourceTarget),
    MoveSelection(i32),
    NewSession,
}
