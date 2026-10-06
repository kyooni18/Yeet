use super::*;
use crate::harness::{HarnessCommand, HarnessState};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsState {
    pub selected: Option<String>,
    pub editor: Option<SettingsEditorView>,
    pub error: Option<String>,
}
impl SettingsState {
    pub fn view(&self, env: &SettingsEnvironment, harness: &HarnessState) -> SettingsView {
        super::projection::project(self, env, harness)
    }
    pub fn reconcile(&mut self, env: &SettingsEnvironment, harness: &HarnessState) {
        let view = self.view(env, harness);
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !view.controls().any(|control| control.id == *id))
        {
            self.selected = None;
        }
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| !env.supported_editors.contains(&editor.kind))
        {
            self.editor = None;
        }
    }
    pub fn apply(
        &mut self,
        action: SettingsAction,
        env: &SettingsEnvironment,
        harness: &HarnessState,
    ) -> SettingsEffect {
        let mut effect = SettingsEffect::default();
        let view = self.view(env, harness);
        match action {
            SettingsAction::Select(id) => {
                self.selected = id.filter(|id| view.controls().any(|control| control.id == *id))
            }
            SettingsAction::MoveSelection(delta) => {
                let ids = view
                    .controls()
                    .map(|control| control.id.clone())
                    .collect::<Vec<_>>();
                let target = self
                    .selected
                    .as_ref()
                    .and_then(|id| ids.iter().position(|item| item == id))
                    .map(|index| {
                        index
                            .saturating_add_signed(delta)
                            .min(ids.len().saturating_sub(1))
                    })
                    .unwrap_or(if delta < 0 {
                        ids.len().saturating_sub(1)
                    } else {
                        0
                    });
                self.selected = ids.get(target).cloned();
            }
            SettingsAction::Refresh if env.available => {
                effect.command = Some(HarnessCommand::RequestSettings)
            }
            SettingsAction::CancelEditor => {
                self.editor = None;
                self.error = None;
            }
            SettingsAction::Activate(id) => {
                let Some(control) = view
                    .controls()
                    .find(|control| control.id == id && control.enabled)
                else {
                    return effect;
                };
                self.selected = Some(id.clone());
                match control.kind {
                    SettingsControlKind::Toggle => {
                        return self.apply(control.action.clone(), env, harness);
                    }
                    SettingsControlKind::Choice => {
                        let next = control
                            .options
                            .iter()
                            .position(|option| option.selected)
                            .map(|index| (index + 1) % control.options.len())
                            .unwrap_or(0);
                        if let Some(option) = control.options.get(next) {
                            return self.apply(option.action.clone(), env, harness);
                        }
                    }
                    SettingsControlKind::Editor => {
                        let kind = match id.as_str() {
                            "context_length" => SettingsEditorKind::ContextLength,
                            "theme_dark" => SettingsEditorKind::DarkTheme,
                            "theme_light" => SettingsEditorKind::LightTheme,
                            _ => return effect,
                        };
                        self.editor = Some(SettingsEditorView {
                            kind,
                            label: control.label.clone(),
                            value: control.value.clone(),
                            hint: control.detail.clone(),
                        });
                    }
                    SettingsControlKind::Navigation => {
                        if let Some(provider) = &control.provider {
                            if let Some(provider) = harness
                                .auth_providers
                                .iter()
                                .find(|item| item.provider == *provider)
                            {
                                effect.command = Some(if provider.authenticated {
                                    HarnessCommand::AuthLogout {
                                        provider: provider.provider.clone(),
                                    }
                                } else {
                                    HarnessCommand::AuthLogin {
                                        provider: provider.provider.clone(),
                                    }
                                });
                            }
                        } else {
                            effect.ui.destination = Some(match id.as_str() {
                                "model" => SettingsDestination::Models,
                                "reasoning" => SettingsDestination::Reasoning,
                                "agents" => SettingsDestination::Agents,
                                "permissions" => SettingsDestination::Permissions,
                                "auth" => SettingsDestination::Auth,
                                "providers" => SettingsDestination::Providers,
                                "capabilities" => SettingsDestination::Capabilities,
                                _ => return effect,
                            });
                        }
                    }
                }
            }
            SettingsAction::SetToggle { id, enabled } => {
                let Some(control) = view.controls().find(|control| {
                    control.id == id
                        && control.enabled
                        && control.kind == SettingsControlKind::Toggle
                }) else {
                    return effect;
                };
                effect.command = Some(match id.as_str() {
                    "openai_flex" => HarnessCommand::SetOpenAiFlex { enabled },
                    "foundation_memory" => HarnessCommand::SetFoundationMemory { enabled },
                    _ => {
                        let Some(capability) = id.strip_prefix("capability:").and_then(|id| {
                            harness
                                .available_capabilities
                                .iter()
                                .find(|capability| capability.id == id)
                        }) else {
                            return effect;
                        };
                        if capability.enabled == enabled {
                            return effect;
                        }
                        HarnessCommand::ToggleCapability {
                            id: capability.id.clone(),
                        }
                    }
                });
                self.selected = Some(control.id.clone());
            }
            SettingsAction::SetChoice { id, value } => {
                let Some(control) = view.controls().find(|control| {
                    control.id == id
                        && control.enabled
                        && control.options.iter().any(|option| option.value == value)
                }) else {
                    return effect;
                };
                effect.command = Some(match id.as_str() {
                    "appearance" => HarnessCommand::SetAppearance { appearance: value },
                    "jev_loop" => HarnessCommand::SetJevLoopMode { mode: value },
                    "memory_backend" => HarnessCommand::SetServiceBackend {
                        service: "memory".into(),
                        backend: value,
                        server: Some(harness.foundation_memory_server.clone()),
                    },
                    "web_backend" => HarnessCommand::SetServiceBackend {
                        service: "web".into(),
                        backend: value,
                        server: Some(harness.web_server.clone()),
                    },
                    _ => return effect,
                });
                self.selected = Some(control.id.clone());
            }
            SettingsAction::SubmitEditor(value) if env.available && !harness.settings_working => {
                let Some(editor) = self.editor.clone() else {
                    return effect;
                };
                effect.command = Some(match editor.kind {
                    SettingsEditorKind::ContextLength => {
                        let value = value.trim();
                        let length = if value.is_empty()
                            || value.eq_ignore_ascii_case("auto")
                            || value.eq_ignore_ascii_case("reset")
                        {
                            None
                        } else {
                            match crate::config::parse_context_length(value) {
                                Ok(value) => Some(value),
                                Err(error) => {
                                    self.error = Some(error.to_string());
                                    return effect;
                                }
                            }
                        };
                        HarnessCommand::SetContextLength { length }
                    }
                    SettingsEditorKind::DarkTheme | SettingsEditorKind::LightTheme => {
                        if value.trim().is_empty() {
                            self.error = Some("Theme name or path cannot be empty".into());
                            return effect;
                        }
                        HarnessCommand::SetTheme {
                            mode: if editor.kind == SettingsEditorKind::DarkTheme {
                                "dark"
                            } else {
                                "light"
                            }
                            .into(),
                            value: value.trim().into(),
                        }
                    }
                });
                self.error = None;
                effect.ui.accepted_editor = Some(editor);
                self.editor = None;
            }
            _ => {}
        }
        effect
    }
}
