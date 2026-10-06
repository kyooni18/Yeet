//! Ordered application controls; native widgets, focus and geometry stay in adapters.
use super::{
    composer::{ComposerDestination, ComposerView},
    shell::ShellState,
};
use crate::harness::{HarnessCommand, HarnessState};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarIcon {
    Menu,
    NewSession,
    Controls,
    Interrupt,
    Model,
    Reasoning,
    Goal,
    Settings,
    Sessions,
    Files,
    Capabilities,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarControlKind {
    Button,
    Choice,
    Toggle,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickSection {
    Workspace,
    Response,
    Permissions,
    Sandbox,
    Context,
    Usage,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ToolbarAction {
    Activate(String),
    Choose { id: String, value: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolbarOption {
    pub value: String,
    pub label: String,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolbarControl {
    pub id: String,
    pub label: String,
    pub icon: ToolbarIcon,
    pub kind: ToolbarControlKind,
    pub value: String,
    pub enabled: bool,
    pub pressed: Option<bool>,
    pub options: Vec<ToolbarOption>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolbarGroup {
    pub id: String,
    pub controls: Vec<ToolbarControl>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolbarView {
    pub groups: Vec<ToolbarGroup>,
    pub quick_title: String,
    pub quick_sections: Vec<QuickSection>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolbarUiEffect {
    pub destination: Option<ComposerDestination>,
}
#[derive(Debug, Clone, Default)]
pub struct ToolbarEffect {
    pub command: Option<HarnessCommand>,
    pub destination: Option<ComposerDestination>,
}
#[derive(Clone, Default)]
pub(super) struct ToolbarRuntime {
    model: String,
    reasoning: String,
    goal: bool,
    streaming: bool,
    sandbox: bool,
    context: bool,
}
impl ToolbarRuntime {
    pub fn update(&mut self, h: &HarnessState) {
        self.model = h.active_model.clone();
        self.reasoning = h.active_reasoning_level.clone();
        self.goal = h.goal_mode;
        self.streaming = h.is_streaming;
        self.sandbox = h.sandbox_settings.is_some();
        self.context = h.current_context_tokens.is_some()
            && h.active_model_context_length.is_some_and(|total| total > 0);
    }
    pub fn view(
        &self,
        shell: &ShellState,
        composer: &ComposerView,
        available: bool,
    ) -> ToolbarView {
        let control =
            |id: &str, label: &str, icon: ToolbarIcon, enabled: bool, pressed: Option<bool>| {
                ToolbarControl {
                    id: id.into(),
                    label: label.into(),
                    icon,
                    kind: ToolbarControlKind::Button,
                    value: String::new(),
                    enabled,
                    pressed,
                    options: Vec::new(),
                }
            };
        let mut reasoning = control(
            "reasoning",
            "Reasoning",
            ToolbarIcon::Reasoning,
            available,
            None,
        );
        reasoning.kind = ToolbarControlKind::Choice;
        reasoning.value = if self.reasoning.is_empty() {
            "auto".into()
        } else {
            self.reasoning.clone()
        };
        reasoning.options = crate::model::reasoning_levels_for_model(&self.model)
            .iter()
            .map(|v| ToolbarOption {
                value: (*v).into(),
                description: match *v {
                    "auto" => "Model default",
                    "low" => "Faster responses",
                    "medium" => "Balanced reasoning",
                    "high" => "Deeper reasoning",
                    "xhigh" => "Extended reasoning",
                    "max" => "Maximum reasoning",
                    _ => "",
                }
                .into(),
                label: match *v {
                    "xhigh" => "Extra High".into(),
                    v => {
                        let mut chars = v.chars();
                        chars
                            .next()
                            .map(|c| c.to_uppercase().to_string() + chars.as_str())
                            .unwrap_or_default()
                    }
                },
            })
            .collect();
        let mut goal = control(
            "goal",
            "Goal mode",
            ToolbarIcon::Goal,
            available,
            Some(self.goal),
        );
        goal.kind = ToolbarControlKind::Toggle;
        goal.value = self.goal.to_string();
        goal.options = vec![
            ToolbarOption {
                value: "true".into(),
                label: "ON".into(),
                description: "Strict success judged execution".into(),
            },
            ToolbarOption {
                value: "false".into(),
                label: "OFF".into(),
                description: "Normal completion behavior".into(),
            },
        ];
        let mut model = control("models", "Model", ToolbarIcon::Model, available, None);
        model.value = self.model.clone();
        let mut quick_sections = vec![QuickSection::Workspace, QuickSection::Response];
        if !composer.permissions.is_empty() {
            quick_sections.push(QuickSection::Permissions);
        }
        if self.sandbox {
            quick_sections.push(QuickSection::Sandbox);
        }
        if self.context {
            quick_sections.push(QuickSection::Context);
        }
        quick_sections.push(QuickSection::Usage);
        ToolbarView {
            quick_sections,
            quick_title: if self.streaming {
                "Working"
            } else {
                "Controls"
            }
            .into(),
            groups: vec![
                ToolbarGroup {
                    id: "header_leading".into(),
                    controls: vec![control(
                        "navigation",
                        "Open sidebar",
                        ToolbarIcon::Menu,
                        true,
                        Some(shell.navigation),
                    )],
                },
                ToolbarGroup {
                    id: "header_actions".into(),
                    controls: vec![
                        control(
                            "new_session",
                            "New chat",
                            ToolbarIcon::NewSession,
                            available,
                            None,
                        ),
                        control(
                            "quick_controls",
                            "Quick settings",
                            ToolbarIcon::Controls,
                            true,
                            Some(shell.inspector),
                        ),
                    ],
                },
                ToolbarGroup {
                    id: "session_actions".into(),
                    controls: vec![control(
                        "interrupt",
                        "Interrupt run",
                        ToolbarIcon::Interrupt,
                        available && self.streaming,
                        None,
                    )],
                },
                ToolbarGroup {
                    id: "quick_response".into(),
                    controls: vec![model, reasoning, goal],
                },
                ToolbarGroup {
                    id: "quick_footer".into(),
                    controls: vec![control(
                        "settings",
                        "Settings",
                        ToolbarIcon::Settings,
                        true,
                        None,
                    )],
                },
                ToolbarGroup {
                    id: "native_navigation".into(),
                    controls: vec![
                        control(
                            "sessions",
                            "Sessions",
                            ToolbarIcon::Sessions,
                            available,
                            None,
                        ),
                        control("files", "Files", ToolbarIcon::Files, true, None),
                        control(
                            "capabilities",
                            "Capabilities",
                            ToolbarIcon::Capabilities,
                            available,
                            None,
                        ),
                    ],
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_ui::application_session::ApplicationSession;
    #[test]
    fn ordered_controls_validate_live_choices_and_delegate_composer_commands() {
        let mut state = HarnessState::default();
        state.active_model = "openai/gpt-5.6-sol".into();
        state.is_streaming = true;
        let mut ui = ApplicationSession::new(&state);
        let view = ui.toolbar_view();
        assert_eq!(
            view.groups[1]
                .controls
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["new_session", "quick_controls"]
        );
        let interrupt = ui.prepare_toolbar(ToolbarAction::Activate("interrupt".into()), &state);
        assert!(matches!(
            interrupt.effect.command,
            Some(HarnessCommand::Interrupt)
        ));
        let invalid = ui.prepare_toolbar(
            ToolbarAction::Choose {
                id: "reasoning".into(),
                value: "bogus".into(),
            },
            &state,
        );
        assert!(invalid.effect.command.is_none());
        let reasoning = ui.prepare_toolbar(
            ToolbarAction::Choose {
                id: "reasoning".into(),
                value: "max".into(),
            },
            &state,
        );
        assert!(
            matches!(reasoning.effect.command,Some(HarnessCommand::SelectReasoning{level})if level=="max")
        );
        let before = ui.shell_projection();
        let prepared = ui.prepare_toolbar(ToolbarAction::Activate("models".into()), &state);
        assert!(
            !ui.shell_projection().state.models,
            "preparing does not mutate UI before successful delivery"
        );
        assert_eq!(ui.shell_projection().ui_revision, before.ui_revision);
        ui.commit_toolbar(prepared, &state);
        assert!(ui.shell_projection().state.models);
        state.is_streaming = false;
        assert!(
            ui.prepare_toolbar(ToolbarAction::Activate("interrupt".into()), &state)
                .effect
                .command
                .is_none()
        );
    }
}
