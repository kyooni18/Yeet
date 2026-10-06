use crate::harness::HarnessCommand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerContext {
    pub workspace: String,
    pub session_id: Option<String>,
    pub unsaved_generation: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComposerMode {
    #[default]
    Draft,
    EditLast {
        has_attachments: bool,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerAttachment {
    /// Native stable identity; upload/preview resources never enter UI.
    pub id: String,
    pub name: String,
    pub attachment_id: Option<String>,
    pub ready: bool,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSnapshot {
    pub context: ComposerContext,
    pub text: String,
    pub revision: u64,
    pub mode: ComposerMode,
    #[serde(default)]
    pub attachments: Vec<ComposerAttachment>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposerDestination {
    Models,
    Sessions,
    Settings,
    Reasoning,
    Goal,
    Agents,
    Files,
    Views,
    Capabilities,
    Permissions,
    Status,
    Auth,
    Providers,
    Debate,
    Help,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposerEnvironment {
    pub context: ComposerContext,
    pub available: bool,
    pub supported_destinations: Vec<ComposerDestination>,
    /// Inspection shortcuts may remain usable during execution when supported.
    pub allow_commands_while_streaming: bool,
    /// Permission shortcuts can capture editor input without platform types.
    #[serde(default)]
    pub freeze_editor_for_permissions: bool,
}
impl Default for ComposerEnvironment {
    fn default() -> Self {
        Self {
            context: ComposerContext::default(),
            available: true,
            supported_destinations: Vec::new(),
            allow_commands_while_streaming: false,
            freeze_editor_for_permissions: false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Shell,
    NativeApp,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionTarget {
    pub kind: PermissionKind,
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ComposerAction {
    UpdateEditor(EditorSnapshot),
    Submit(EditorSnapshot),
    SelectSuggestion {
        editor: EditorSnapshot,
        command: String,
    },
    CancelEdit(EditorSnapshot),
    Interrupt,
    NewSession,
    RespondPermission {
        target: PermissionTarget,
        allow: bool,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposerIcon {
    Send,
    Stop,
    Edit,
    Close,
    Terminal,
    Application,
    Allow,
    Deny,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposerControl {
    pub label: String,
    pub icon: ComposerIcon,
    pub action: ComposerAction,
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionView {
    pub target: PermissionTarget,
    pub title: String,
    pub detail: String,
    pub operation: String,
    pub reason: String,
    pub icon: ComposerIcon,
    pub controls: Vec<ComposerControl>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposerView {
    pub context: ComposerContext,
    pub editor: EditorSnapshot,
    pub placeholder: String,
    pub editable: bool,
    pub can_submit: bool,
    pub primary_control: ComposerControl,
    pub edit_banner: Option<String>,
    pub cancel_edit_control: Option<ComposerControl>,
    pub permissions: Vec<PermissionView>,
    pub suggestions: Vec<ComposerSuggestion>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComposerUiEffect {
    /// Clear only when native context/revision/text/mode/attachments still match.
    pub accepted_editor: Option<EditorSnapshot>,
    pub cancel_edit: Option<EditorSnapshot>,
    pub destination: Option<ComposerDestination>,
    pub replace_editor: Option<EditorReplacement>,
}
#[derive(Debug, Clone, Default)]
pub struct ComposerEffect {
    pub command: Option<HarnessCommand>,
    pub ui: ComposerUiEffect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposerSuggestion {
    pub command: String,
    pub description: String,
    pub arguments: Option<String>,
    pub destination: Option<ComposerDestination>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorReplacement {
    pub editor: EditorSnapshot,
    pub text: String,
}
