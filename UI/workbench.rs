//! Declarative workbench controls. Platforms place and paint these controls;
//! their identity, labels, semantic icons and actions belong to UI.
use super::surfaces::SurfaceId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "view", content = "id", rename_all = "snake_case")]
pub enum WorkbenchTab {
    Home,
    Session,
    Files,
    File(SurfaceId),
    CloseFile(SurfaceId),
    Diff(SurfaceId),
    CloseDiff(SurfaceId),
    NewDiff,
    Agents,
    Launcher,
}

/// Visual meaning only. Glyphs, fonts, assets and colors belong to adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Icon {
    Home,
    Conversation,
    Folder,
    File,
    Changes,
    Agents,
    Add,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub action: WorkbenchTab,
    pub label: String,
    pub icon: Icon,
    pub close: Option<WorkbenchTab>,
}
impl Tab {
    pub fn new(action: WorkbenchTab, resource_label: impl Into<String>) -> Self {
        let resource_label = resource_label.into();
        let (label, icon, close) = match action {
            WorkbenchTab::Home => ("Home".into(), Icon::Home, None),
            WorkbenchTab::Session => (resource_label, Icon::Conversation, None),
            WorkbenchTab::Files => ("Files".into(), Icon::Folder, None),
            WorkbenchTab::Agents => ("Agents".into(), Icon::Agents, None),
            WorkbenchTab::File(id) => (
                resource_label,
                Icon::File,
                Some(WorkbenchTab::CloseFile(id)),
            ),
            WorkbenchTab::Diff(id) => (
                resource_label,
                Icon::Changes,
                Some(WorkbenchTab::CloseDiff(id)),
            ),
            WorkbenchTab::NewDiff => ("Diff".into(), Icon::Changes, None),
            WorkbenchTab::Launcher => ("Open a view".into(), Icon::Add, None),
            WorkbenchTab::CloseFile(_) | WorkbenchTab::CloseDiff(_) => {
                (resource_label, Icon::File, None)
            }
        };
        Self {
            action,
            label,
            icon,
            close,
        }
    }
}

/// Ordered header controls. Hosts provide resource display names and observed
/// file presentation; action identity and close/launcher policy remain shared.
pub fn header(
    resources: &super::navigation::WorkbenchResources,
    mut resource_label: impl FnMut(WorkbenchTab) -> String,
    changed_file: Option<SurfaceId>,
    active: WorkbenchTab,
) -> Vec<Tab> {
    let mut controls: Vec<_> = resources
        .tabs()
        .into_iter()
        .map(|action| {
            let mut tab = Tab::new(action, resource_label(action));
            if action == active
                && matches!(action, WorkbenchTab::File(id) if Some(id) == changed_file)
            {
                tab.icon = Icon::Changes;
            }
            tab
        })
        .collect();
    controls.push(Tab::new(WorkbenchTab::Launcher, String::new()));
    controls
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LauncherItem {
    pub action: WorkbenchTab,
    pub label: &'static str,
    pub description: &'static str,
    pub icon: Icon,
}

pub const LAUNCHER: &[LauncherItem] = &[
    LauncherItem {
        action: WorkbenchTab::Home,
        label: "Home",
        description: "Workspace overview",
        icon: Icon::Home,
    },
    LauncherItem {
        action: WorkbenchTab::Session,
        label: "Session",
        description: "Current conversation",
        icon: Icon::Conversation,
    },
    LauncherItem {
        action: WorkbenchTab::Files,
        label: "Files",
        description: "Browse workspace files",
        icon: Icon::Folder,
    },
    LauncherItem {
        action: WorkbenchTab::NewDiff,
        label: "Diff",
        description: "Review Git changes",
        icon: Icon::Changes,
    },
    LauncherItem {
        action: WorkbenchTab::Agents,
        label: "Agents",
        description: "Delegated agent group",
        icon: Icon::Agents,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_ui::surfaces::Tabs;

    #[test]
    fn serialized_controls_keep_instance_identity_after_another_tab_closes() {
        let mut tabs = Tabs::default();
        let first = tabs.open("same", ());
        let second = tabs.open("same", ());
        let resources = super::super::navigation::WorkbenchResources {
            diffs: vec![first, second],
            ..Default::default()
        };
        let controls = header(&resources, |_| "same".into(), None, WorkbenchTab::Session);
        assert_eq!(controls.last().unwrap().action, WorkbenchTab::Launcher);
        let control = controls
            .into_iter()
            .find(|tab| tab.action == WorkbenchTab::Diff(second))
            .unwrap();
        let wire = serde_json::to_string(&control).unwrap();
        tabs.close(first);
        let decoded: Tab = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded.action, WorkbenchTab::Diff(second));
        assert_eq!(decoded.close, Some(WorkbenchTab::CloseDiff(second)));
        assert_eq!(decoded.icon, Icon::Changes);
        assert!(tabs.activate(second));
    }
}
