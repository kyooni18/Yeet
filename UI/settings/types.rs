use crate::harness::HarnessCommand;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsIcon {
    Appearance,
    Settings,
    Memory,
    Web,
    Model,
    Reasoning,
    Context,
    Theme,
    Policy,
    Agents,
    Permissions,
    Provider,
    Capabilities,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsDestination {
    Models,
    Reasoning,
    Agents,
    Permissions,
    Auth,
    Providers,
    Capabilities,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsEditorKind {
    ContextLength,
    DarkTheme,
    LightTheme,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsEnvironment {
    pub supported_features: Vec<SettingsFeature>,
    pub available: bool,
    pub supported_destinations: Vec<SettingsDestination>,
    pub supported_editors: Vec<SettingsEditorKind>,
}
impl Default for SettingsEnvironment {
    fn default() -> Self {
        Self {
            supported_features: Vec::new(),
            available: true,
            supported_destinations: Vec::new(),
            supported_editors: Vec::new(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SettingsAction {
    Select(Option<String>),
    MoveSelection(isize),
    Activate(String),
    SetChoice { id: String, value: String },
    SetToggle { id: String, enabled: bool },
    SubmitEditor(String),
    CancelEditor,
    Refresh,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsControlKind {
    Toggle,
    Choice,
    Navigation,
    Editor,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
    pub action: SettingsAction,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsControl {
    pub provider: Option<String>,
    pub action_label: String,
    pub id: String,
    pub label: String,
    pub detail: String,
    pub icon: SettingsIcon,
    pub kind: SettingsControlKind,
    pub value: String,
    pub checked: Option<bool>,
    pub enabled: bool,
    pub options: Vec<SettingsOption>,
    pub action: SettingsAction,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsSection {
    pub id: String,
    pub label: String,
    pub icon: SettingsIcon,
    pub controls: Vec<SettingsControl>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsEditorView {
    pub kind: SettingsEditorKind,
    pub label: String,
    pub value: String,
    pub hint: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsView {
    pub title: String,
    pub subtitle: String,
    pub sections: Vec<SettingsSection>,
    pub selected: Option<String>,
    pub editor: Option<SettingsEditorView>,
    pub notice: Option<String>,
    pub working: bool,
}
impl SettingsView {
    pub fn controls(&self) -> impl Iterator<Item = &SettingsControl> {
        self.sections
            .iter()
            .flat_map(|section| section.controls.iter())
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsUiEffect {
    pub destination: Option<SettingsDestination>,
    pub accepted_editor: Option<SettingsEditorView>,
}
#[derive(Debug, Clone, Default)]
pub struct SettingsEffect {
    pub command: Option<HarnessCommand>,
    pub ui: SettingsUiEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsFeature {
    ServiceBackends,
    JevLoop,
}
