//! Shared transcript controls and semantic view descriptions.
use crate::model::{ConversationEntry, ConversationToolCall, ModelActivity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationIcon {
    Terminal,
    Edit,
    File,
    Search,
    Folder,
    Screen,
    Web,
    Mcp,
    Skill,
    Tool,
    Reasoning,
    Info,
    Error,
    Copy,
    Refresh,
    Activity,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailSection {
    pub title: String,
    pub content: String,
    pub monospaced: bool,
    pub is_error: bool,
    pub has_background: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolView {
    pub raw_tool: ConversationToolCall,
    pub title: String,
    pub summary: Option<String>,
    pub status_label: Option<String>,
    pub metadata: Option<String>,
    pub active: bool,
    pub failed: bool,
    pub awaits_permission: bool,
    pub icon: ConversationIcon,
    pub details: Vec<DetailSection>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceView {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub status_label: Option<String>,
    pub metadata: Option<String>,
    pub icon: ConversationIcon,
    pub active: bool,
    pub expanded: bool,
    pub details: Vec<DetailSection>,
}
impl TraceView {
    /// Only exceptional states need visible status copy beside the action.
    pub fn attention_label(&self) -> Option<&str> {
        self.status_label.as_deref().filter(|label| {
            let label = label.to_ascii_lowercase();
            [
                "permission",
                "approv",
                "fail",
                "error",
                "denied",
                "timed",
                "cancel",
                "interrupt",
            ]
            .iter()
            .any(|word| label.contains(word))
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityEvent {
    Activity {
        id: String,
        key: String,
        activity: ModelActivity,
        is_active: bool,
        trace: TraceView,
    },
    Tool {
        key: String,
        tool: ConversationToolCall,
        view: ToolView,
        trace: TraceView,
    },
    Skill {
        id: String,
        key: String,
        name: String,
        content: String,
        status: Option<String>,
        trace: TraceView,
    },
    Mcp {
        id: String,
        key: String,
        server: String,
        name: String,
        content: String,
        is_error: bool,
        trace: TraceView,
    },
}
impl ActivityEvent {
    pub fn key(&self) -> &str {
        match self {
            Self::Activity { key, .. }
            | Self::Tool { key, .. }
            | Self::Skill { key, .. }
            | Self::Mcp { key, .. } => key,
        }
    }
    pub fn trace(&self) -> &TraceView {
        match self {
            Self::Activity { trace, .. }
            | Self::Tool { trace, .. }
            | Self::Skill { trace, .. }
            | Self::Mcp { trace, .. } => trace,
        }
    }
    pub fn failed(&self) -> bool {
        match self {
            Self::Tool { view, .. } => view.failed,
            Self::Mcp { is_error, .. } => *is_error,
            _ => false,
        }
    }
    pub fn awaits_permission(&self) -> bool {
        matches!(self,Self::Tool{view,..} if view.awaits_permission)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityGroup {
    pub id: String,
    pub events: Vec<ActivityEvent>,
    pub summary: String,
    /// Compact exceptional-state copy, projected once for all adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_label: Option<String>,
    pub active: bool,
    pub failed: bool,
    pub awaits_permission: bool,
    pub expanded: bool,
}
impl ActivityGroup {
    /// A singleton is one disclosure, not a group wrapped around the same row.
    /// Keep the group's stable ID so expansion survives appended tool events.
    pub fn single_trace(&self) -> Option<TraceView> {
        if self.events.len() != 1 {
            return None;
        }
        let mut trace = self.events[0].trace().clone();
        trace.id = self.id.clone();
        trace.expanded = self.expanded;
        Some(trace)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ConversationAction {
    Toggle(String),
    Select(Option<String>),
    MoveSelection(isize),
    SetExpandAll(bool),
    SetInspectWork(bool),
    Copy(String),
    Edit(String),
    Regenerate(String),
    Reset,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageControl {
    pub label: String,
    pub icon: ConversationIcon,
    pub action: ConversationAction,
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DisplayItem {
    Entry {
        id: String,
        entry: ConversationEntry,
        controls: Vec<MessageControl>,
    },
    Activity {
        id: String,
        group: ActivityGroup,
    },
    Reasoning {
        id: String,
        content: String,
        summary: Option<String>,
        is_active: bool,
        trace: TraceView,
    },
}
impl DisplayItem {
    pub fn id(&self) -> &str {
        match self {
            Self::Entry { id, .. } | Self::Activity { id, .. } | Self::Reasoning { id, .. } => id,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationView {
    pub items: Vec<DisplayItem>,
    pub last_user_id: Option<String>,
    pub last_assistant_id: Option<String>,
    pub streaming_text: String,
    pub streaming: bool,
    pub preparing: bool,
    pub preparing_label: String,
    pub error: Option<String>,
    pub selected: Option<String>,
}
impl ConversationView {
    /// Visible work disclosures, in the same order as native row geometry.
    pub fn work_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for item in &self.items {
            match item {
                DisplayItem::Entry { .. } => {}
                DisplayItem::Reasoning { id, .. } => ids.push(id.clone()),
                DisplayItem::Activity { id, group } => {
                    ids.push(id.clone());
                    if group.expanded && group.events.len() > 1 {
                        ids.extend(
                            group
                                .events
                                .iter()
                                .filter(|event| {
                                    !event.trace().details.is_empty()
                                        || event.trace().metadata.is_some()
                                })
                                .map(|event| event.key().to_owned()),
                        );
                    }
                }
            }
        }
        ids
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationEffect {
    pub command: Option<crate::harness::HarnessCommand>,
    pub copy: Option<CopyEffect>,
    pub edit_draft: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyEffect {
    pub entry_id: String,
    pub text: String,
}

/// Native clipboard/editor effects; execution commands remain host-internal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationUiEffect {
    pub copy: Option<CopyEffect>,
    pub edit_draft: Option<String>,
}
impl From<&ConversationEffect> for ConversationUiEffect {
    fn from(effect: &ConversationEffect) -> Self {
        Self {
            copy: effect.copy.clone(),
            edit_draft: effect.edit_draft.clone(),
        }
    }
}
