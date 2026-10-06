use super::*;
use crate::harness::{HarnessCommand, HarnessState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComposerState {
    pub editor: EditorSnapshot,
}
impl ComposerState {
    pub fn view(&self, env: &ComposerEnvironment, harness: &HarnessState) -> ComposerView {
        let editor = self.editor.clone();
        let can_submit = eligible(&editor, env, harness);
        let editing = matches!(editor.mode, ComposerMode::EditLast { .. });
        ComposerView {
            context: env.context.clone(),
            placeholder: if harness.is_streaming {
                "Draft your next message…"
            } else if !env.available {
                "Draft while reconnecting…"
            } else {
                "Message"
            }
            .into(),
            editable: !env.freeze_editor_for_permissions || permissions(harness, env.available).is_empty(),
            can_submit,
            primary_control: if harness.is_streaming {
                control(
                    "Stop",
                    ComposerIcon::Stop,
                    ComposerAction::Interrupt,
                    env.available,
                )
            } else {
                control(
                    "Send",
                    ComposerIcon::Send,
                    ComposerAction::Submit(editor.clone()),
                    can_submit,
                )
            },
            edit_banner: editing.then(|| {
                if matches!(
                    editor.mode,
                    ComposerMode::EditLast {
                        has_attachments: true
                    }
                ) {
                    "Editing last message · Existing attachments will be preserved.".into()
                } else {
                    "Editing last message".into()
                }
            }),
            cancel_edit_control: editing.then(|| {
                control(
                    "Cancel editing",
                    ComposerIcon::Close,
                    ComposerAction::CancelEdit(editor.clone()),
                    true,
                )
            }),
            permissions: permissions(harness, env.available),
            suggestions: suggestions(&editor.text, harness),
            editor,
        }
    }
    pub fn reconcile(&mut self, env: &ComposerEnvironment) {
        if self.editor.context != env.context {
            self.editor = EditorSnapshot {
                context: env.context.clone(),
                ..Default::default()
            };
        }
    }
    pub fn apply(
        &mut self,
        action: ComposerAction,
        env: &ComposerEnvironment,
        harness: &HarnessState,
    ) -> ComposerEffect {
        let mut effect = ComposerEffect::default();
        match action {
            ComposerAction::UpdateEditor(editor) => {
                if editor.context == env.context && editor.revision >= self.editor.revision {
                    self.editor = editor;
                }
            }
            ComposerAction::SelectSuggestion { editor, command } => {
                if editor.context != env.context || editor.revision < self.editor.revision {
                    return effect;
                }
                let Some(suggestion) = suggestions(&editor.text, harness)
                    .into_iter()
                    .find(|suggestion| suggestion.command == command)
                else {
                    return effect;
                };
                if suggestion
                    .destination
                    .is_some_and(|destination| env.supported_destinations.contains(&destination))
                {
                    let mut submitted = editor.clone();
                    submitted.text = command;
                    effect = self.apply(ComposerAction::Submit(submitted), env, harness);
                    if effect.ui.accepted_editor.is_some() {
                        effect.ui.accepted_editor = Some(editor);
                    }
                } else {
                    let text = format!(
                        "{}{}",
                        command,
                        if suggestion.arguments.is_some() {
                            " "
                        } else {
                            ""
                        }
                    );
                    effect.ui.replace_editor = Some(EditorReplacement { editor, text });
                }
            }
            ComposerAction::Submit(editor) => {
                if editor.revision < self.editor.revision || !eligible(&editor, env, harness) {
                    return effect;
                }
                let text = editor.text.trim().to_owned();
                if matches!(editor.mode, ComposerMode::EditLast { .. }) {
                    effect.command = Some(HarnessCommand::EditLast { text });
                } else if let Some(destination) = destination(&text)
                    .filter(|destination| env.supported_destinations.contains(destination))
                {
                    effect.ui.destination = Some(destination);
                } else if text == "/new" {
                    effect.command = Some(HarnessCommand::NewSession);
                } else if let Some(command) = extension_command_invocation(&text, harness) {
                    effect.command = Some(command);
                } else {
                    effect.command = Some(HarnessCommand::Submit {
                        text,
                        images: Vec::new(),
                        attachment_ids: editor
                            .attachments
                            .iter()
                            .filter_map(|attachment| attachment.attachment_id.clone())
                            .collect(),
                    });
                }
                effect.ui.accepted_editor = Some(editor.clone());
                self.editor = EditorSnapshot {
                    context: editor.context,
                    revision: editor.revision,
                    ..Default::default()
                };
            }
            ComposerAction::CancelEdit(editor) => {
                if editor.context == env.context
                    && editor.revision >= self.editor.revision
                    && matches!(editor.mode, ComposerMode::EditLast { .. })
                {
                    effect.ui.cancel_edit = Some(editor.clone());
                    self.editor = EditorSnapshot {
                        context: editor.context,
                        revision: editor.revision,
                        ..Default::default()
                    };
                }
            }
            ComposerAction::NewSession if env.available => {
                effect.command = Some(HarnessCommand::NewSession)
            }
            ComposerAction::Interrupt if env.available && harness.is_streaming => {
                effect.command = Some(HarnessCommand::Interrupt)
            }
            ComposerAction::RespondPermission { target, allow } if env.available => {
                let current = permissions(harness, true)
                    .into_iter()
                    .any(|permission| permission.target == target);
                if current {
                    effect.command = Some(HarnessCommand::ResolvePermission {
                        request_id: target.id,
                        granted: allow,
                    });
                }
            }
            _ => {}
        }
        effect
    }
}
fn control(
    label: &str,
    icon: ComposerIcon,
    action: ComposerAction,
    enabled: bool,
) -> ComposerControl {
    ComposerControl {
        label: label.into(),
        icon,
        action,
        enabled,
    }
}
fn eligible(editor: &EditorSnapshot, env: &ComposerEnvironment, harness: &HarnessState) -> bool {
    if !env.available || editor.context != env.context {
        return false;
    }
    let text = editor.text.trim();
    if harness.is_streaming
        && !(env.allow_commands_while_streaming
            && matches!(editor.mode, ComposerMode::Draft)
            && (text.starts_with('/') || text == "?"))
    {
        return false;
    }
    match editor.mode {
        ComposerMode::EditLast { has_attachments } => {
            !harness.is_streaming && (!text.is_empty() || has_attachments)
        }
        ComposerMode::Draft => {
            editor.attachments.iter().all(|attachment| {
                attachment.ready && attachment.error.is_none() && attachment.attachment_id.is_some()
            }) && (!text.is_empty() || !editor.attachments.is_empty())
        }
    }
}
fn destination(text: &str) -> Option<ComposerDestination> {
    use ComposerDestination::*;
    Some(match text {
        "/model" => Models,
        "/sessions" => Sessions,
        "/settings" => Settings,
        "/reasoning" => Reasoning,
        "/goal" => Goal,
        "/agent-group" => Agents,
        "/files" => Files,
        "/views" => Views,
        "/capabilities" => Capabilities,
        "/permissions" => Permissions,
        "/status" => Status,
        "/login" => Auth,
        "/provider" | "/providers" => Providers,
        "/debate" => Debate,
        "?" => Help,
        _ => return None,
    })
}
pub fn extension_command_invocation(text: &str, harness: &HarnessState) -> Option<HarnessCommand> {
    let mut parts = text.split_whitespace();
    let name = parts.next()?;
    harness
        .extension_commands
        .iter()
        .any(|command| command.command.eq_ignore_ascii_case(name))
        .then(|| HarnessCommand::ExtensionCommand {
            command: name.trim_start_matches('/').into(),
            args: parts.map(str::to_owned).collect(),
        })
}
fn permissions(harness: &HarnessState, available: bool) -> Vec<PermissionView> {
    let mut views = Vec::new();
    // Native application requests have richer identity; both requests remain
    // explicit so adapters never infer which command a displayed action targets.
    if let Some(permission) = &harness.pending_native_app_permission {
        views.push(permission_view(
            PermissionTarget {
                kind: PermissionKind::NativeApp,
                id: permission.id.clone(),
            },
            "Native app permission requested",
            format!(
                "{} · {}",
                permission.app_name.as_deref().unwrap_or(&permission.server),
                permission.tool
            ),
            permission.operation.clone(),
            permission.reason.clone(),
            ComposerIcon::Application,
            available,
        ));
    }
    if let Some(permission) = &harness.pending_shell_permission {
        views.push(permission_view(
            PermissionTarget {
                kind: PermissionKind::Shell,
                id: permission.id.clone(),
            },
            "Shell permission requested",
            permission.command.clone(),
            permission.operation.clone(),
            permission.reason.clone(),
            ComposerIcon::Terminal,
            available,
        ));
    }
    views
}
fn permission_view(
    target: PermissionTarget,
    title: &str,
    detail: String,
    operation: String,
    reason: String,
    icon: ComposerIcon,
    available: bool,
) -> PermissionView {
    PermissionView {
        controls: vec![
            control(
                "Deny",
                ComposerIcon::Deny,
                ComposerAction::RespondPermission {
                    target: target.clone(),
                    allow: false,
                },
                available,
            ),
            control(
                "Allow",
                ComposerIcon::Allow,
                ComposerAction::RespondPermission {
                    target: target.clone(),
                    allow: true,
                },
                available,
            ),
        ],
        target,
        title: title.into(),
        detail,
        operation,
        reason,
        icon,
    }
}

/// Application command suggestions; adapters may limit visible rows to fit.
pub fn suggestions(text: &str, harness: &HarnessState) -> Vec<ComposerSuggestion> {
    if !text.starts_with('/') || text.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let query = text.to_ascii_lowercase();
    let commands = [
        (
            "/debate",
            "Debate with independent Pro, Con and Jury models",
        ),
        ("/new", "Start a new session"),
        ("/model", "Select model"),
        ("/reasoning", "Select reasoning level"),
        ("/login", "Manage provider authentication"),
        ("/provider", "Manage custom API endpoints"),
        ("/providers", "Manage custom API endpoints"),
        ("/settings", "Runtime and application settings"),
        ("/permissions", "Sandbox and permission settings"),
        ("/sessions", "Browse saved chats"),
        ("/files", "Browse workspace files"),
        ("/views", "Switch view"),
        ("/capabilities", "Toggle skills, capabilities, and MCP"),
        ("/skyline", "Attach or detach Skyline coordination"),
        ("/image", "Queue an image for the next turn"),
        ("/compact", "Compact model context now"),
        ("/context", "Show or override model context length"),
        ("/status", "Show detailed runtime and usage status"),
        (
            "/goal",
            "Continue until a strict success judge accepts concrete evidence",
        ),
        (
            "/agent-group",
            "Agent Group: member coordination and shared budgets",
        ),
        ("/attach", "Attach an optional capability"),
        ("/detach", "Detach an optional capability"),
        ("/allow", "Allow pending shell command once"),
        ("/deny", "Deny pending shell command"),
        ("/permission", "Respond to pending permission"),
        ("/workspace", "Manage session workspace"),
        ("/cd", "Change session working directory"),
        ("/help", "Show commands"),
        ("/clear", "Dismiss latest system notice"),
    ];
    let mut items: Vec<_> = commands
        .into_iter()
        .map(|(command, description)| ComposerSuggestion {
            command: command.into(),
            description: description.into(),
            arguments: suggestion_arguments(command).map(str::to_owned),
            destination: destination(command),
        })
        .collect();
    for command in &harness.extension_commands {
        if !items
            .iter()
            .any(|item| item.command.eq_ignore_ascii_case(&command.command))
        {
            items.push(ComposerSuggestion {
                command: command.command.clone(),
                description: command.description.clone(),
                arguments: None,
                destination: None,
            });
        }
    }
    items.sort_by(|a, b| a.command.cmp(&b.command));
    items.retain(|item| item.command.to_ascii_lowercase().starts_with(&query));
    items
}

fn suggestion_arguments(command: &str) -> Option<&'static str> {
    Some(match command {
        "/debate" => "TOPIC",
        "/model" => "MODEL_ID",
        "/reasoning" => "auto|low|medium|high",
        "/goal" => "on|off|toggle|status",
        "/context" => "LENGTH|auto",
        "/workspace" => "list|cd|add|remove|reset PATH",
        "/cd" => "PATH",
        "/attach" | "/detach" => "ID",
        "/skyline" => "on|off",
        "/image" => "PATH|clear",
        "/permission" => "allow|deny",
        _ => return None,
    })
}
